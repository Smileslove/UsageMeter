//! cc-switch 兼容相关的 Tauri 命令与清洗触发编排

use crate::proxy::ccswitch_compat::{
    self, poll_should_trigger_clean, try_clean_ccswitch_db, CcSwitchEnv, CleanReport,
};
use crate::proxy::ProxyDatabase;
use tauri::{Emitter, State};

use super::usage::ProxyState;

/// cc-switch 兼容层的前端状态快照。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchCompatStatus {
    pub installed: bool,
    pub running: bool,
    pub db_exists: bool,
    /// 当前处于礼让状态的工具 ID 列表（如 "claude_code"、"codex"）。
    pub yielded_tools: Vec<String>,
    pub last_clean_at_ms: Option<i64>,
    pub last_report: Option<CleanReport>,
    pub pending_clean: bool,
    /// 最近一次清洗失败的错误码（如 ccswitchSchemaMismatch），成功后为 None。
    pub last_error_code: Option<String>,
    pub imported_requests: u64,
    pub last_import_at_ms: Option<i64>,
    pub last_import_error_code: Option<String>,
}

#[tauri::command]
pub async fn get_ccswitch_compat_status(
    state: State<'_, ProxyState>,
) -> Result<CcSwitchCompatStatus, String> {
    // 进程检测走外部命令，放到阻塞线程池执行。
    let (installed, running, db_exists) = tauri::async_runtime::spawn_blocking(|| {
        let env = CcSwitchEnv::new();
        let installed = env.is_installed();
        let running = installed && CcSwitchEnv::is_running();
        (installed, running, env.db_exists())
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut yielded_tools = Vec::new();
    {
        let server_guard = state.server.read().await;
        if let Some(server) = server_guard.as_ref() {
            for tool in ["claude_code", "codex"] {
                if server.yielded_external_manager(tool).await.is_some() {
                    yielded_tools.push(tool.to_string());
                }
            }
        }
    }

    let compat_state = ccswitch_compat::read_compat_state();
    let import_state = tauri::async_runtime::spawn_blocking(|| {
        ProxyDatabase::get_global()
            .and_then(|database| database.ccswitch_import_state().ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Ok(CcSwitchCompatStatus {
        installed,
        running,
        db_exists,
        yielded_tools,
        last_clean_at_ms: compat_state.last_clean_at_ms,
        last_report: compat_state.last_report,
        pending_clean: compat_state.pending_clean,
        last_error_code: compat_state.last_error_code,
        imported_requests: import_state.total_imported,
        last_import_at_ms: import_state.last_import_at_ms,
        last_import_error_code: import_state.last_error_code,
    })
}

/// Import one bounded batch of CC Switch proxy logs. Both the source read and local write run
/// off the async executor; failures only update integration status and never affect forwarding.
fn import_ccswitch_logs_once() {
    let Ok(database) = ProxyDatabase::get_or_create_global() else {
        return;
    };
    let Ok(mut state) = database.ccswitch_import_state() else {
        return;
    };
    let env = CcSwitchEnv::new();
    if !env.db_exists() {
        return;
    }
    let batch = match env.read_proxy_usage_batch(&state) {
        Ok(Some(batch)) => batch,
        Ok(None) => {
            if state.last_error_code.take().is_some() {
                let _ = database.save_ccswitch_import_state(&state);
            }
            return;
        }
        Err(code) => {
            if state.last_error_code.as_deref() != Some(code.as_str()) {
                state.last_error_code = Some(code.clone());
                let _ = database.save_ccswitch_import_state(&state);
                eprintln!("[ccswitch-import] import paused: {code}");
            }
            return;
        }
    };

    state.cursor_created_at = batch.last_created_at;
    state.cursor_request_id = batch.last_request_id;
    state.last_import_at_ms = Some(chrono::Utc::now().timestamp_millis());
    state.last_error_code = None;
    let imported = match database.insert_ccswitch_batch(&batch.records, &state) {
        Ok(imported) => imported,
        Err(_) => {
            state.last_error_code = Some("ccswitchImportWriteFailed".to_string());
            let _ = database.save_ccswitch_import_state(&state);
            eprintln!("[ccswitch-import] local batch write failed");
            return;
        }
    };

    if imported > 0 {
        eprintln!("[ccswitch-import] imported {imported} proxy request(s)");
    }
}

pub fn spawn_ccswitch_log_importer() {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(error) =
                tauri::async_runtime::spawn_blocking(import_ccswitch_logs_once).await
            {
                eprintln!("[ccswitch-import] worker failed: {error}");
            }
        }
    });
}

/// 手动清洗 cc-switch 供应商库。cc-switch 运行中返回错误码 `ccswitchRunning`（绝不强清）。
#[tauri::command]
pub async fn run_ccswitch_db_clean(app: tauri::AppHandle) -> Result<CleanReport, String> {
    let report = tauri::async_runtime::spawn_blocking(ccswitch_compat::clean_ccswitch_db_now)
        .await
        .map_err(|e| e.to_string())??;
    emit_db_cleaned(&app, &report);
    Ok(report)
}

fn emit_db_cleaned(app: &tauri::AppHandle, report: &CleanReport) {
    let _ = app.emit("ccswitch_db_cleaned", report.clone());
}

/// 自动清洗触发：在阻塞线程池中执行守卫检查与清洗，完成后按需通知前端。
///
/// 供应用启动等 fire-and-forget 时机调用；退出路径请用
/// `run_ccswitch_auto_clean_blocking` 以保证清洗在进程退出前完成。
pub fn spawn_ccswitch_auto_clean(app_handle: Option<tauri::AppHandle>, reason: &'static str) {
    tauri::async_runtime::spawn_blocking(move || {
        run_auto_clean_sync(app_handle, reason);
    });
}

/// 自动清洗触发（等待完成）：在阻塞线程池中执行清洗并 await 结果，超时放弃等待。
///
/// 供代理停止/应用退出路径调用——退出流程随后会 `app.exit(0)`，fire-and-forget
/// 的清洗任务会被杀掉跑不完。超时后任务仍在后台继续，SQLite 事务保证即便
/// 进程退出也不会损坏 cc-switch DB。
pub async fn run_ccswitch_auto_clean_blocking(
    app_handle: Option<tauri::AppHandle>,
    reason: &'static str,
    timeout: std::time::Duration,
) {
    let task =
        tauri::async_runtime::spawn_blocking(move || run_auto_clean_sync(app_handle, reason));
    if tokio::time::timeout(timeout, task).await.is_err() {
        eprintln!(
            "[ccswitch-compat] auto clean still running after {}ms (reason={reason}), continuing in background",
            timeout.as_millis()
        );
    }
}

fn run_auto_clean_sync(app_handle: Option<tauri::AppHandle>, reason: &'static str) {
    if let Some(report) = try_clean_ccswitch_db(reason) {
        if report.cleaned > 0 || report.unresolved > 0 {
            if let Some(app) = app_handle {
                emit_db_cleaned(&app, &report);
            }
        }
    }
}

/// 监控循环用的边沿触发：检测 cc-switch 退出（或存在 pending 清洗）时执行清洗。
///
/// 整个检查（含进程探测）在阻塞线程池中进行，调用方每个 tick 直接调用即可。
pub fn poll_ccswitch_exit_and_clean(app_handle: Option<tauri::AppHandle>) {
    tauri::async_runtime::spawn_blocking(move || {
        if !poll_should_trigger_clean() {
            return;
        }
        if let Some(report) = try_clean_ccswitch_db("ccswitch_exit") {
            if report.cleaned > 0 || report.unresolved > 0 {
                if let Some(app) = app_handle {
                    emit_db_cleaned(&app, &report);
                }
            }
        }
    });
}
