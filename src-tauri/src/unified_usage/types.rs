use std::collections::HashMap;

use crate::models::SourceFilter;
use crate::proxy::UsageRecord;
use crate::session::constants::{
    TOOL_CLAUDE_CODE, TOOL_CODEX, TOOL_COPILOT, TOOL_GEMINI, TOOL_HERMES, TOOL_OPENCLAW,
    TOOL_OPENCODE, TOOL_QODER_CLI, TOOL_QODER_IDE, TOOL_QODER_IDE_CN, TOOL_QODER_WORK,
    TOOL_QODER_WORK_CN, TOOL_REASONIX,
};
use crate::session::{LocalRequestRecord, SessionMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageOrigin {
    ProxyOnly,
    LocalOnly,
    MergedProxyPreferred,
    /// Codex-only: local record synthesizes a fake message_id (codex_reader.rs), so it can
    /// never exact-key-match its real proxy counterpart. Reconciled via a bounded fuzzy match
    /// (same session/model/total_tokens, close timestamp) instead. Kept distinct from
    /// `MergedProxyPreferred` purely for observability — same field-merge semantics otherwise.
    MergedFuzzyMatched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionMethod {
    Unattributed,
    ConfigInferred,
    Manual,
}

impl AttributionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unattributed => "unattributed",
            Self::ConfigInferred => "config_inferred",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MergedCoverage {
    pub proxy_backed_requests: u64,
    pub local_only_requests: u64,
    pub merged_overlap_requests: u64,
    pub has_partial_status_coverage: bool,
    pub has_partial_performance_coverage: bool,
}

#[derive(Debug, Clone)]
pub struct MergedRequestFact {
    pub canonical_request_key: String,
    pub session_id: String,
    pub project_name: Option<String>,
    pub project_path: Option<String>,
    pub api_key_prefix: Option<String>,
    pub request_base_url: Option<String>,
    pub tool: String,
    pub timestamp_sec: i64,
    pub timestamp_ms: i64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub estimated_cost: f64,
    /// 该事实的用量是否为本地来源标记的估算值（Reasonix telemetry `estimated`）。
    /// 估算数据来自请求中断/失败时的推算，展示时应与真实 API 用量区分。
    pub estimated: bool,
    pub coverage_origin: CoverageOrigin,
    pub status_code: Option<u16>,
    pub duration_ms: Option<u64>,
    pub output_tokens_per_second: Option<f64>,
    pub ttft_ms: Option<u64>,
    /// 用于「按来源分桶」展示的标签：
    /// - 优先取 `api_key_prefix`（最具区分度）
    /// - 否则取 `request_base_url`
    /// - 两者都没有 → None，表示「未识别来源」（仅本地补全的请求会出现这种情况）
    ///
    /// 该字段当前由后端写入、前端展示时消费；编译期标注 dead_code 是为了
    /// 在前端 UI 阶段（阶段 3）尚未接入时不报警。
    #[allow(dead_code)]
    pub source_label: Option<String>,
    /// Stable source identity and evidence level. These are resolved after raw facts merge so
    /// local scanner facts stay immutable and can be safely recomputed.
    pub attribution_source_id: Option<String>,
    pub attribution_method: AttributionMethod,
}

pub(crate) fn has_partial_coverage(proxy_backed_requests: u64, local_only_requests: u64) -> bool {
    proxy_backed_requests > 0 && local_only_requests > 0
}

pub(crate) fn canonical_request_key_for_local(record: &LocalRequestRecord) -> String {
    if let Some(key) = record.request_key.as_ref() {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            record.tool,
            record.session_id,
            record.timestamp,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            record.total_tokens
        )
    } else {
        // 判定与拼接使用同一 trim 后值：真实 message_id 带首尾空白时，两侧键仍对齐。
        format!("{}:{}", record.tool, record.message_id.trim())
    }
}

/// 网关（api_gateway）记录在转发器无法从上游响应提取真实 id 时合成的 fallback
/// message_id 标记（如 `claude_usage_missing_{ts}_{status}`、`gemini_usage_missing_*`、
/// `codex_usage_missing_*`、`anthropic_usage_missing_*`、`codex_{ts}_{n}`）。
/// 这类 id 与本地扫描的 message_id 不匹配同一物理请求，绝不能参与 gateway → 本地键的
/// 归一化合并。
const GATEWAY_FALLBACK_ID_MARKERS: [&str; 4] = [
    "usage_missing",
    "fallback",
    "missing",
    // OpenAI 转发器 `next_openai_fallback_message_id` 生成的 `codex_{ts}_{n}`
    // 不含上述任何子串；本地 Codex 扫描的 message_id 是 `codex:{session}:{index}`
    // （冒号分隔），因此 `codex_` 下划线前缀不会误伤真实 id。
    "codex_",
];

/// 判断网关记录 message_id 是否为 fallback 合成 id（含 `usage_missing` / `fallback` /
/// `missing` 子串）。命中 → true（不允许参与归一化合并）。
fn is_gateway_fallback_message_id(message_id: &str) -> bool {
    let lower = message_id.to_ascii_lowercase();
    GATEWAY_FALLBACK_ID_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// 将网关调用者 label（`X-UsageMeter-Client` 头、profile client_label 或 managed-key
/// remark）归一化为本地扫描工具白名单（session/constants.rs）内的 tool_id。
///
/// 归一化策略：label 转小写、去除非字母数字字符后，与白名单工具 id 的紧凑形式比对，
/// 并兼容少量常见展示名变体（如 "Claude"、"Codex CLI"、"Gemini CLI"、"Qoder"、
/// "GitHub Copilot"）。label 是用户自定义展示名，可能匹配不到任何白名单工具
/// （例如 Cursor）——此时返回 None，调用方回退网关孤立键，避免误合并。
fn normalize_gateway_caller_label(label: &str) -> Option<&'static str> {
    let compact: String = label
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    match compact.as_str() {
        "claudecode" | "claude" => Some(TOOL_CLAUDE_CODE),
        "codex" | "codexcli" => Some(TOOL_CODEX),
        "openclaw" => Some(TOOL_OPENCLAW),
        "opencode" | "opencodeai" => Some(TOOL_OPENCODE),
        "reasonix" => Some(TOOL_REASONIX),
        "gemini" | "geminicli" => Some(TOOL_GEMINI),
        "hermes" => Some(TOOL_HERMES),
        "qoder" | "qoderide" => Some(TOOL_QODER_IDE),
        "qoderidecn" => Some(TOOL_QODER_IDE_CN),
        "qodercli" => Some(TOOL_QODER_CLI),
        "qoderwork" => Some(TOOL_QODER_WORK),
        "qoderworkcn" => Some(TOOL_QODER_WORK_CN),
        "copilot" | "githubcopilot" => Some(TOOL_COPILOT),
        _ => None,
    }
}

/// 网关记录在 message_id 可靠（非空、非 fallback 合成 id）且 caller label 命中工具
/// 白名单时，返回其对应的真实工具 tool_id；否则返回 None（保持 api_gateway 孤立显示）。
/// 判定条件与 canonical_request_key_for_proxy 的归一化完全一致，供 from_proxy 把
/// proxy-only 网关 fact 归因到真实工具，避免同一请求的键前缀与 tool 字段分裂。
fn normalized_gateway_tool(record: &UsageRecord) -> Option<&'static str> {
    if record.ingress_kind != "gateway" {
        return None;
    }
    let message_id = record.message_id.trim();
    if message_id.is_empty() || is_gateway_fallback_message_id(message_id) {
        return None;
    }
    record
        .gateway_caller_label
        .as_deref()
        .and_then(normalize_gateway_caller_label)
}

/// 判断代理记录是否属于 Codex。本地代理（接管）记录的 client_tool 直接是 codex；
/// 网关记录（client_tool = api_gateway）在 message_id 为真实上游 id 且 caller label
/// 归一化为 codex 时同样属于 Codex。网关 Codex 记录的 message_id（resp_*/chatcmpl-*）
/// 与本地 Codex 扫描的合成 id（codex:{session}:{index}）结构不同，精确键永不相等，
/// 必须进入 fuzzy 二次匹配池才能与本地记录对账，否则同一请求仍会双计。
fn is_codex_proxy_record(record: &UsageRecord) -> bool {
    record.client_tool == "codex" || normalized_gateway_tool(record) == Some(TOOL_CODEX)
}

pub(crate) fn canonical_request_key_for_proxy(record: &UsageRecord) -> String {
    // 网关记录（ingress_kind == "gateway"）的存储层键固定为
    // `gateway:{profile_id}:{gateway_request_id}`（见 proxy/database/session.rs 与
    // ingest.rs），与本地扫描键 `{tool}:{message_id}` 永不相等，导致同一物理请求在
    // unified_usage 合并时生成两条 fact，token/费用双计。
    //
    // 网关记录的 message_id 来自上游真实响应 id（OpenAI chatcmpl-*/resp_*、
    // Anthropic msg_*、Gemini responseId），与 Claude Code 本地扫描的 message_id
    // 同源相等（match_support.rs 的 attach_proxy_session_ids 正是用本地
    // message_id → session_id 回填网关记录的 session_id）。因此当 message_id 可靠
    // （非空、非 fallback 合成 id）且 caller label 能归一化为白名单 tool_id 时，
    // 直接返回 `{tool_id}:{message_id}` 对齐本地键，触发 merge_engine 的
    // proxy_preferred 合并分支。
    //
    // 归一化失败 / message_id 为空 / fallback id → 回退现有逻辑（存储层
    // canonical_request_key 优先），保持 api_gateway 孤立显示，避免误合并。
    if let Some(tool_id) = normalized_gateway_tool(record) {
        // 拼接使用与判定同一的 trim 后 message_id，避免两侧不一致。
        let message_id = record.message_id.trim();
        return format!("{tool_id}:{message_id}");
    }
    if let Some(key) = record.canonical_request_key.as_ref() {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            record.client_tool,
            record.session_id.clone().unwrap_or_default(),
            record.timestamp / 1000,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            record.total_tokens
        )
    } else {
        format!("{}:{}", record.client_tool, record.message_id.trim())
    }
}

/// Codex local JSONL scanning fabricates a per-request message_id (see codex_reader.rs), so
/// exact canonical-key matching against the proxy's real API response id is structurally
/// impossible. This bounds the time window of the second-chance fuzzy reconciliation pass:
///   - local timestamps are whole-second-truncated (session/shared.rs::extract_timestamp)
///     and come from the JSONL token_count event
///   - proxy timestamps are captured at response-completion time
///
/// Both mark the same real request, but session-file flush latency, retries, and slow
/// streaming responses routinely space the two out by far more than a few seconds. The
/// original 5s window under-matched badly for exactly this reason. cc-switch, which solved the
/// identical local-vs-proxy Codex reconciliation, uses a 10-minute window in production; we
/// match it. The strong per-field token fingerprint in `codex_fuzzy_candidate_matches` (in
/// particular the large, request-specific cache_read count) keeps accidental collisions inside
/// this wider window negligible for the normal single-active-process case.
pub(crate) const CODEX_FUZZY_MATCH_TOLERANCE_SECS: i64 = 10 * 60;

/// Namespacing prefix the local scanner puts on every Codex session id (see
/// codex_reader.rs::collect_codex_session_files) to keep it globally unique across tools. The
/// proxy only ever sees the bare id Codex CLI sends, so any lookup keyed by session id that
/// needs to bridge the two sides — fuzzy matching, session-meta lookup for proxy-only facts —
/// must add or strip this prefix explicitly rather than compare/format the literal inline.
pub(crate) const CODEX_SESSION_ID_PREFIX: &str = "codex::";

/// Builds the key to look up a proxy-only fact's session metadata in a map indexed by the
/// *local* reader's session id (e.g. `session_meta_by_id`). Only Codex needs translation today:
/// its local session id is namespaced with `CODEX_SESSION_ID_PREFIX`, while the proxy only ever
/// captures the bare id Codex CLI sends — without this, a genuinely proxy-only Codex fact (no
/// local counterpart in range) silently loses its project attribution even when the matching
/// local session metadata exists.
pub(crate) fn session_meta_lookup_key_for_proxy(
    client_tool: &str,
    proxy_session_id: &str,
) -> String {
    if client_tool == "codex" {
        format!("{CODEX_SESSION_ID_PREFIX}{proxy_session_id}")
    } else {
        proxy_session_id.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexFuzzyOutcome {
    /// Local record fuzzy-matched a proxy record that's actually visible (counted) in this
    /// query. Both should be consumed and replaced by one proxy-preferred merged fact.
    MatchedVisible {
        local_key: String,
        proxy_key: String,
    },
    /// Local record's only candidate lives in the unfiltered-but-excluded pool (e.g. an
    /// errored response filtered out of `proxy_index`). Mirrors the existing exact-key
    /// `all_proxy_index.contains_key(&key) => continue` guard: the real request is already
    /// accounted for (or intentionally hidden) elsewhere, so the local record must not
    /// surface as a separate `from_local` fact either.
    SuppressedByFilteredProxy { local_key: String },
}

/// Pure pool-partitioning helper: given the exact-match indexes already built by
/// `merge_realtime_range`, extract the Codex-only orphan subsets the fuzzy pass operates on.
/// No I/O — safe to unit test with hand-built maps. Non-Codex tools (in particular Claude
/// Code, whose local reader sets a real message id and matches exactly today) are untouched.
///
/// The proxy-side pool includes gateway records whose caller label normalizes to Codex
/// (see `is_codex_proxy_record`): their real upstream message id can never equal the local
/// synthetic id, so they need the same fuzzy second-chance reconciliation as direct proxy
/// Codex records to avoid double-counting.
pub(crate) fn codex_orphan_pools<'a>(
    local_index: &'a HashMap<String, LocalRequestRecord>,
    proxy_index: &'a HashMap<String, UsageRecord>,
    all_proxy_index: &'a HashMap<String, UsageRecord>,
) -> (
    Vec<&'a LocalRequestRecord>,
    Vec<&'a UsageRecord>,
    Vec<&'a UsageRecord>,
) {
    let mut local_orphans: Vec<&LocalRequestRecord> = local_index
        .iter()
        .filter(|(key, rec)| rec.tool == "codex" && !proxy_index.contains_key(*key))
        .map(|(_, rec)| rec)
        .collect();

    let mut proxy_orphans_visible: Vec<&UsageRecord> = proxy_index
        .iter()
        .filter(|(key, rec)| is_codex_proxy_record(rec) && !local_index.contains_key(*key))
        .map(|(_, rec)| rec)
        .collect();

    // Records only present in the unfiltered index (filtered out by status/source, or from a
    // different range) — used only as a second-chance suppression signal, never to produce a
    // real match.
    let mut proxy_orphans_all_extra: Vec<&UsageRecord> = all_proxy_index
        .iter()
        .filter(|(key, rec)| {
            is_codex_proxy_record(rec)
                && !local_index.contains_key(*key)
                && !proxy_index.contains_key(*key)
        })
        .map(|(_, rec)| rec)
        .collect();

    // HashMap iteration order is not stable — sort so the greedy algorithm below (and any
    // test asserting on it) is deterministic.
    local_orphans.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });
    proxy_orphans_visible.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });
    proxy_orphans_all_extra.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    (
        local_orphans,
        proxy_orphans_visible,
        proxy_orphans_all_extra,
    )
}

/// Per-field token fingerprint reconciling a local Codex record with its proxy counterpart,
/// modeled on cc-switch's production-proven dedup key (`has_matching_proxy_usage_log`).
///
/// session_id is deliberately NOT part of the fingerprint: real Codex CLI requests carry no
/// session/conversation id the proxy can observe (confirmed against production proxy_data.db —
/// `session_id` is empty for every captured Codex record), so requiring it made the match fail
/// for every request and silently double-count.
///
/// All of the following must hold:
///   - input / output / cache_read tokens: exact per-field equality. This is strictly stronger
///     than the previous single `total_tokens == total_tokens` check — two different requests
///     can share a total while differing in breakdown — and, crucially, it isolates the one
///     component the two sides genuinely can't agree on (cache_create, below) instead of
///     folding it into a total that then never matches.
///   - cache_create tokens: "unknown passthrough". Codex's JSONL token_count events don't
///     expose a cache-creation figure, so the local side is effectively always 0; when it is,
///     accept any proxy value rather than forcing `0 == proxy.cache_create`.
///   - model: case-insensitive equality, with either side empty or "unknown" accepted — the
///     proxy sometimes only learns the model from the response and may leave it blank or
///     normalize it differently than the local reader.
///   - timestamp: within CODEX_FUZZY_MATCH_TOLERANCE_SECS.
fn codex_fuzzy_candidate_matches(local: &LocalRequestRecord, proxy: &UsageRecord) -> bool {
    let model_matches = {
        let local_model = local.model.trim();
        let proxy_model = proxy.model.trim();
        local_model.eq_ignore_ascii_case(proxy_model)
            || local_model.is_empty()
            || proxy_model.is_empty()
            || local_model.eq_ignore_ascii_case("unknown")
            || proxy_model.eq_ignore_ascii_case("unknown")
    };
    // Local (session-log) side is the "unknown" one for cache_create: when it's 0 we can't
    // distinguish "genuinely zero" from "not reported", so we don't let it veto the match.
    let cache_create_matches =
        local.cache_create_tokens == 0 || local.cache_create_tokens == proxy.cache_create_tokens;

    model_matches
        && local.input_tokens == proxy.input_tokens
        && local.output_tokens == proxy.output_tokens
        && local.cache_read_tokens == proxy.cache_read_tokens
        && cache_create_matches
        && (proxy.timestamp / 1000 - local.timestamp).abs() <= CODEX_FUZZY_MATCH_TOLERANCE_SECS
}

/// Second-chance fuzzy match for orphaned Codex records. Greedy, ascending-by-time: process
/// local orphans oldest-first, and for each pick the nearest *unclaimed* visible-pool
/// candidate within tolerance, so no proxy record is ever consumed by more than one local
/// record.
pub(crate) fn find_codex_fuzzy_matches(
    local_orphans: &[&LocalRequestRecord],
    proxy_orphans_visible: &[&UsageRecord],
    proxy_orphans_all_extra: &[&UsageRecord],
) -> Vec<CodexFuzzyOutcome> {
    // P0 优化：按 session_id 分桶 + 时间戳排序，将 O(N×M) 降至 O(N log M)
    find_codex_fuzzy_matches_optimized(
        local_orphans,
        proxy_orphans_visible,
        proxy_orphans_all_extra,
    )
}

/// 优化版本：全局按时间戳排序，再对每条 Local 记录二分定位时间窗口。
///
/// Codex Proxy 请求通常拿不到 CLI session_id，且 Local session_id 带有 `codex::`
/// 命名空间，因此不能按 session_id 分桶，否则生产数据会落入不同桶而完全失配。
/// 排序后二分将全表扫描缩小为 O((N + M) log M + K)，K 为窗口内候选数。
fn find_codex_fuzzy_matches_optimized(
    local_orphans: &[&LocalRequestRecord],
    proxy_orphans_visible: &[&UsageRecord],
    proxy_orphans_all_extra: &[&UsageRecord],
) -> Vec<CodexFuzzyOutcome> {
    let mut local_records = local_orphans.to_vec();
    local_records.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    let mut visible_candidates: Vec<(usize, &UsageRecord)> =
        proxy_orphans_visible.iter().copied().enumerate().collect();
    visible_candidates.sort_by(|(_, a), (_, b)| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    let mut all_candidates: Vec<(usize, &UsageRecord)> = proxy_orphans_all_extra
        .iter()
        .copied()
        .enumerate()
        .collect();
    all_candidates.sort_by(|(_, a), (_, b)| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.message_id.cmp(&b.message_id))
    });

    let mut used_visible = vec![false; proxy_orphans_visible.len()];
    let mut used_all = vec![false; proxy_orphans_all_extra.len()];
    let mut outcomes = Vec::new();

    for local in local_records {
        if let Some((idx, proxy)) =
            find_best_match_in_window(local, &visible_candidates, &used_visible)
        {
            used_visible[idx] = true;
            outcomes.push(CodexFuzzyOutcome::MatchedVisible {
                local_key: canonical_request_key_for_local(local),
                proxy_key: canonical_request_key_for_proxy(proxy),
            });
            continue;
        }

        if let Some((idx, _)) = find_best_match_in_window(local, &all_candidates, &used_all) {
            used_all[idx] = true;
            outcomes.push(CodexFuzzyOutcome::SuppressedByFilteredProxy {
                local_key: canonical_request_key_for_local(local),
            });
        }
    }

    outcomes
}

/// 在已排序的候选列表中使用滑动窗口查找最佳匹配
/// 利用时间戳排序特性，只检查时间窗口内的候选（避免全量遍历）
fn find_best_match_in_window<'a>(
    local: &LocalRequestRecord,
    candidates: &[(usize, &'a UsageRecord)],
    used: &[bool],
) -> Option<(usize, &'a UsageRecord)> {
    let local_ts = local.timestamp;
    let tolerance = CODEX_FUZZY_MATCH_TOLERANCE_SECS;

    // 二分查找找到时间窗口的起点（local_ts - tolerance）
    let window_start =
        candidates.partition_point(|(_, p)| p.timestamp / 1000 < local_ts - tolerance);

    // 从窗口起点开始，只遍历时间窗口内的候选
    let mut best_match: Option<(usize, &UsageRecord, i64)> = None;

    for &(idx, proxy) in candidates.iter().skip(window_start) {
        let proxy_ts = proxy.timestamp / 1000;

        // 超出时间窗口，提前终止（因为已排序）
        if proxy_ts > local_ts + tolerance {
            break;
        }

        // 跳过已使用的候选
        if used[idx] {
            continue;
        }

        // 检查是否匹配
        if codex_fuzzy_candidate_matches(local, proxy) {
            let distance = (proxy_ts - local_ts).abs();
            match best_match {
                None => best_match = Some((idx, proxy, distance)),
                Some((_, _, prev_distance)) if distance < prev_distance => {
                    best_match = Some((idx, proxy, distance));
                }
                _ => {}
            }
        }
    }

    best_match.map(|(idx, proxy, _)| (idx, proxy))
}

/// 原始的 O(N×M) 实现，保留用于对比和回退
#[allow(dead_code)]
fn find_codex_fuzzy_matches_original(
    local_orphans: &[&LocalRequestRecord],
    proxy_orphans_visible: &[&UsageRecord],
    proxy_orphans_all_extra: &[&UsageRecord],
) -> Vec<CodexFuzzyOutcome> {
    let mut used_visible = vec![false; proxy_orphans_visible.len()];
    let mut used_all = vec![false; proxy_orphans_all_extra.len()];
    let mut outcomes = Vec::new();

    for local in local_orphans {
        let best_visible = proxy_orphans_visible
            .iter()
            .enumerate()
            .filter(|(idx, _)| !used_visible[*idx])
            .filter(|(_, p)| codex_fuzzy_candidate_matches(local, p))
            .min_by_key(|(_, p)| (p.timestamp / 1000 - local.timestamp).abs());

        if let Some((idx, proxy)) = best_visible {
            used_visible[idx] = true;
            outcomes.push(CodexFuzzyOutcome::MatchedVisible {
                local_key: canonical_request_key_for_local(local),
                proxy_key: canonical_request_key_for_proxy(proxy),
            });
            continue;
        }

        let best_all = proxy_orphans_all_extra
            .iter()
            .enumerate()
            .filter(|(idx, _)| !used_all[*idx])
            .filter(|(_, p)| codex_fuzzy_candidate_matches(local, p))
            .min_by_key(|(_, p)| (p.timestamp / 1000 - local.timestamp).abs());

        if let Some((idx, _)) = best_all {
            used_all[idx] = true;
            outcomes.push(CodexFuzzyOutcome::SuppressedByFilteredProxy {
                local_key: canonical_request_key_for_local(local),
            });
        }
        // else: genuinely local-only (proxy wasn't running for this request) — falls through
        // untouched to the ordinary `from_local` path.
    }

    outcomes
}

impl CoverageOrigin {
    pub fn as_storage_str(self) -> &'static str {
        match self {
            CoverageOrigin::ProxyOnly => "proxy_only",
            CoverageOrigin::LocalOnly => "local_only",
            CoverageOrigin::MergedProxyPreferred => "merged_proxy_preferred",
            CoverageOrigin::MergedFuzzyMatched => "merged_fuzzy_matched",
        }
    }

    pub fn from_storage_str(value: &str) -> Self {
        match value {
            "proxy_only" => CoverageOrigin::ProxyOnly,
            "local_only" => CoverageOrigin::LocalOnly,
            "merged_proxy_preferred" => CoverageOrigin::MergedProxyPreferred,
            "merged_fuzzy_matched" => CoverageOrigin::MergedFuzzyMatched,
            _ => CoverageOrigin::LocalOnly,
        }
    }
}

/// 根据 proxy 字段派生面向 UI 的来源标签。
/// 与 `SourceFilter` 的匹配规则保持同源：先看 api_key_prefix，再看 base_url。
fn derive_source_label(
    api_key_prefix: Option<&str>,
    request_base_url: Option<&str>,
) -> Option<String> {
    if let Some(prefix) = api_key_prefix {
        let trimmed = prefix.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(url) = request_base_url {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn is_opaque_model_name(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("unknown")
        || trimmed.eq_ignore_ascii_case("custom_model")
}

fn fallback_model_name_for_tool(tool: &str) -> Option<&'static str> {
    match tool {
        "qoder_ide" => Some("Qoder IDE"),
        "qoder_ide_cn" => Some("Qoder IDE CN"),
        "qoder_cli" => Some("Qoder CLI"),
        "qoder_work" => Some("Qoder Work"),
        "qoder_work_cn" => Some("Qoder Work CN"),
        _ => None,
    }
}

pub(crate) fn normalize_model_bucket(tool: &str, model: &str) -> String {
    if !is_opaque_model_name(model) {
        let trimmed = model.trim();
        return if tool == "reasonix" {
            trimmed
                .split_once('/')
                .map(|(_, model)| model.trim())
                .filter(|model| !model.is_empty())
                .unwrap_or(trimmed)
                .to_string()
        } else if tool.starts_with("qoder_") {
            crate::qoder_models::normalize_qoder_model_name(trimmed)
        } else {
            trimmed.to_string()
        };
    }

    fallback_model_name_for_tool(tool)
        .unwrap_or("unknown")
        .to_string()
}

pub(crate) fn matches_source_filter(fact: &MergedRequestFact, filter: &SourceFilter) -> bool {
    match filter {
        SourceFilter::All => true,
        SourceFilter::OfficialOpenAiOAuth => {
            fact.attribution_source_id.as_deref()
                == Some(crate::models::OFFICIAL_OPENAI_OAUTH_SOURCE_ID)
        }
        SourceFilter::Unknown { known_pairs } => {
            if fact.attribution_method == AttributionMethod::Manual {
                return fact.attribution_source_id.is_none();
            }
            if fact.attribution_source_id.is_some() {
                return false;
            }
            let prefix = fact.api_key_prefix.as_deref().unwrap_or_default();
            let base_url = fact.request_base_url.clone();
            let known = known_pairs
                .iter()
                .any(|(known_prefix, known_url)| known_prefix == prefix && *known_url == base_url);
            !known
        }
        SourceFilter::Source {
            source_id,
            api_key_prefixes,
            base_url,
        } => {
            if let Some(resolved_source_id) = fact.attribution_source_id.as_deref() {
                return resolved_source_id == source_id;
            }
            let prefix_match = fact
                .api_key_prefix
                .as_ref()
                .map(|prefix| api_key_prefixes.iter().any(|candidate| candidate == prefix))
                .unwrap_or(false);
            let base_url_match = &fact.request_base_url == base_url;
            prefix_match && base_url_match
        }
    }
}

impl MergedRequestFact {
    pub fn from_local(
        record: &LocalRequestRecord,
        meta: Option<&SessionMeta>,
        cost: f64,
        fallback_base_url: Option<&str>,
    ) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        Self {
            canonical_request_key: canonical_request_key_for_local(record),
            session_id: record.session_id.clone(),
            project_name,
            project_path,
            // api_key_prefix is never reconstructable for local-only records (no Authorization
            // header was observed); request_base_url can be best-effort filled by the caller
            // from Codex's currently-configured upstream (see service.rs's codex_fallback_base_url).
            api_key_prefix: None,
            request_base_url: fallback_base_url.map(str::to_string),
            tool: record.tool.clone(),
            timestamp_sec: record.timestamp,
            timestamp_ms: record.timestamp.saturating_mul(1000),
            model: normalize_model_bucket(&record.tool, &record.model),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            request_count: record.request_count.max(1),
            estimated_cost: record.explicit_estimated_cost.unwrap_or(cost).max(0.0),
            estimated: meta.map(|m| m.estimated).unwrap_or(false),
            coverage_origin: CoverageOrigin::LocalOnly,
            // Local transcript requests are treated as successful; no proxy performance fields available.
            status_code: Some(200),
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            // local 无 source 维度——明确标 None 表示「未识别来源」桶
            source_label: None,
            attribution_source_id: None,
            attribution_method: AttributionMethod::Unattributed,
        }
    }

    pub fn from_proxy(record: &UsageRecord, meta: Option<&SessionMeta>) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());
        let source_label = derive_source_label(
            record.api_key_prefix.as_deref(),
            record.request_base_url.as_deref(),
        );
        // 网关记录经归一化判定命中时（message_id 为真实上游 id 且 label 命中白名单），
        // 该 fact 对应真实工具的请求：tool 归因到归一化 tool_id 而非 api_gateway，
        // 与 merge 路径（tool 取 local）的语义一致，避免同一工具的请求在统计中
        // 分裂为 api_gateway 与真实工具两部分；未命中（fallback id / 未知 label /
        // 非网关）保持 client_tool 原样。
        let tool = normalized_gateway_tool(record)
            .map(str::to_string)
            .unwrap_or_else(|| record.client_tool.clone());

        Self {
            canonical_request_key: canonical_request_key_for_proxy(record),
            session_id: record.session_id.clone().unwrap_or_default(),
            project_name,
            project_path,
            api_key_prefix: record.api_key_prefix.clone(),
            request_base_url: record.request_base_url.clone(),
            tool: tool.clone(),
            timestamp_sec: record.timestamp / 1000,
            timestamp_ms: record.timestamp,
            model: normalize_model_bucket(&tool, &record.model),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            request_count: 1,
            estimated_cost: record.estimated_cost,
            estimated: false,
            coverage_origin: CoverageOrigin::ProxyOnly,
            status_code: Some(record.status_code),
            duration_ms: Some(record.duration_ms),
            output_tokens_per_second: record.output_tokens_per_second,
            ttft_ms: record.ttft_ms,
            source_label,
            attribution_source_id: None,
            attribution_method: AttributionMethod::Unattributed,
        }
    }

    /// 合并 proxy 与 local 的同一条请求事实。
    ///
    /// 字段优先级（按字段类别分桶，而不是一刀切「proxy 非零优先」）：
    ///
    /// - 身份字段
    ///   - `session_id` / `project_name` / `project_path`：**local 优先**（transcript 是会话归属的自然事实源）
    ///   - `tool`：local 非空则 local，否则 proxy
    ///   - `api_key_prefix` / `request_base_url`：**proxy 独有**
    /// - 用量字段
    ///   - `input_tokens` / `output_tokens`：**proxy 优先**（响应头/body 最权威）
    ///   - `cache_create_tokens` / `cache_read_tokens`：**local 优先**（JSONL 解析更全，
    ///     proxy 流式 SSE 经常拿到 0）
    ///   - `total_tokens`：**重新计算 = input + output + cache_create + cache_read**，
    ///     避免任一方少算导致 total 漂移
    /// - 时间字段
    ///   - `timestamp_sec`：local 优先（JSONL ISO 时间稳定）
    ///   - `timestamp_ms`：proxy 优先（毫秒精度）
    /// - 性能字段：**仅 proxy**，从不伪造
    /// - 成本字段：proxy `cost_locked = true` 时用 proxy；否则用 local 实时估算
    pub fn merge_proxy_preferred(
        proxy: &UsageRecord,
        local: &LocalRequestRecord,
        meta: Option<&SessionMeta>,
        fallback_cost: f64,
    ) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        let session_id = if !local.session_id.trim().is_empty() {
            local.session_id.clone()
        } else {
            proxy.session_id.clone().unwrap_or_default()
        };
        let tool = if !local.tool.trim().is_empty() {
            local.tool.clone()
        } else {
            proxy.client_tool.clone()
        };
        let model = if !proxy.model.trim().is_empty() {
            proxy.model.clone()
        } else {
            local.model.clone()
        };
        let model = normalize_model_bucket(&tool, &model);

        // 用量：proxy 优先 input/output，local 优先 cache_*
        let input_tokens = if proxy.input_tokens > 0 {
            proxy.input_tokens
        } else {
            local.input_tokens
        };
        let output_tokens = if proxy.output_tokens > 0 {
            proxy.output_tokens
        } else {
            local.output_tokens
        };
        let cache_create_tokens = if local.cache_create_tokens > 0 {
            local.cache_create_tokens
        } else {
            proxy.cache_create_tokens
        };
        let cache_read_tokens = if local.cache_read_tokens > 0 {
            local.cache_read_tokens
        } else {
            proxy.cache_read_tokens
        };
        // total 显式重新计算，不取任一方的旧值——避免任一方丢字段导致 total 漂移
        let total_tokens = input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_create_tokens)
            .saturating_add(cache_read_tokens);

        // 成本：cost_locked 表示用户/系统已经按"当时价格"冻结过这条记录，
        // 不能被实时估算覆盖；未 lock 的 proxy cost 与 local 估算同源，
        // 用 local 反而能在用户改价格表后立刻生效。
        let estimated_cost = if proxy.cost_locked {
            proxy.estimated_cost
        } else {
            local.explicit_estimated_cost.unwrap_or(fallback_cost)
        };
        let local_request_key = canonical_request_key_for_local(local);
        let canonical_request_key = if !local_request_key.trim().is_empty() {
            local_request_key
        } else {
            canonical_request_key_for_proxy(proxy)
        };

        Self {
            canonical_request_key,
            session_id,
            project_name,
            project_path,
            api_key_prefix: proxy.api_key_prefix.clone(),
            request_base_url: proxy.request_base_url.clone(),
            tool,
            timestamp_sec: local.timestamp,
            timestamp_ms: proxy.timestamp,
            model,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens,
            request_count: local.request_count.max(1),
            estimated_cost,
            estimated: meta.map(|m| m.estimated).unwrap_or(false),
            coverage_origin: CoverageOrigin::MergedProxyPreferred,
            status_code: Some(proxy.status_code),
            duration_ms: Some(proxy.duration_ms),
            output_tokens_per_second: proxy.output_tokens_per_second,
            ttft_ms: proxy.ttft_ms,
            // 同一请求 proxy 也有 → source 标签从 proxy 派生
            source_label: derive_source_label(
                proxy.api_key_prefix.as_deref(),
                proxy.request_base_url.as_deref(),
            ),
            attribution_source_id: None,
            attribution_method: AttributionMethod::Unattributed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_bucket_only_strips_provider_for_reasonix() {
        assert_eq!(
            normalize_model_bucket("reasonix", "基元律动/deepseek-v4-flash-0731"),
            "deepseek-v4-flash-0731"
        );
        assert_eq!(
            normalize_model_bucket("codex", "openai/gpt-5.4-2026-03-05"),
            "openai/gpt-5.4-2026-03-05"
        );
    }

    fn proxy_with(
        input: u64,
        output: u64,
        cc: u64,
        cr: u64,
        cost: f64,
        cost_locked: bool,
    ) -> UsageRecord {
        UsageRecord {
            timestamp: 1_700_000_000_500,
            message_id: "msg-1".to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_create_tokens: cc,
            cache_read_tokens: cr,
            total_tokens: input + output + cc + cr,
            model: "claude-3-5-sonnet".to_string(),
            session_id: Some("proxy-session".to_string()),
            status_code: 200,
            duration_ms: 5_000,
            output_tokens_per_second: Some(20.0),
            ttft_ms: Some(800),
            estimated_cost: cost,
            cost_locked,
            api_key_prefix: Some("sk-xxxxxxxxxxxx".to_string()),
            request_base_url: Some("https://api.anthropic.com".to_string()),
            client_tool: "claude_code".to_string(),
            ..Default::default()
        }
    }

    fn local_with(
        input: u64,
        output: u64,
        cc: u64,
        cr: u64,
        session_id: &str,
        timestamp: i64,
    ) -> LocalRequestRecord {
        LocalRequestRecord {
            session_id: session_id.to_string(),
            tool: "claude_code".to_string(),
            timestamp,
            message_id: "msg-1".to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_create_tokens: cc,
            cache_read_tokens: cr,
            total_tokens: input + output + cc + cr,
            model: "claude-3-5-sonnet".to_string(),
            is_subagent: false,
            ..Default::default()
        }
    }

    #[test]
    fn merge_cache_tokens_prefer_local() {
        // proxy 流式响应没拿到 cache，local 解析 JSONL 拿到了——应该用 local 的
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 500, 700, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.123);
        assert_eq!(merged.cache_create_tokens, 500);
        assert_eq!(merged.cache_read_tokens, 700);
    }

    #[test]
    fn merge_input_output_tokens_prefer_proxy() {
        let proxy = proxy_with(150, 250, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.input_tokens, 150);
        assert_eq!(merged.output_tokens, 250);
    }

    #[test]
    fn merge_total_tokens_recomputed_from_parts() {
        // proxy 缺 cache、local 缺 input/output 时，total 必须按合并后字段重算
        let proxy = proxy_with(150, 250, 0, 0, 0.0, false);
        let local = local_with(0, 0, 800, 900, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.input_tokens, 150);
        assert_eq!(merged.output_tokens, 250);
        assert_eq!(merged.cache_create_tokens, 800);
        assert_eq!(merged.cache_read_tokens, 900);
        assert_eq!(merged.total_tokens, 150 + 250 + 800 + 900);
    }

    #[test]
    fn merge_estimated_cost_respects_cost_locked() {
        // cost_locked=true：用 proxy 冻结值
        let proxy_locked = proxy_with(100, 200, 0, 0, 0.99, true);
        let local = local_with(100, 200, 100, 100, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy_locked, &local, None, 0.42);
        assert!((merged.estimated_cost - 0.99).abs() < 1e-9);

        // cost_locked=false：忽略 proxy.estimated_cost，使用 local 实时估算（fallback_cost）
        let proxy_unlocked = proxy_with(100, 200, 0, 0, 0.55, false);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy_unlocked, &local, None, 0.42);
        assert!(
            (merged.estimated_cost - 0.42).abs() < 1e-9,
            "unlocked proxy cost should be replaced by local estimate"
        );
    }

    #[test]
    fn merge_session_id_prefers_local_when_present() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        // proxy 提供了一个 session_id（可能是 legacy fallback），但 local 也有 → 用 local
        let local = local_with(100, 200, 0, 0, "real-session-uuid", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.session_id, "real-session-uuid");

        // local 没 session_id → 回退到 proxy
        let local_empty = local_with(100, 200, 0, 0, "", 1_700_000_000);
        let merged_empty =
            MergedRequestFact::merge_proxy_preferred(&proxy, &local_empty, None, 0.0);
        assert_eq!(merged_empty.session_id, "proxy-session");
    }

    #[test]
    fn merge_carries_proxy_performance_fields() {
        // 性能字段绝不应该被「合并」逻辑伪造或丢失——proxy 有就用，local 无能力提供
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.status_code, Some(200));
        assert_eq!(merged.duration_ms, Some(5_000));
        assert_eq!(merged.ttft_ms, Some(800));
        assert_eq!(merged.output_tokens_per_second, Some(20.0));
        assert!(matches!(
            merged.coverage_origin,
            CoverageOrigin::MergedProxyPreferred
        ));
    }

    #[test]
    fn merge_carries_proxy_only_identity_fields() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.api_key_prefix.as_deref(), Some("sk-xxxxxxxxxxxx"));
        assert_eq!(
            merged.request_base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
    }

    #[test]
    fn merge_uses_local_session_meta_when_available() {
        // SessionMeta 提供 project 信息 → 合并结果 project_name / project_path 应来自它
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let meta = SessionMeta {
            session_id: "sess".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/Users/me/work".to_string()),
            project_name: Some("MyProject".to_string()),
            ..Default::default()
        };
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, Some(&meta), 0.0);
        assert_eq!(merged.project_name.as_deref(), Some("MyProject"));
        assert_eq!(merged.project_path.as_deref(), Some("/Users/me/work"));
    }

    #[test]
    fn local_only_is_treated_as_success_without_performance() {
        // Local transcript requests are treated as successful 200s, but proxy-only performance
        // fields must remain absent.
        let local = local_with(100, 200, 50, 60, "sess", 1_700_000_000);
        let fact = MergedRequestFact::from_local(&local, None, 0.05, None);
        assert!(matches!(fact.coverage_origin, CoverageOrigin::LocalOnly));
        assert_eq!(fact.status_code, Some(200));
        assert_eq!(fact.duration_ms, None);
        assert_eq!(fact.ttft_ms, None);
        assert_eq!(fact.output_tokens_per_second, None);
        assert!(fact.api_key_prefix.is_none());
    }

    #[test]
    fn local_only_has_no_source_label() {
        // 本地 transcript 没有 source 维度，必须明确为 None 进入「未识别来源」桶
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let fact = MergedRequestFact::from_local(&local, None, 0.0, None);
        assert_eq!(fact.source_label, None);
    }

    #[test]
    fn qoder_model_bucket_uses_display_name_without_changing_local_record() {
        let mut local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        local.tool = "qoder_work_cn".to_string();
        local.model = "gm51model".to_string();

        let fact = MergedRequestFact::from_local(&local, None, 0.0, None);

        assert_eq!(local.model, "gm51model");
        assert_eq!(fact.model, "GLM-5.2");
    }

    #[test]
    fn from_proxy_derives_source_label_from_api_key_prefix() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        // proxy_with helper 设置了 api_key_prefix=Some("sk-xxxxxxxxxxxx")
        assert_eq!(fact.source_label.as_deref(), Some("sk-xxxxxxxxxxxx"));
    }

    #[test]
    fn from_proxy_falls_back_to_base_url_when_no_prefix() {
        let proxy = UsageRecord {
            api_key_prefix: None,
            request_base_url: Some("https://custom.endpoint".to_string()),
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(
            fact.source_label.as_deref(),
            Some("https://custom.endpoint")
        );
    }

    #[test]
    fn from_proxy_returns_none_label_when_both_missing() {
        let proxy = UsageRecord {
            api_key_prefix: None,
            request_base_url: None,
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(fact.source_label, None);
    }

    #[test]
    fn from_proxy_treats_empty_prefix_as_missing() {
        // 防御性：空白/空字符串前缀不应被视为有效来源标签
        let proxy = UsageRecord {
            api_key_prefix: Some("   ".to_string()),
            request_base_url: Some("https://fallback".to_string()),
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(fact.source_label.as_deref(), Some("https://fallback"));
    }

    #[test]
    fn merge_proxy_preferred_carries_source_label_from_proxy() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        // 合并时 source 永远跟 proxy 走——local 本就无 source 维度
        assert_eq!(merged.source_label.as_deref(), Some("sk-xxxxxxxxxxxx"));
    }

    #[test]
    fn merged_fact_preserves_canonical_request_key() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.canonical_request_key, "claude_code:msg-1");

        let local_only = MergedRequestFact::from_local(&local, None, 0.0, None);
        assert_eq!(local_only.canonical_request_key, "claude_code:msg-1");

        let proxy_only = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(proxy_only.canonical_request_key, "claude_code:msg-1");
    }

    #[test]
    fn opencode_local_and_proxy_keys_share_single_tool_prefix() {
        let proxy = UsageRecord {
            client_tool: "opencode".to_string(),
            message_id: "msg-opencode-1".to_string(),
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let local = LocalRequestRecord {
            tool: "opencode".to_string(),
            message_id: "msg-opencode-1".to_string(),
            ..local_with(100, 200, 0, 0, "opencode::sess", 1_700_000_000)
        };

        assert_eq!(
            canonical_request_key_for_local(&local),
            "opencode:msg-opencode-1"
        );
        assert_eq!(
            canonical_request_key_for_proxy(&proxy),
            "opencode:msg-opencode-1"
        );
    }

    #[test]
    fn partial_coverage_requires_mixed_proxy_and_local_only_data() {
        assert!(!has_partial_coverage(0, 1));
        assert!(!has_partial_coverage(3, 0));
        assert!(has_partial_coverage(2, 1));
    }

    // --- Codex fuzzy-match reconciliation (Fix A) ---

    fn codex_local_with(
        session_id: &str,
        timestamp: i64,
        message_id: &str,
        model: &str,
        total: u64,
    ) -> LocalRequestRecord {
        // Real Codex local records always carry the `codex::` namespacing prefix (see
        // codex_reader.rs::collect_codex_session_files) — build fixtures the same way so
        // these tests actually exercise the prefix-stripping comparison in
        // `codex_fuzzy_candidate_matches` instead of passing on a same-bare-string false
        // positive that production data would never hit.
        LocalRequestRecord {
            session_id: format!("{CODEX_SESSION_ID_PREFIX}{session_id}"),
            tool: "codex".to_string(),
            timestamp,
            message_id: message_id.to_string(),
            input_tokens: total / 2,
            output_tokens: total - total / 2,
            total_tokens: total,
            model: model.to_string(),
            ..Default::default()
        }
    }

    fn codex_proxy_with(
        session_id: &str,
        timestamp_ms: i64,
        message_id: &str,
        model: &str,
        total: u64,
    ) -> UsageRecord {
        UsageRecord {
            client_tool: "codex".to_string(),
            session_id: Some(session_id.to_string()),
            timestamp: timestamp_ms,
            message_id: message_id.to_string(),
            input_tokens: total / 2,
            output_tokens: total - total / 2,
            total_tokens: total,
            model: model.to_string(),
            status_code: 200,
            duration_ms: 1_000,
            estimated_cost: 0.01,
            api_key_prefix: Some("sk-codexprefix".to_string()),
            request_base_url: Some("https://sui-xiang.com".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn fuzzy_match_reconciles_close_timestamps_same_session_and_tokens() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let proxy = codex_proxy_with("sess-1", 1_700_000_002_000, "resp_abc123", "gpt-5", 300);

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0],
            CodexFuzzyOutcome::MatchedVisible { local_key, proxy_key }
                if *local_key == canonical_request_key_for_local(&local)
                    && *proxy_key == canonical_request_key_for_proxy(&proxy)
        ));
    }

    #[test]
    fn fuzzy_match_rejects_candidate_outside_tolerance_window() {
        // Just past the (now 10-minute) window: 601s apart must not match.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let proxy = codex_proxy_with("sess-1", 1_700_000_601_000, "resp_abc123", "gpt-5", 300);

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert!(outcomes.is_empty());
    }

    #[test]
    fn fuzzy_match_accepts_candidate_minutes_apart_within_window() {
        // Session-file flush latency / slow streaming can space the two timestamps out by
        // minutes; the widened window plus the exact token fingerprint must still reconcile.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let proxy = codex_proxy_with("sess-1", 1_700_000_120_000, "resp_slow", "gpt-5", 300);

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert_eq!(outcomes.len(), 1);
    }

    #[test]
    fn fuzzy_match_rejects_mismatched_tokens() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let proxy = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_abc123", "gpt-5", 400);

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert!(outcomes.is_empty());
    }

    #[test]
    fn fuzzy_match_rejects_same_total_but_different_breakdown() {
        // Per-field matching must reject two requests that happen to share a total but split it
        // differently between input and output — the old total-only check would wrongly merge.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let mut proxy = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_x", "gpt-5", 300);
        proxy.input_tokens = 100;
        proxy.output_tokens = 200; // total still 300

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert!(outcomes.is_empty());
    }

    #[test]
    fn fuzzy_match_allows_missing_local_cache_create_against_proxy_value() {
        // Codex JSONL never reports a cache-creation figure, so the local side is 0. A proxy
        // record that *did* observe cache_creation (making its total_tokens larger) must still
        // match — this is exactly the case the old total-equality check silently dropped.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let mut proxy = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_cc", "gpt-5", 300);
        proxy.cache_create_tokens = 4096;
        proxy.total_tokens += 4096;

        let outcomes = find_codex_fuzzy_matches(&[&local], &[&proxy], &[]);
        assert_eq!(outcomes.len(), 1);
    }

    #[test]
    fn fuzzy_match_allows_model_case_and_unknown_differences() {
        // Proxy may only learn the model from the response (blank / differently cased /
        // "unknown"); none of those should block an otherwise-exact token+time match.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "GPT-5", 300);
        let cased = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_a", "gpt-5", 300);
        assert_eq!(find_codex_fuzzy_matches(&[&local], &[&cased], &[]).len(), 1);

        let unknown = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_b", "unknown", 300);
        assert_eq!(
            find_codex_fuzzy_matches(&[&local], &[&unknown], &[]).len(),
            1
        );
    }

    #[test]
    fn fuzzy_match_ignores_session_id_since_proxy_never_captures_it_for_codex() {
        // Real Codex CLI requests carry no session/conversation identifier the proxy can
        // observe (verified against production data: session_id is empty for every captured
        // Codex proxy record), so a differing — or entirely absent — proxy session_id must not
        // block an otherwise-good match on model + per-field tokens + timestamp.
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let different_session =
            codex_proxy_with("sess-2", 1_700_000_001_000, "resp_a", "gpt-5", 300);
        assert_eq!(
            find_codex_fuzzy_matches(&[&local], &[&different_session], &[]).len(),
            1
        );
    }

    #[test]
    fn fuzzy_match_rejects_mismatched_model() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let wrong_model = codex_proxy_with("sess-1", 1_700_000_001_000, "resp_b", "gpt-4", 300);
        assert!(find_codex_fuzzy_matches(&[&local], &[&wrong_model], &[]).is_empty());
    }

    #[test]
    fn fuzzy_match_prefers_nearest_unclaimed_candidate_when_multiple_ties_exist() {
        // Both proxy candidates are within tolerance of BOTH local records (max diff 4s <= 5s),
        // so a naive first-match algorithm could wrongly pair local_a with `far`. The greedy
        // nearest-first algorithm must instead give each local record its closest candidate.
        let local_a = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let local_b = codex_local_with("sess-1", 1_700_000_004, "codex:sess-1:2", "gpt-5", 300);
        let near = codex_proxy_with("sess-1", 1_700_000_000_000, "resp_near", "gpt-5", 300);
        let far = codex_proxy_with("sess-1", 1_700_000_004_000, "resp_far", "gpt-5", 300);

        let outcomes = find_codex_fuzzy_matches(&[&local_a, &local_b], &[&far, &near], &[]);
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            &outcomes[0],
            CodexFuzzyOutcome::MatchedVisible { proxy_key, .. }
                if *proxy_key == canonical_request_key_for_proxy(&near)
        ));
        assert!(matches!(
            &outcomes[1],
            CodexFuzzyOutcome::MatchedVisible { proxy_key, .. }
                if *proxy_key == canonical_request_key_for_proxy(&far)
        ));
    }

    #[test]
    fn fuzzy_match_suppresses_local_when_only_filtered_proxy_candidate_exists() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let filtered_proxy =
            codex_proxy_with("sess-1", 1_700_000_001_000, "resp_filtered", "gpt-5", 300);

        let outcomes = find_codex_fuzzy_matches(&[&local], &[], &[&filtered_proxy]);
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0],
            CodexFuzzyOutcome::SuppressedByFilteredProxy { local_key }
                if *local_key == canonical_request_key_for_local(&local)
        ));
    }

    #[test]
    fn fuzzy_match_produces_no_outcome_when_no_candidate_at_all() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let outcomes = find_codex_fuzzy_matches(&[&local], &[], &[]);
        assert!(outcomes.is_empty());
    }

    #[test]
    fn codex_orphan_pools_excludes_non_codex_records() {
        let codex_local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let claude_local = local_with(100, 200, 0, 0, "claude-sess", 1_700_000_000);

        let mut local_index = HashMap::new();
        local_index.insert(
            canonical_request_key_for_local(&codex_local),
            codex_local.clone(),
        );
        local_index.insert(
            canonical_request_key_for_local(&claude_local),
            claude_local.clone(),
        );

        let proxy_index: HashMap<String, UsageRecord> = HashMap::new();
        let all_proxy_index: HashMap<String, UsageRecord> = HashMap::new();

        let (local_orphans, _, _) =
            codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
        assert_eq!(local_orphans.len(), 1);
        assert_eq!(local_orphans[0].tool, "codex");
    }

    #[test]
    fn codex_orphan_pools_includes_gateway_codex_record() {
        // 网关记录（client_tool=api_gateway）只要 caller label 归一化为 codex 且
        // message_id 为真实上游 id，就应进入 codex fuzzy 池；label 非 codex 的
        // 网关记录（如 Claude Code）不得进入。
        let codex_local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let mut local_index = HashMap::new();
        local_index.insert(
            canonical_request_key_for_local(&codex_local),
            codex_local.clone(),
        );

        let gateway_codex = gateway_proxy_with("resp_abc123", Some("Codex"), None);
        let gateway_claude = gateway_proxy_with("chatcmpl-abc123", Some("Claude Code"), None);

        let mut proxy_index = HashMap::new();
        proxy_index.insert(
            canonical_request_key_for_proxy(&gateway_codex),
            gateway_codex.clone(),
        );
        proxy_index.insert(
            canonical_request_key_for_proxy(&gateway_claude),
            gateway_claude.clone(),
        );
        let all_proxy_index = proxy_index.clone();

        let (_, proxy_orphans_visible, _) =
            codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
        // 只有归一化为 codex 的网关记录进入池；Claude Code 网关记录必须被排除。
        assert_eq!(proxy_orphans_visible.len(), 1);
        assert_eq!(proxy_orphans_visible[0].message_id, "resp_abc123");
    }

    #[test]
    fn fuzzy_match_reconciles_gateway_codex_record_with_local() {
        // 网关 Codex 记录与本地 Codex 扫描记录（合成 id）精确键永不相等，必须经
        // fuzzy 匹配合并为单条事实，否则同一请求双计。token 指纹需与本地一致。
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let mut gateway = gateway_proxy_with("resp_abc123", Some("Codex"), None);
        gateway.model = "gpt-5".to_string();
        gateway.input_tokens = 150;
        gateway.output_tokens = 150;
        gateway.total_tokens = 300;
        gateway.timestamp = 1_700_000_002_000;
        gateway.cache_create_tokens = 0;
        gateway.cache_read_tokens = 0;

        let mut local_index = HashMap::new();
        local_index.insert(canonical_request_key_for_local(&local), local.clone());
        let mut proxy_index = HashMap::new();
        proxy_index.insert(canonical_request_key_for_proxy(&gateway), gateway.clone());

        let all_proxy_index = proxy_index.clone();
        let (local_orphans, proxy_orphans_visible, proxy_orphans_all_extra) =
            codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
        let outcomes = find_codex_fuzzy_matches(
            &local_orphans,
            &proxy_orphans_visible,
            &proxy_orphans_all_extra,
        );

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            &outcomes[0],
            CodexFuzzyOutcome::MatchedVisible { local_key, proxy_key }
                if *local_key == canonical_request_key_for_local(&local)
                    && *proxy_key == canonical_request_key_for_proxy(&gateway)
        ));
    }

    #[test]
    fn from_local_uses_fallback_base_url_when_provided() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let fact = MergedRequestFact::from_local(&local, None, 0.0, Some("https://sui-xiang.com"));
        assert_eq!(
            fact.request_base_url.as_deref(),
            Some("https://sui-xiang.com")
        );
        assert!(fact.api_key_prefix.is_none());
    }

    #[test]
    fn from_local_leaves_request_base_url_none_without_fallback() {
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let fact = MergedRequestFact::from_local(&local, None, 0.0, None);
        assert!(fact.request_base_url.is_none());
    }

    #[test]
    fn end_to_end_fuzzy_reconciliation_prevents_double_count_and_attributes_correctly() {
        // Reproduces the reported bug: one local-scanned Codex record and one proxy-captured
        // Codex record for the *same* real request (same session/model/tokens, close
        // timestamps, different message_ids). Without fuzzy matching, both would surface as
        // separate facts (double count), and the local one would be "未归因" (unattributed).
        let local = codex_local_with("sess-1", 1_700_000_000, "codex:sess-1:1", "gpt-5", 300);
        let proxy = codex_proxy_with("sess-1", 1_700_000_002_000, "resp_abc123", "gpt-5", 300);

        let mut local_index = HashMap::new();
        local_index.insert(canonical_request_key_for_local(&local), local.clone());
        let mut proxy_index = HashMap::new();
        proxy_index.insert(canonical_request_key_for_proxy(&proxy), proxy.clone());
        let all_proxy_index = proxy_index.clone();

        // Exact-key match must fail (that's the bug) — the two canonical keys differ.
        assert_ne!(
            canonical_request_key_for_local(&local),
            canonical_request_key_for_proxy(&proxy)
        );

        let (local_orphans, proxy_orphans_visible, proxy_orphans_all_extra) =
            codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
        let outcomes = find_codex_fuzzy_matches(
            &local_orphans,
            &proxy_orphans_visible,
            &proxy_orphans_all_extra,
        );

        assert_eq!(outcomes.len(), 1);
        let CodexFuzzyOutcome::MatchedVisible { .. } = &outcomes[0] else {
            panic!("expected a MatchedVisible outcome");
        };

        let mut fact = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.02);
        fact.coverage_origin = CoverageOrigin::MergedFuzzyMatched;

        // Exactly one fact should represent this request, correctly attributed to the proxy's
        // source — not split into an unattributed local fact plus a proxy fact.
        assert_eq!(fact.api_key_prefix.as_deref(), Some("sk-codexprefix"));
        assert_eq!(
            fact.request_base_url.as_deref(),
            Some("https://sui-xiang.com")
        );
        assert!(matches!(
            fact.coverage_origin,
            CoverageOrigin::MergedFuzzyMatched
        ));
    }

    #[test]
    fn session_meta_lookup_key_adds_codex_prefix_only_for_codex() {
        // Regression guard: session_meta_by_id is indexed by the *local* reader's session id,
        // which for Codex is namespaced `codex::<uuid>` — the proxy only ever captures the bare
        // uuid. A genuinely proxy-only Codex fact (no local counterpart in range) must still
        // resolve to the same key the local session was indexed under.
        assert_eq!(
            session_meta_lookup_key_for_proxy("codex", "sess-1"),
            "codex::sess-1"
        );
        // Other tools' local session ids aren't namespaced the same way — the bare proxy id is
        // already the right lookup key and must not be altered.
        assert_eq!(
            session_meta_lookup_key_for_proxy("claude_code", "sess-1"),
            "sess-1"
        );
    }

    fn gateway_proxy_with(
        message_id: &str,
        caller_label: Option<&str>,
        canonical_key: Option<&str>,
    ) -> UsageRecord {
        UsageRecord {
            timestamp: 1_700_000_000_500,
            message_id: message_id.to_string(),
            canonical_request_key: canonical_key.map(str::to_string),
            input_tokens: 100,
            output_tokens: 50,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 150,
            model: "claude-3-5-sonnet".to_string(),
            session_id: Some("proxy-session".to_string()),
            status_code: 200,
            duration_ms: 5_000,
            estimated_cost: 0.01,
            client_tool: "api_gateway".to_string(),
            ingress_kind: "gateway".to_string(),
            gateway_profile_id: Some("profile-1".to_string()),
            gateway_caller_label: caller_label.map(str::to_string),
            gateway_request_id: Some("gw-123-0".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn gateway_label_hit_returns_tool_message_key() {
        // 网关 message_id（上游真实响应 id）+ 可归一化的 caller label → 返回
        // `{tool_id}:{message_id}`，与本地扫描键对齐，从而触发 proxy_preferred 合并。
        let record = gateway_proxy_with(
            "chatcmpl-abc123",
            Some("Claude Code"),
            Some("gateway:profile-1:gw-123-0"),
        );
        assert_eq!(
            canonical_request_key_for_proxy(&record),
            "claude_code:chatcmpl-abc123"
        );
    }

    #[test]
    fn gateway_label_miss_falls_back_to_storage_key() {
        // label 归一化失败（非白名单工具，如 Cursor）→ 回退存储层
        // `gateway:{profile_id}:{request_id}` 现状，保持 api_gateway 孤立显示。
        let record = gateway_proxy_with(
            "chatcmpl-abc123",
            Some("Cursor"),
            Some("gateway:profile-1:gw-123-0"),
        );
        assert_eq!(
            canonical_request_key_for_proxy(&record),
            "gateway:profile-1:gw-123-0"
        );
    }

    #[test]
    fn gateway_empty_message_id_falls_back() {
        // message_id 为空 → 不参与归一化合并，回退存储层键。
        let record =
            gateway_proxy_with("", Some("Claude Code"), Some("gateway:profile-1:gw-123-0"));
        assert_eq!(
            canonical_request_key_for_proxy(&record),
            "gateway:profile-1:gw-123-0"
        );
    }

    #[test]
    fn gateway_fallback_message_id_falls_back() {
        // fallback 合成 id（转发器无法提取真实响应 id 时生成）→ 回退存储层键，
        // 避免与本地扫描记录误合并。
        for message_id in [
            "claude_usage_missing_1700000000_200",
            "gemini_usage_missing_1700000000_429",
            "codex_usage_missing_1700000000_200",
            "anthropic_usage_missing_1700000000_200",
        ] {
            let record = gateway_proxy_with(
                message_id,
                Some("Claude Code"),
                Some("gateway:profile-1:gw-123-0"),
            );
            assert_eq!(
                canonical_request_key_for_proxy(&record),
                "gateway:profile-1:gw-123-0",
                "message_id: {message_id}"
            );
        }
    }

    #[test]
    fn gateway_message_id_with_whitespace_is_trimmed_in_normalized_key() {
        // message_id 带首尾空白：判定与拼接使用同一 trim 后值（本地键同样 trim），
        // 两侧键才能对齐。
        let record = gateway_proxy_with(" chatcmpl-abc123 ", Some("Claude Code"), None);
        assert_eq!(
            canonical_request_key_for_proxy(&record),
            "claude_code:chatcmpl-abc123"
        );
    }

    #[test]
    fn local_message_id_with_whitespace_is_trimmed_in_canonical_key() {
        // 与网关侧对称：canonical_request_key_for_local 的尾部分支也必须用
        // trim 后值拼接，否则同一 message_id 在 local/proxy 两侧键不一致。
        let mut local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        local.message_id = " chatcmpl-abc123 ".to_string();
        assert_eq!(
            canonical_request_key_for_local(&local),
            "claude_code:chatcmpl-abc123"
        );
    }

    #[test]
    fn gateway_proxy_only_fact_attributes_tool_to_normalized_tool() {
        // 网关归一化命中（真实 message_id + 白名单 label）→ proxy-only fact 的
        // tool 归因到真实工具，与键前缀一致，避免同一工具的请求在统计中分裂为
        // api_gateway 与真实工具两部分。
        let record = gateway_proxy_with(
            "chatcmpl-abc123",
            Some("Claude Code"),
            Some("gateway:profile-1:gw-123-0"),
        );
        let fact = MergedRequestFact::from_proxy(&record, None);
        assert_eq!(fact.tool, "claude_code");
        assert_eq!(fact.canonical_request_key, "claude_code:chatcmpl-abc123");
    }

    #[test]
    fn gateway_proxy_only_fact_keeps_api_gateway_when_label_misses() {
        // label 未命中白名单 → 保持 api_gateway 孤立显示，键也回退存储层键。
        let record = gateway_proxy_with(
            "chatcmpl-abc123",
            Some("Cursor"),
            Some("gateway:profile-1:gw-123-0"),
        );
        let fact = MergedRequestFact::from_proxy(&record, None);
        assert_eq!(fact.tool, "api_gateway");
        assert_eq!(fact.canonical_request_key, "gateway:profile-1:gw-123-0");
    }

    #[test]
    fn gateway_proxy_only_fact_keeps_api_gateway_for_fallback_message_id() {
        // fallback message_id → 不参与归一化，tool 保持 api_gateway。
        let record = gateway_proxy_with(
            "claude_usage_missing_1700000000_200",
            Some("Claude Code"),
            Some("gateway:profile-1:gw-123-0"),
        );
        let fact = MergedRequestFact::from_proxy(&record, None);
        assert_eq!(fact.tool, "api_gateway");
    }

    #[test]
    fn gateway_caller_label_matches_whitelisted_tools() {
        let cases = [
            ("claude_code", TOOL_CLAUDE_CODE),
            ("Claude Code", TOOL_CLAUDE_CODE),
            ("claude", TOOL_CLAUDE_CODE),
            ("codex", TOOL_CODEX),
            ("Codex CLI", TOOL_CODEX),
            ("openclaw", TOOL_OPENCLAW),
            ("opencode", TOOL_OPENCODE),
            ("OpenCode", TOOL_OPENCODE),
            ("reasonix", TOOL_REASONIX),
            ("gemini", TOOL_GEMINI),
            ("Gemini CLI", TOOL_GEMINI),
            ("hermes", TOOL_HERMES),
            ("qoder_ide", TOOL_QODER_IDE),
            ("Qoder", TOOL_QODER_IDE),
            ("qoder_ide_cn", TOOL_QODER_IDE_CN),
            ("qoder_cli", TOOL_QODER_CLI),
            ("qoder_work", TOOL_QODER_WORK),
            ("qoder_work_cn", TOOL_QODER_WORK_CN),
            ("copilot", TOOL_COPILOT),
            ("GitHub Copilot", TOOL_COPILOT),
        ];
        for (label, expected) in cases {
            assert_eq!(
                normalize_gateway_caller_label(label),
                Some(expected),
                "label: {label}"
            );
        }
        // 非白名单 label 一律回退，绝不误合并。
        for label in ["Cursor", "", "Trae", "Windsurf", "unknown"] {
            assert_eq!(
                normalize_gateway_caller_label(label),
                None,
                "label: {label}"
            );
        }
    }

    #[test]
    fn gateway_fallback_id_detection_covers_all_synthetic_markers() {
        assert!(is_gateway_fallback_message_id("claude_usage_missing_1_200"));
        assert!(is_gateway_fallback_message_id("codex_fallback_abc"));
        assert!(is_gateway_fallback_message_id(
            "anthropic_usage_missing_1_500"
        ));
        // OpenAI 转发器合成的 `codex_{ts}_{n}`（无 usage_missing 子串）必须被拦截
        assert!(is_gateway_fallback_message_id("codex_1700000000100_3"));
        assert!(!is_gateway_fallback_message_id("chatcmpl-abc123"));
        assert!(!is_gateway_fallback_message_id("msg_01AbCdef"));
        // 本地 Codex 扫描的 message_id 是 `codex:{session}:{index}`（冒号分隔），不得误伤
        assert!(!is_gateway_fallback_message_id("codex:rollout-abc-123:42"));
    }
}
