//! cc-switch 兼容层
//!
//! cc-switch（第三方供应商切换器）会在切换供应商前把当前 live 配置回填进它自己的
//! SQLite 供应商库（~/.cc-switch/cc-switch.db）。当 UsageMeter 的代理接管处于活跃
//! 状态时，live 配置中的本地代理地址会被 cc-switch 当作普通第三方地址吸收，造成
//! 永久污染：UsageMeter 退出后，用户在 cc-switch 中切回被污染的供应商会把一个已
//! 失效的 localhost 地址写回 live 配置。
//!
//! 本模块提供三类能力：
//! 1. 环境与进程检测（`CcSwitchEnv`）；
//! 2. cc-switch 内置代理的接管痕迹识别（礼让判据，避免双代理互相覆盖或链式套娃）；
//! 3. cc-switch 供应商库清洗引擎（`CcSwitchDbCleaner`），在 cc-switch 未运行时把
//!    污染记录还原为 source registry 中保存的真实上游地址。
//!
//! 清洗遵循"不猜上游"原则：source_id 无法在 registry 中解析时不改写记录，只计入
//! `unresolved` 并交由 UI 提示。

use super::types::ClaudeSettings;
use super::url_identity;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::time::Duration;
use toml_edit::{DocumentMut, Item, TableLike, Value};

/// 对外展示的外部管理器名称（事件 payload / 状态字段中使用的稳定标识，非 UI 文案）。
pub const CCSWITCH_MANAGER_NAME: &str = "cc-switch";
/// cc-switch 内置代理写入 live 配置的凭据占位符（其接管状态的唯一强标记）。
pub const CCSWITCH_PROXY_PLACEHOLDER: &str = "PROXY_MANAGED";
/// cc-switch 内置代理的默认监听端口（其 DB 缺失 proxy_config 时的回退值）。
const DEFAULT_CCSWITCH_PROXY_PORT: u16 = 15721;
/// 清洗前的 cc-switch DB 备份保留份数。
const MAX_DB_BACKUPS: usize = 5;
/// 打开 cc-switch DB 时的 busy 等待上限。
const DB_BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

// ---------------------------------------------------------------------------
// 环境检测
// ---------------------------------------------------------------------------

/// cc-switch 安装环境探测。
pub struct CcSwitchEnv {
    dir: PathBuf,
}

impl CcSwitchEnv {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            dir: home.join(".cc-switch"),
        }
    }

    #[cfg(test)]
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn is_installed(&self) -> bool {
        self.dir.is_dir()
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.join("cc-switch.db")
    }

    pub fn db_exists(&self) -> bool {
        self.db_path().is_file()
    }

    /// 检测 cc-switch 进程是否在运行。
    ///
    /// 检测命令执行失败时返回 `true`（fail-safe：宁可推迟清洗，也不与运行中的
    /// cc-switch 并发写它的数据库）。
    pub fn is_running() -> bool {
        detect_ccswitch_process().unwrap_or(true)
    }

    /// 读取 cc-switch 内置代理的监听端口（只读查询其 DB），失败回退默认端口。
    pub fn proxy_listen_port(&self) -> u16 {
        self.read_proxy_listen_port()
            .unwrap_or(DEFAULT_CCSWITCH_PROXY_PORT)
    }

    fn read_proxy_listen_port(&self) -> Option<u16> {
        let db_path = self.db_path();
        if !db_path.is_file() {
            return None;
        }
        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let _ = conn.busy_timeout(DB_BUSY_TIMEOUT);
        conn.query_row(
            "SELECT listen_port FROM proxy_config WHERE app_type = 'claude'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .and_then(|port| u16::try_from(port).ok())
    }
}

impl Default for CcSwitchEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
fn detect_ccswitch_process() -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    for image in ["cc-switch.exe", "CC Switch.exe"] {
        let output = Command::new("tasklist")
            .args(["/NH", "/FI", &format!("IMAGENAME eq {image}")])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("tasklist failed: {e}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
        if stdout.contains(&image.to_lowercase()) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
fn detect_ccswitch_process() -> Result<bool, String> {
    // 打包后的二进制名为 cc-switch；兜底匹配 .app 名（开发/重命名场景）。
    let by_name = Command::new("pgrep")
        .args(["-x", "cc-switch"])
        .output()
        .map_err(|e| format!("pgrep failed: {e}"))?;
    if by_name.status.success() {
        return Ok(true);
    }
    let by_pattern = Command::new("pgrep")
        .args(["-f", "CC Switch.app/Contents/MacOS"])
        .output()
        .map_err(|e| format!("pgrep failed: {e}"))?;
    Ok(by_pattern.status.success())
}

// ---------------------------------------------------------------------------
// 接管痕迹识别（礼让判据）
// ---------------------------------------------------------------------------

/// 判断 Claude live 配置是否处于 cc-switch 内置代理的接管状态。
///
/// 强判据：凭据字段为 cc-switch 的 `PROXY_MANAGED` 占位符；
/// 弱判据：base_url 指向回环地址且端口等于 cc-switch 代理端口，且不是自家代理 URL。
pub fn detect_ccswitch_takeover_claude(
    settings: &ClaudeSettings,
    ccswitch_proxy_port: u16,
) -> bool {
    if settings_contain_proxy_managed_placeholder(settings) {
        return true;
    }

    if let Some(base_url) = settings.get_base_url() {
        if !url_identity::is_usagemeter_proxy_url(&base_url, &["claude-code"])
            && is_loopback_url_with_port(&base_url, ccswitch_proxy_port)
        {
            return true;
        }
    }
    false
}

/// cc-switch 内置代理接管时可能被写成占位符的全部 env 凭据键
/// （cc-switch 会重写这 4 个键中在配置内已存在者）。
const CCSWITCH_PLACEHOLDER_ENV_KEYS: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
];

/// 判断 Claude settings 的任一凭据键是否为 cc-switch 的 `PROXY_MANAGED` 占位符。
pub fn settings_contain_proxy_managed_placeholder(settings: &ClaudeSettings) -> bool {
    CCSWITCH_PLACEHOLDER_ENV_KEYS.iter().any(|key| {
        settings.env.get(*key).and_then(|v| v.as_str()) == Some(CCSWITCH_PROXY_PLACEHOLDER)
    })
}

/// 判断 Codex live 配置是否处于 cc-switch 内置代理的接管状态。
pub fn detect_ccswitch_takeover_codex(
    api_key: Option<&str>,
    config_toml: &str,
    ccswitch_proxy_port: u16,
) -> bool {
    if api_key == Some(CCSWITCH_PROXY_PLACEHOLDER) {
        return true;
    }

    match config_toml.parse::<DocumentMut>() {
        Ok(doc) => {
            if toml_contains_proxy_managed_token(&doc) {
                return true;
            }
            for base_url in collect_codex_base_urls(&doc) {
                if !url_identity::is_usagemeter_proxy_url(&base_url, &["codex"])
                    && is_loopback_url_with_port(&base_url, ccswitch_proxy_port)
                {
                    return true;
                }
            }
            false
        }
        // TOML 解析失败时的保守降级：两个标记同时出现即认为被 cc-switch 接管。
        Err(_) => {
            config_toml.contains("experimental_bearer_token")
                && config_toml.contains(CCSWITCH_PROXY_PLACEHOLDER)
        }
    }
}

fn toml_contains_proxy_managed_token(doc: &DocumentMut) -> bool {
    let matches_placeholder = |table: &dyn TableLike| {
        table
            .get("experimental_bearer_token")
            .and_then(Item::as_str)
            == Some(CCSWITCH_PROXY_PLACEHOLDER)
    };

    if matches_placeholder(doc.as_table()) {
        return true;
    }
    // as_table_like 同时覆盖标准表与 inline table 两种写法。
    if let Some(providers) = doc.get("model_providers").and_then(Item::as_table_like) {
        for (_, item) in providers.iter() {
            if item
                .as_table_like()
                .map(matches_placeholder)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

fn collect_codex_base_urls(doc: &DocumentMut) -> Vec<String> {
    let mut urls = Vec::new();
    for key in ["chatgpt_base_url", "base_url"] {
        if let Some(url) = doc.get(key).and_then(Item::as_str) {
            urls.push(url.to_string());
        }
    }
    if let Some(providers) = doc.get("model_providers").and_then(Item::as_table_like) {
        for (_, item) in providers.iter() {
            if let Some(url) = item
                .as_table_like()
                .and_then(|table| table.get("base_url"))
                .and_then(Item::as_str)
            {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// 判断 base_url 是否指向 cc-switch 内置代理（用于 registry 防御，避免把它注册为上游）。
///
/// 仅在 cc-switch 数据库存在时才做端口匹配，避免误伤用户合法的本地网关上游
/// （如 LiteLLM / ollama 等本地服务；目录残留但 DB 已删同样视为未安装）。
pub fn is_ccswitch_proxy_url(base_url: &str) -> bool {
    let env = CcSwitchEnv::new();
    if !env.db_exists() {
        return false;
    }
    is_loopback_url_with_port(base_url, env.proxy_listen_port())
}

fn is_loopback_url_with_port(base_url: &str, port: u16) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let is_loopback =
        host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]";
    is_loopback && url.port() == Some(port)
}

// ---------------------------------------------------------------------------
// 清洗引擎
// ---------------------------------------------------------------------------

/// Claude 记录的还原目标（来自 `ProxySourceRegistry` 的 handle）。
#[derive(Debug, Clone)]
pub struct ClaudeRestoreTarget {
    pub had_base_url: bool,
    pub original_base_url: Option<String>,
}

/// 单次清洗的结果报告。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    /// 扫描的供应商记录总数。
    pub scanned: usize,
    /// 成功还原的记录数。
    pub cleaned: usize,
    /// 含代理地址但 source_id 无法解析、未改写的记录数。
    pub unresolved: usize,
    /// settings_config 解析失败而跳过的记录数。
    pub skipped_parse_errors: usize,
    /// 写前备份文件路径（本次无写入时为 None）。
    pub backup_path: Option<String>,
    pub cleaned_at_ms: i64,
}

type ClaudeLookup = Box<dyn Fn(&str) -> Option<ClaudeRestoreTarget> + Send + Sync>;
type CodexLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// cc-switch 供应商库清洗引擎。
///
/// 不直接持有 source registry：通过注入的查找闭包解析 source_id → 真实上游，
/// 生产路径注入真实 registry，单测注入假映射。
pub struct CcSwitchDbCleaner {
    db_path: PathBuf,
    backup_dir: PathBuf,
    claude_lookup: ClaudeLookup,
    codex_lookup: CodexLookup,
}

enum RowRewrite {
    Unchanged,
    Cleaned {
        new_config: String,
        unresolved: bool,
    },
    Unresolved,
    ParseError,
}

impl CcSwitchDbCleaner {
    pub fn new(
        db_path: PathBuf,
        backup_dir: PathBuf,
        claude_lookup: ClaudeLookup,
        codex_lookup: CodexLookup,
    ) -> Self {
        Self {
            db_path,
            backup_dir,
            claude_lookup,
            codex_lookup,
        }
    }

    /// 执行一次清洗。幂等：重复执行时已还原的记录不会再被改写。
    pub fn clean(&self) -> Result<CleanReport, String> {
        let mut report = CleanReport {
            cleaned_at_ms: now_ms(),
            ..CleanReport::default()
        };

        if !self.db_path.is_file() {
            return Ok(report);
        }

        // 不带 CREATE 标志：DB 不存在或不可写时直接失败，绝不凭空创建。
        let mut conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("ccswitchDbOpenFailed: {e}"))?;
        conn.busy_timeout(DB_BUSY_TIMEOUT)
            .map_err(|e| format!("ccswitchDbOpenFailed: {e}"))?;

        self.verify_schema(&conn)?;

        // 干跑扫描：先算出全部待写内容，确认确有污染记录才做备份与写事务。
        let mut updates: Vec<(String, String, String)> = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT id, app_type, settings_config FROM providers")
                .map_err(|e| format!("ccswitchDbQueryFailed: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| format!("ccswitchDbQueryFailed: {e}"))?;

            for row in rows {
                let (id, app_type, settings_config) =
                    row.map_err(|e| format!("ccswitchDbQueryFailed: {e}"))?;
                report.scanned += 1;

                // cc-switch 自己的代理占位符体系，原样保留。
                if settings_config.contains(CCSWITCH_PROXY_PLACEHOLDER) {
                    continue;
                }

                let rewrite = match app_type.as_str() {
                    "claude" => rewrite_claude_settings_config(&settings_config, |source_id| {
                        (self.claude_lookup)(source_id)
                    }),
                    "codex" => rewrite_codex_settings_config(&settings_config, |source_id| {
                        (self.codex_lookup)(source_id)
                    }),
                    _ => RowRewrite::Unchanged,
                };

                match rewrite {
                    RowRewrite::Unchanged => {}
                    RowRewrite::Cleaned {
                        new_config,
                        unresolved,
                    } => {
                        report.cleaned += 1;
                        if unresolved {
                            report.unresolved += 1;
                        }
                        updates.push((id, app_type, new_config));
                    }
                    RowRewrite::Unresolved => report.unresolved += 1,
                    RowRewrite::ParseError => report.skipped_parse_errors += 1,
                }
            }
        }

        if updates.is_empty() {
            return Ok(report);
        }

        report.backup_path = Some(self.backup_database(&conn)?);

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("ccswitchDbBusy: {e}"))?;
        for (id, app_type, new_config) in &updates {
            tx.execute(
                "UPDATE providers SET settings_config = ?1 WHERE id = ?2 AND app_type = ?3",
                rusqlite::params![new_config, id, app_type],
            )
            .map_err(|e| format!("ccswitchDbWriteFailed: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("ccswitchDbWriteFailed: {e}"))?;

        Ok(report)
    }

    fn verify_schema(&self, conn: &Connection) -> Result<(), String> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(providers)")
            .map_err(|e| format!("ccswitchSchemaMismatch: {e}"))?;
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("ccswitchSchemaMismatch: {e}"))?
            .filter_map(Result::ok)
            .collect();

        for required in ["id", "app_type", "settings_config"] {
            if !columns.iter().any(|column| column == required) {
                return Err(format!(
                    "ccswitchSchemaMismatch: providers table missing column {required}"
                ));
            }
        }
        Ok(())
    }

    fn backup_database(&self, conn: &Connection) -> Result<String, String> {
        fs::create_dir_all(&self.backup_dir).map_err(|e| format!("ccswitchBackupFailed: {e}"))?;
        // 追加进程内自增序号，避免同毫秒内两次备份撞名。
        let seq = BACKUP_SEQ.fetch_add(1, Ordering::Relaxed);
        let backup_path = self
            .backup_dir
            .join(format!("cc-switch-{}-{seq}.db", now_ms()));
        let backup_str = backup_path.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!("VACUUM INTO '{backup_str}'"))
            .map_err(|e| format!("ccswitchBackupFailed: {e}"))?;
        prune_old_backups(&self.backup_dir);
        Ok(backup_path.to_string_lossy().to_string())
    }
}

static BACKUP_SEQ: AtomicU64 = AtomicU64::new(0);

fn prune_old_backups(backup_dir: &Path) {
    let Ok(entries) = fs::read_dir(backup_dir) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("cc-switch-") && name.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();
    // 文件名内嵌毫秒时间戳，字典序即时间序。
    backups.sort();
    while backups.len() > MAX_DB_BACKUPS {
        let oldest = backups.remove(0);
        let _ = fs::remove_file(oldest);
    }
}

/// 还原 Claude 供应商记录中的 UsageMeter 代理地址。
fn rewrite_claude_settings_config(
    raw: &str,
    lookup: impl Fn(&str) -> Option<ClaudeRestoreTarget>,
) -> RowRewrite {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return RowRewrite::ParseError;
    };
    let Some(base_url) = value
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return RowRewrite::Unchanged;
    };
    if !url_identity::is_usagemeter_proxy_url(&base_url, &["claude-code"]) {
        return RowRewrite::Unchanged;
    }
    let Some(source_id) =
        url_identity::extract_source_id_from_proxy_url(&base_url, &["claude-code"])
    else {
        // 无 source_id 的旧式代理 URL 无从映射真实上游，不猜测。
        return RowRewrite::Unresolved;
    };
    let Some(target) = lookup(&source_id) else {
        return RowRewrite::Unresolved;
    };

    let Some(env) = value.get_mut("env").and_then(|env| env.as_object_mut()) else {
        return RowRewrite::ParseError;
    };
    match (target.had_base_url, target.original_base_url) {
        (true, Some(original)) => {
            env.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                serde_json::Value::String(original),
            );
        }
        _ => {
            env.remove("ANTHROPIC_BASE_URL");
        }
    }

    match serde_json::to_string(&value) {
        Ok(new_config) => RowRewrite::Cleaned {
            new_config,
            unresolved: false,
        },
        Err(_) => RowRewrite::ParseError,
    }
}

/// 还原 Codex 供应商记录（`{"auth": ..., "config": "<TOML 文本>"}`）中的代理地址。
fn rewrite_codex_settings_config(raw: &str, lookup: impl Fn(&str) -> Option<String>) -> RowRewrite {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return RowRewrite::ParseError;
    };
    let Some(config_toml) = value
        .get("config")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return RowRewrite::Unchanged;
    };

    let (new_toml, replaced, unresolved) = match config_toml.parse::<DocumentMut>() {
        Ok(mut doc) => {
            let (replaced, unresolved) = rewrite_codex_toml_urls(&mut doc, &lookup);
            (doc.to_string(), replaced, unresolved)
        }
        // TOML 解析失败的保守降级：只替换以带引号形式精确出现的完整代理 URL。
        Err(_) => rewrite_codex_urls_in_quoted_strings(&config_toml, &lookup),
    };

    if replaced == 0 {
        return if unresolved > 0 {
            RowRewrite::Unresolved
        } else {
            RowRewrite::Unchanged
        };
    }

    value["config"] = serde_json::Value::String(new_toml);
    match serde_json::to_string(&value) {
        Ok(new_config) => RowRewrite::Cleaned {
            new_config,
            unresolved: unresolved > 0,
        },
        Err(_) => RowRewrite::ParseError,
    }
}

/// 遍历 Codex TOML 中所有 base_url 位置，替换 UsageMeter 代理地址。返回 (替换数, 未解析数)。
fn rewrite_codex_toml_urls(
    doc: &mut DocumentMut,
    lookup: &impl Fn(&str) -> Option<String>,
) -> (usize, usize) {
    let mut replaced = 0;
    let mut unresolved = 0;

    let mut resolve = |current: &str| -> Option<String> {
        if !url_identity::is_usagemeter_proxy_url(current, &["codex"]) {
            return None;
        }
        match url_identity::extract_source_id_from_proxy_url(current, &["codex"])
            .and_then(|source_id| lookup(&source_id))
        {
            Some(real) => {
                replaced += 1;
                Some(real)
            }
            None => {
                unresolved += 1;
                None
            }
        }
    };

    for key in ["chatgpt_base_url", "base_url"] {
        if let Some(current) = doc.get(key).and_then(Item::as_str).map(str::to_string) {
            if let Some(real) = resolve(&current) {
                doc[key] = Item::Value(Value::from(real));
            }
        }
    }

    // as_table_like 同时覆盖 [model_providers.x] 与 model_providers = { x = {...} } 写法；
    // Index 写路径对两者同样有效且保留原有风格。
    let provider_ids: Vec<String> = doc
        .get("model_providers")
        .and_then(Item::as_table_like)
        .map(|providers| providers.iter().map(|(key, _)| key.to_string()).collect())
        .unwrap_or_default();
    for provider_id in provider_ids {
        let current = doc
            .get("model_providers")
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(&provider_id))
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("base_url"))
            .and_then(Item::as_str)
            .map(str::to_string);
        if let Some(current) = current {
            if let Some(real) = resolve(&current) {
                doc["model_providers"][&provider_id]["base_url"] = Item::Value(Value::from(real));
            }
        }
    }

    (replaced, unresolved)
}

/// TOML 解析失败时的降级替换：按双引号切分，仅替换引号内完整命中的代理 URL。
fn rewrite_codex_urls_in_quoted_strings(
    config_toml: &str,
    lookup: &impl Fn(&str) -> Option<String>,
) -> (String, usize, usize) {
    let mut replaced = 0;
    let mut unresolved = 0;
    let segments: Vec<String> = config_toml
        .split('"')
        .enumerate()
        .map(|(index, segment)| {
            // 奇数段位于双引号内部。
            if index % 2 == 1 && url_identity::is_usagemeter_proxy_url(segment, &["codex"]) {
                match url_identity::extract_source_id_from_proxy_url(segment, &["codex"])
                    .and_then(|source_id| lookup(&source_id))
                {
                    Some(real) => {
                        replaced += 1;
                        return real;
                    }
                    None => unresolved += 1,
                }
            }
            segment.to_string()
        })
        .collect();
    (segments.join("\""), replaced, unresolved)
}

// ---------------------------------------------------------------------------
// 兼容状态持久化与清洗编排
// ---------------------------------------------------------------------------

/// 持久化在 app_config.db 的兼容层状态。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchCompatState {
    #[serde(default)]
    pub last_clean_at_ms: Option<i64>,
    #[serde(default)]
    pub last_report: Option<CleanReport>,
    /// cc-switch 运行中导致清洗被推迟时置位，等待下次时机自动执行。
    #[serde(default)]
    pub pending_clean: bool,
    /// 最近一次清洗失败的错误码（如 ccswitchSchemaMismatch），成功后清空。
    #[serde(default)]
    pub last_error_code: Option<String>,
}

fn compat_state_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".usagemeter").join("ccswitch_compat_state.json")
}

const RUNTIME_DOCUMENT_KEY: &str = "ccswitch_compat_state";

pub fn read_compat_state() -> CcSwitchCompatState {
    #[cfg(not(test))]
    if let Ok(Some(value)) = crate::app_config::load_runtime_document(RUNTIME_DOCUMENT_KEY) {
        return serde_json::from_value(value).unwrap_or_default();
    }
    let path = compat_state_path();
    if !path.exists() {
        return CcSwitchCompatState::default();
    }
    let state = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    #[cfg(not(test))]
    if let Ok(value) = serde_json::to_value(&state) {
        if crate::app_config::save_runtime_document(RUNTIME_DOCUMENT_KEY, &value).is_ok() {
            let _ = crate::utils::remove_usagemeter_state_file(&path, "ccswitch_compat_state.json");
        }
    }
    state
}

fn write_compat_state(state: &CcSwitchCompatState) {
    #[cfg(not(test))]
    {
        if let Ok(value) = serde_json::to_value(state) {
            let _ = crate::app_config::save_runtime_document(RUNTIME_DOCUMENT_KEY, &value);
        }
        return;
    }

    #[cfg(test)]
    {
        let path = compat_state_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(state) {
            let _ = fs::write(&path, content);
        }
    }
}

fn backup_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".usagemeter").join("ccswitch_backups")
}

/// 构造生产路径的清洗器（注入真实 source registry）。
fn production_cleaner() -> CcSwitchDbCleaner {
    CcSwitchDbCleaner::new(
        CcSwitchEnv::new().db_path(),
        backup_dir(),
        Box::new(|source_id| {
            super::source_registry::ProxySourceRegistry::new()
                .get(source_id)
                .map(|handle| ClaudeRestoreTarget {
                    had_base_url: handle.had_base_url,
                    original_base_url: handle.original_base_url,
                })
        }),
        Box::new(|source_id| {
            super::codex_config::CodexSourceRegistry::new()
                .get(source_id)
                .map(|handle| handle.real_base_url)
        }),
    )
}

/// 清洗互斥标志：启动清洗、代理停止清洗、监控边沿清洗、手动清洗可能并发触发，
/// 同一时刻只允许一个清洗流程持有 cc-switch DB 的写路径。
static CLEAN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// RAII 互斥守卫：Drop 时释放 `CLEAN_IN_FLIGHT`，覆盖所有 early return 路径。
struct CleanFlightGuard;

impl CleanFlightGuard {
    fn try_acquire() -> Option<Self> {
        CLEAN_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| CleanFlightGuard)
    }
}

impl Drop for CleanFlightGuard {
    fn drop(&mut self) {
        CLEAN_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// 判断清洗错误是否为永久性错误（重试也不会成功，不应保持 pending 反复触发）。
fn is_permanent_clean_error(error: &str) -> bool {
    error.starts_with("ccswitchSchemaMismatch")
}

/// 从错误串中提取稳定错误码（冒号前缀部分）。
fn clean_error_code(error: &str) -> String {
    error.split(':').next().unwrap_or(error).trim().to_string()
}

/// 记录一次清洗失败：瞬态错误（busy/打开失败）保持 pending 等待下次时机，
/// 永久性错误（schema 不符）不再置 pending，避免反复空转。
fn record_clean_failure(error: &str) {
    let mut state = read_compat_state();
    state.pending_clean = !is_permanent_clean_error(error);
    state.last_error_code = Some(clean_error_code(error));
    write_compat_state(&state);
}

fn record_clean_success(report: &CleanReport) {
    let mut state = read_compat_state();
    state.pending_clean = false;
    state.last_clean_at_ms = Some(report.cleaned_at_ms);
    state.last_report = Some(report.clone());
    state.last_error_code = None;
    write_compat_state(&state);
}

/// 手动清洗入口（Tauri 命令用）：以错误码区分失败原因，绝不在 cc-switch 运行时强清。
///
/// 同步执行（文件与 SQLite 操作），异步上下文请通过 `spawn_blocking` 调用。
pub fn clean_ccswitch_db_now() -> Result<CleanReport, String> {
    let Some(_guard) = CleanFlightGuard::try_acquire() else {
        return Err("ccswitchCleanBusy".to_string());
    };

    let env = CcSwitchEnv::new();
    if !env.is_installed() || !env.db_exists() {
        return Err("ccswitchNotInstalled".to_string());
    }
    if CcSwitchEnv::is_running() {
        let mut state = read_compat_state();
        state.pending_clean = true;
        write_compat_state(&state);
        return Err("ccswitchRunning".to_string());
    }

    match production_cleaner().clean() {
        Ok(report) => {
            record_clean_success(&report);
            Ok(report)
        }
        Err(e) => {
            record_clean_failure(&e);
            Err(e)
        }
    }
}

/// 尝试清洗 cc-switch 供应商库。
///
/// 守卫条件：cc-switch 已安装、DB 存在、进程未运行、无并发清洗在途；
/// 不满足时按需置 pending 并返回 None。
/// 同步执行（文件与 SQLite 操作），异步上下文请通过 `spawn_blocking` 调用。
pub fn try_clean_ccswitch_db(reason: &str) -> Option<CleanReport> {
    let Some(_guard) = CleanFlightGuard::try_acquire() else {
        eprintln!("[ccswitch-compat] clean skipped (another clean in flight), reason={reason}");
        return None;
    };

    let env = CcSwitchEnv::new();
    if !env.is_installed() || !env.db_exists() {
        return None;
    }

    if CcSwitchEnv::is_running() {
        let mut state = read_compat_state();
        if !state.pending_clean {
            state.pending_clean = true;
            write_compat_state(&state);
        }
        eprintln!("[ccswitch-compat] clean deferred (cc-switch running), reason={reason}");
        return None;
    }

    match production_cleaner().clean() {
        Ok(report) => {
            record_clean_success(&report);
            if report.cleaned > 0 || report.unresolved > 0 {
                eprintln!(
                    "[ccswitch-compat] clean done, reason={reason}, cleaned={}, unresolved={}",
                    report.cleaned, report.unresolved
                );
            }
            Some(report)
        }
        Err(e) => {
            record_clean_failure(&e);
            eprintln!("[ccswitch-compat] clean failed, reason={reason}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// cc-switch 运行状态边沿追踪（供监控循环触发退出清洗）
// ---------------------------------------------------------------------------

const RUNNING_UNKNOWN: u8 = 0;
const RUNNING_YES: u8 = 1;
const RUNNING_NO: u8 = 2;

static LAST_RUNNING_STATE: AtomicU8 = AtomicU8::new(RUNNING_UNKNOWN);

/// 轮询 cc-switch 运行状态，返回 true 表示应触发一次清洗：
/// 检测到 running → not-running 边沿，或存在 pending 清洗且当前未运行。
pub fn poll_should_trigger_clean() -> bool {
    let env = CcSwitchEnv::new();
    if !env.is_installed() {
        return false;
    }

    let running = CcSwitchEnv::is_running();
    let current = if running { RUNNING_YES } else { RUNNING_NO };
    let previous = LAST_RUNNING_STATE.swap(current, Ordering::Relaxed);

    if running {
        return false;
    }
    previous == RUNNING_YES || read_compat_state().pending_clean
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("usagemeter_ccswitch_{label}_{nanos}"))
    }

    fn create_fake_ccswitch_db(dir: &Path, rows: &[(&str, &str, &str)]) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let db_path = dir.join("cc-switch.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (
                id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                settings_config TEXT NOT NULL,
                is_current INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (id, app_type)
            );",
        )
        .unwrap();
        for (id, app_type, settings_config) in rows {
            conn.execute(
                "INSERT INTO providers (id, app_type, settings_config) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, app_type, settings_config],
            )
            .unwrap();
        }
        db_path
    }

    fn read_settings_config(db_path: &Path, id: &str, app_type: &str) -> String {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT settings_config FROM providers WHERE id = ?1 AND app_type = ?2",
            rusqlite::params![id, app_type],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn cleaner_with_lookups(db_path: PathBuf, backup_dir: PathBuf) -> CcSwitchDbCleaner {
        CcSwitchDbCleaner::new(
            db_path,
            backup_dir,
            Box::new(|source_id| match source_id {
                "h_known" => Some(ClaudeRestoreTarget {
                    had_base_url: true,
                    original_base_url: Some("https://api.example.com".to_string()),
                }),
                "h_nobase" => Some(ClaudeRestoreTarget {
                    had_base_url: false,
                    original_base_url: None,
                }),
                _ => None,
            }),
            Box::new(|source_id| match source_id {
                "h_codex" => Some("https://codex.example.com/v1".to_string()),
                _ => None,
            }),
        )
    }

    #[test]
    fn cleans_polluted_claude_record() {
        let root = unique_test_dir("claude_clean");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/usagemeter/claude-code/source/h_known",
                "ANTHROPIC_AUTH_TOKEN": "sk-real-token"
            },
            "permissions": {"allow": []}
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let cleaner = cleaner_with_lookups(db_path.clone(), root.join("backups"));
        let report = cleaner.clean().unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.cleaned, 1);
        assert_eq!(report.unresolved, 0);
        assert!(report.backup_path.is_some());

        let cleaned: serde_json::Value =
            serde_json::from_str(&read_settings_config(&db_path, "p1", "claude")).unwrap();
        assert_eq!(
            cleaned["env"]["ANTHROPIC_BASE_URL"].as_str(),
            Some("https://api.example.com")
        );
        // 其余字段保留
        assert_eq!(
            cleaned["env"]["ANTHROPIC_AUTH_TOKEN"].as_str(),
            Some("sk-real-token")
        );
        assert!(cleaned["permissions"].is_object());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn removes_base_url_when_source_had_none() {
        let root = unique_test_dir("claude_nobase");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/usagemeter/claude-code/source/h_nobase",
                "ANTHROPIC_API_KEY": "sk-key"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 1);

        let cleaned: serde_json::Value =
            serde_json::from_str(&read_settings_config(&db_path, "p1", "claude")).unwrap();
        assert!(cleaned["env"].get("ANTHROPIC_BASE_URL").is_none());
        assert_eq!(cleaned["env"]["ANTHROPIC_API_KEY"].as_str(), Some("sk-key"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleans_codex_record_preserving_toml_comments() {
        let root = unique_test_dir("codex_clean");
        let toml = "# managed by user\nmodel_provider = \"custom\"\n\n[model_providers.custom]\n# provider endpoint\nbase_url = \"http://127.0.0.1:18765/usagemeter/codex/source/h_codex/v1\"\n";
        let config = serde_json::json!({
            "auth": {"OPENAI_API_KEY": "sk-codex"},
            "config": toml
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "codex", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 1);

        let cleaned: serde_json::Value =
            serde_json::from_str(&read_settings_config(&db_path, "p1", "codex")).unwrap();
        let cleaned_toml = cleaned["config"].as_str().unwrap();
        assert!(cleaned_toml.contains("https://codex.example.com/v1"));
        assert!(cleaned_toml.contains("# managed by user"));
        assert!(cleaned_toml.contains("# provider endpoint"));
        assert_eq!(cleaned["auth"]["OPENAI_API_KEY"].as_str(), Some("sk-codex"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleans_codex_inline_table_record_preserving_inline_style() {
        let root = unique_test_dir("codex_inline");
        let toml = "model_provider = \"custom\"\nmodel_providers = { custom = { base_url = \"http://127.0.0.1:18765/usagemeter/codex/source/h_codex/v1\" } }\n";
        let config = serde_json::json!({
            "auth": {"OPENAI_API_KEY": "sk-codex"},
            "config": toml
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "codex", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 1);
        assert_eq!(report.unresolved, 0);

        let cleaned: serde_json::Value =
            serde_json::from_str(&read_settings_config(&db_path, "p1", "codex")).unwrap();
        let cleaned_toml = cleaned["config"].as_str().unwrap();
        assert!(cleaned_toml.contains("https://codex.example.com/v1"));
        assert!(!cleaned_toml.contains("127.0.0.1:18765"));
        // inline table 风格保留，不被展开为 [model_providers.custom] 标准表
        assert!(cleaned_toml.contains("model_providers = {"));
        assert!(!cleaned_toml.contains("[model_providers"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detects_codex_takeover_in_inline_table() {
        let inline_token = "model_providers = { x = { base_url = \"https://api.example.com/v1\", experimental_bearer_token = \"PROXY_MANAGED\" } }\n";
        assert!(detect_ccswitch_takeover_codex(None, inline_token, 15721));

        let inline_port =
            "model_providers = { x = { base_url = \"http://127.0.0.1:15721/v1\" } }\n";
        assert!(detect_ccswitch_takeover_codex(None, inline_port, 15721));

        let inline_clean =
            "model_providers = { x = { base_url = \"https://api.example.com/v1\" } }\n";
        assert!(!detect_ccswitch_takeover_codex(None, inline_clean, 15721));
    }

    #[test]
    fn skips_proxy_managed_records() {
        let root = unique_test_dir("proxy_managed");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
                "ANTHROPIC_API_KEY": "PROXY_MANAGED"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 0);
        assert_eq!(read_settings_config(&db_path, "p1", "claude"), config);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_source_id_is_unresolved_and_untouched() {
        let root = unique_test_dir("unresolved");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/usagemeter/claude-code/source/h_missing"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 0);
        assert_eq!(report.unresolved, 1);
        assert!(report.backup_path.is_none());
        assert_eq!(read_settings_config(&db_path, "p1", "claude"), config);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_json_row_is_skipped() {
        let root = unique_test_dir("corrupt");
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", "{not-json")]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.skipped_parse_errors, 1);
        assert_eq!(report.cleaned, 0);
        assert_eq!(read_settings_config(&db_path, "p1", "claude"), "{not-json");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_required_column_aborts_clean() {
        let root = unique_test_dir("schema");
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("cc-switch.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE providers (id TEXT, app_type TEXT);")
            .unwrap();
        drop(conn);

        let err = cleaner_with_lookups(db_path, root.join("backups"))
            .clean()
            .unwrap_err();
        assert!(err.contains("ccswitchSchemaMismatch"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_proxy_url_is_recognized() {
        let root = unique_test_dir("legacy_url");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/claude-code/source/h_known"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let report = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(report.cleaned, 1);

        let cleaned: serde_json::Value =
            serde_json::from_str(&read_settings_config(&db_path, "p1", "claude")).unwrap();
        assert_eq!(
            cleaned["env"]["ANTHROPIC_BASE_URL"].as_str(),
            Some("https://api.example.com")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clean_is_idempotent() {
        let root = unique_test_dir("idempotent");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/usagemeter/claude-code/source/h_known"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);

        let first = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(first.cleaned, 1);

        let second = cleaner_with_lookups(db_path.clone(), root.join("backups"))
            .clean()
            .unwrap();
        assert_eq!(second.cleaned, 0);
        assert!(second.backup_path.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backup_file_is_created_and_pruned() {
        let root = unique_test_dir("backup");
        let config = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765/usagemeter/claude-code/source/h_known"
            }
        })
        .to_string();
        let db_path = create_fake_ccswitch_db(&root, &[("p1", "claude", &config)]);
        let backup_dir = root.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();
        // 预置超过上限的旧备份
        for i in 0..(MAX_DB_BACKUPS + 2) {
            fs::write(backup_dir.join(format!("cc-switch-{i:03}.db")), b"old").unwrap();
        }

        let report = cleaner_with_lookups(db_path, backup_dir.clone())
            .clean()
            .unwrap();
        let backup_path = PathBuf::from(report.backup_path.unwrap());
        assert!(backup_path.exists());
        // 备份是有效 SQLite 库
        let backup_conn = Connection::open(&backup_path).unwrap();
        let count: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let remaining = fs::read_dir(&backup_dir).unwrap().count();
        assert!(remaining <= MAX_DB_BACKUPS);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detects_claude_takeover_by_placeholder_and_port() {
        // cc-switch 会重写 4 个凭据键中已存在者，任一为占位符都应识别
        for key in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENROUTER_API_KEY",
            "OPENAI_API_KEY",
        ] {
            let mut settings = ClaudeSettings::default();
            settings.env.insert(
                key.to_string(),
                serde_json::Value::String("PROXY_MANAGED".to_string()),
            );
            assert!(
                detect_ccswitch_takeover_claude(&settings, 15721),
                "placeholder in {key} should be detected"
            );
        }

        let mut settings = ClaudeSettings::default();
        settings.set_base_url("http://127.0.0.1:15721");
        assert!(detect_ccswitch_takeover_claude(&settings, 15721));

        // 自家代理 URL 不误判
        let mut settings = ClaudeSettings::default();
        settings.set_base_url("http://127.0.0.1:18765/usagemeter/claude-code/source/h_1");
        assert!(!detect_ccswitch_takeover_claude(&settings, 15721));

        // 真实第三方地址不误判
        let mut settings = ClaudeSettings::default();
        settings.set_base_url("https://api.example.com");
        settings.env.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            serde_json::Value::String("sk-real".to_string()),
        );
        assert!(!detect_ccswitch_takeover_claude(&settings, 15721));

        // 其他端口的本地服务（如 ollama）不误判
        let mut settings = ClaudeSettings::default();
        settings.set_base_url("http://127.0.0.1:11434");
        assert!(!detect_ccswitch_takeover_claude(&settings, 15721));
    }

    #[test]
    fn detects_codex_takeover_markers() {
        assert!(detect_ccswitch_takeover_codex(
            Some("PROXY_MANAGED"),
            "",
            15721
        ));

        let toml = "model_provider = \"x\"\n[model_providers.x]\nbase_url = \"http://127.0.0.1:15721/v1\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n";
        assert!(detect_ccswitch_takeover_codex(None, toml, 15721));

        let toml_port_only = "base_url = \"http://127.0.0.1:15721/v1\"\n";
        assert!(detect_ccswitch_takeover_codex(None, toml_port_only, 15721));

        // 自家代理 URL 与真实地址不误判
        let own = "base_url = \"http://127.0.0.1:18765/usagemeter/codex/source/h_1/v1\"\n";
        assert!(!detect_ccswitch_takeover_codex(None, own, 15721));
        let real = "base_url = \"https://api.example.com/v1\"\n";
        assert!(!detect_ccswitch_takeover_codex(
            Some("sk-real"),
            real,
            15721
        ));
    }

    #[test]
    fn codex_fallback_quoted_replacement_only_hits_exact_urls() {
        let broken_toml = "this is [not valid toml\nbase_url = \"http://127.0.0.1:18765/usagemeter/codex/source/h_codex/v1\"\n# comment mentions http://127.0.0.1:18765/usagemeter/codex/source/h_codex/v1 unquoted\n";
        let (result, replaced, unresolved) =
            rewrite_codex_urls_in_quoted_strings(broken_toml, &|source_id| {
                (source_id == "h_codex").then(|| "https://codex.example.com/v1".to_string())
            });
        assert_eq!(replaced, 1);
        assert_eq!(unresolved, 0);
        assert!(result.contains("base_url = \"https://codex.example.com/v1\""));
        // 注释中未加引号的 URL 不受影响
        assert!(result.contains("# comment mentions http://127.0.0.1:18765"));
    }

    #[test]
    fn clean_error_classification() {
        assert!(is_permanent_clean_error(
            "ccswitchSchemaMismatch: providers table missing column id"
        ));
        assert!(!is_permanent_clean_error("ccswitchDbBusy: locked"));
        assert!(!is_permanent_clean_error("ccswitchDbOpenFailed: denied"));
        assert_eq!(
            clean_error_code("ccswitchSchemaMismatch: providers table missing column id"),
            "ccswitchSchemaMismatch"
        );
        assert_eq!(clean_error_code("ccswitchDbBusy"), "ccswitchDbBusy");
    }

    #[test]
    fn clean_flight_guard_is_exclusive_and_released_on_drop() {
        let first = CleanFlightGuard::try_acquire().expect("first acquire should succeed");
        assert!(CleanFlightGuard::try_acquire().is_none());
        drop(first);
        let second = CleanFlightGuard::try_acquire().expect("guard should release on drop");
        drop(second);
    }

    #[test]
    fn env_detection_uses_directory_presence() {
        let root = unique_test_dir("env");
        let env = CcSwitchEnv::with_dir(root.join(".cc-switch"));
        assert!(!env.is_installed());
        assert!(!env.db_exists());
        fs::create_dir_all(root.join(".cc-switch")).unwrap();
        assert!(env.is_installed());
        assert_eq!(env.proxy_listen_port(), DEFAULT_CCSWITCH_PROXY_PORT);

        let _ = fs::remove_dir_all(root);
    }
}
