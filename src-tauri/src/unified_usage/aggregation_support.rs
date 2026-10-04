use super::derived_support::{project_descriptor_for_session, session_project_identity};
use crate::models::CurrencySettings;
use crate::proxy::{ProjectStats, ProjectToolStats};
use crate::session::{wsl_distro_from_path, SessionMeta};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ProjectAggregate<'a> {
    pub(super) stats: ProjectStats,
    // 聚合生命周期内 facts 与 session metadata 均保持存活；借用稳定标识，避免
    // 为同一 session/tool 的每条事实反复分配 String。
    pub(super) sessions: HashSet<&'a str>,
    pub(super) tool_sessions: HashMap<&'a str, HashSet<&'a str>>,
}

pub(super) fn session_usage_fully_covered(
    _meta: Option<&SessionMeta>,
    _tool: &str,
    _proxy_backed_requests: u64,
    _unresolved_proxy_requests: u64,
    _now_sec: i64,
) -> bool {
    // ReasonX 本地会话链路已移除（v27）：不再有基于会话级 telemetry 的覆盖判定。
    // 其它工具的会话覆盖语义维持"完整覆盖"（由逐请求事实驱动）。
    true
}

pub(super) fn build_metadata_only_session_stats(
    meta: &SessionMeta,
    _currency_settings: &CurrencySettings,
    now_sec: i64,
) -> crate::proxy::SessionStats {
    let explicit_cost_usd = meta
        .explicit_estimated_cost
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    crate::proxy::SessionStats {
        session_id: meta.session_id.clone(),
        tool: meta.tool.clone(),
        total_requests: meta.message_count,
        total_input_tokens: meta.total_input_tokens,
        total_output_tokens: meta.total_output_tokens,
        total_cache_create_tokens: meta.total_cache_create_tokens,
        total_cache_read_tokens: meta.total_cache_read_tokens,
        total_duration_ms: meta.total_elapsed_ms,
        avg_output_tokens_per_second: 0.0,
        first_request_time: meta.start_time,
        last_request_time: meta.end_time.max(meta.last_modified),
        models: meta.models.clone(),
        avg_ttft_ms: 0.0,
        success_requests: 0,
        error_requests: 0,
        estimated_cost: explicit_cost_usd.unwrap_or(0.0),
        is_cost_estimated: explicit_cost_usd.is_none(),
        usage_fully_covered: session_usage_fully_covered(Some(meta), &meta.tool, 0, 0, now_sec),
        covered_requests: 0,
        uncovered_requests: meta.message_count,
        cwd: meta.cwd.clone(),
        project_key: Some(project_descriptor_for_session(meta).key),
        project_name: meta.project_name.clone(),
        project_identity: Some(
            session_project_identity(meta.project_name.as_deref(), meta.cwd.as_deref()).to_string(),
        ),
        topic: meta.topic.clone(),
        last_prompt: meta.last_prompt.clone(),
        session_name: meta.session_name.clone(),
        scope: meta.scope.clone(),
        wsl_distro: wsl_distro_from_path(&meta.file_path),
    }
}

pub(super) fn merge_metadata_only_project<'a>(
    map: &mut HashMap<String, ProjectAggregate<'a>>,
    meta: &'a SessionMeta,
) {
    let descriptor = project_descriptor_for_session(meta);
    let entry = map
        .entry(descriptor.key.clone())
        .or_insert_with(|| ProjectAggregate {
            stats: ProjectStats {
                name: descriptor.name.clone(),
                project_key: Some(descriptor.key.clone()),
                project_identity: Some(descriptor.identity.clone()),
                project_path: descriptor.path.clone(),
                usage_fully_covered: true,
                covered_requests: 0,
                uncovered_requests: 0,
                ..Default::default()
            },
            ..Default::default()
        });

    if entry.stats.project_path.is_none() {
        entry.stats.project_path = descriptor.path.clone();
    }
    if entry.stats.project_key.is_none() {
        entry.stats.project_key = Some(descriptor.key.clone());
    }
    if entry.stats.project_identity.is_none() {
        entry.stats.project_identity = Some(descriptor.identity);
    }
    entry.stats.request_count += meta.message_count;
    entry.stats.uncovered_requests += meta.message_count;
    entry.stats.total_input_tokens += meta.total_input_tokens;
    entry.stats.total_output_tokens += meta.total_output_tokens;
    entry.stats.total_cache_create_tokens += meta.total_cache_create_tokens;
    entry.stats.total_cache_read_tokens += meta.total_cache_read_tokens;
    entry.stats.last_active = entry
        .stats
        .last_active
        .max(meta.end_time.max(meta.last_modified));
    if !meta.session_id.trim().is_empty() {
        entry.sessions.insert(meta.session_id.as_str());
        entry
            .tool_sessions
            .entry(meta.tool.as_str())
            .or_default()
            .insert(meta.session_id.as_str());
    } else {
        entry.tool_sessions.entry(meta.tool.as_str()).or_default();
    }

    let tool_stats = entry
        .stats
        .tool_breakdown
        .iter_mut()
        .find(|stats| stats.tool == meta.tool);
    let tool_stats = match tool_stats {
        Some(stats) => stats,
        None => {
            entry.stats.tool_breakdown.push(ProjectToolStats {
                tool: meta.tool.clone(),
                usage_fully_covered: true,
                covered_requests: 0,
                uncovered_requests: 0,
                ..Default::default()
            });
            entry.stats.tool_breakdown.last_mut().unwrap()
        }
    };
    tool_stats.request_count += meta.message_count;
    tool_stats.uncovered_requests += meta.message_count;
    tool_stats.total_input_tokens += meta.total_input_tokens;
    tool_stats.total_output_tokens += meta.total_output_tokens;
    tool_stats.total_cache_create_tokens += meta.total_cache_create_tokens;
    tool_stats.total_cache_read_tokens += meta.total_cache_read_tokens;
    tool_stats.last_active = tool_stats
        .last_active
        .max(meta.end_time.max(meta.last_modified));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionMeta;

    #[test]
    fn merge_metadata_only_project_merges_multiple_sessions() {
        let mut map: HashMap<String, ProjectAggregate<'_>> = HashMap::new();
        let meta1 = SessionMeta {
            session_id: "sess-1".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/proj".to_string()),
            project_name: Some("proj".to_string()),
            message_count: 10,
            total_input_tokens: 100,
            total_output_tokens: 200,
            total_cache_create_tokens: 5,
            total_cache_read_tokens: 20,
            end_time: 500,
            last_modified: 400,
            ..Default::default()
        };
        let meta2 = SessionMeta {
            session_id: "sess-2".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/proj".to_string()),
            project_name: Some("proj".to_string()),
            message_count: 5,
            total_input_tokens: 50,
            total_output_tokens: 60,
            total_cache_create_tokens: 0,
            total_cache_read_tokens: 10,
            end_time: 700,
            last_modified: 600,
            ..Default::default()
        };

        merge_metadata_only_project(&mut map, &meta1);
        merge_metadata_only_project(&mut map, &meta2);

        let entry = map.get("/proj").expect("project entry");
        assert_eq!(entry.stats.name, "proj");
        assert_eq!(entry.stats.request_count, 15);
        assert_eq!(entry.stats.uncovered_requests, 15);
        assert_eq!(entry.stats.total_input_tokens, 150);
        assert_eq!(entry.stats.total_output_tokens, 260);
        assert_eq!(entry.stats.total_cache_create_tokens, 5);
        assert_eq!(entry.stats.total_cache_read_tokens, 30);
        assert_eq!(entry.stats.last_active, 700); // 取 max(end_time, last_modified)
        assert_eq!(entry.sessions.len(), 2);
        assert_eq!(entry.stats.tool_breakdown.len(), 1);
        assert_eq!(entry.stats.tool_breakdown[0].tool, "claude_code");
        assert_eq!(entry.stats.tool_breakdown[0].request_count, 15);
    }

    #[test]
    fn merge_metadata_only_project_empty_session_id_records_tool_only() {
        let mut map: HashMap<String, ProjectAggregate<'_>> = HashMap::new();
        let meta = SessionMeta {
            session_id: "".to_string(),
            tool: "opencode".to_string(),
            cwd: Some("/proj".to_string()),
            message_count: 3,
            end_time: 100,
            ..Default::default()
        };

        merge_metadata_only_project(&mut map, &meta);

        let entry = map.get("/proj").expect("project entry");
        assert!(entry.sessions.is_empty());
        // 空 session_id 只登记 tool 维度的桶，不产生 session 集合成员。
        assert!(entry.tool_sessions.contains_key("opencode"));
        assert_eq!(entry.tool_sessions["opencode"].len(), 0);
    }

    #[test]
    fn merge_metadata_only_project_fills_project_path_on_first_seen() {
        let mut map: HashMap<String, ProjectAggregate<'_>> = HashMap::new();
        let meta = SessionMeta {
            session_id: "sess-1".to_string(),
            tool: "codex".to_string(),
            cwd: Some("/work/proj".to_string()),
            message_count: 1,
            end_time: 100,
            ..Default::default()
        };
        merge_metadata_only_project(&mut map, &meta);

        let entry = map.get("/work/proj").expect("project entry");
        assert_eq!(entry.stats.project_path.as_deref(), Some("/work/proj"));
        assert_eq!(entry.stats.project_identity.as_deref(), Some("project"));
    }

    #[test]
    fn merge_metadata_only_project_tool_breakdown_appears_on_first_tool() {
        let mut map: HashMap<String, ProjectAggregate<'_>> = HashMap::new();
        let meta1 = SessionMeta {
            session_id: "s1".to_string(),
            tool: "codex".to_string(),
            cwd: Some("/proj".to_string()),
            message_count: 4,
            end_time: 100,
            ..Default::default()
        };
        let meta2 = SessionMeta {
            session_id: "s2".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/proj".to_string()),
            message_count: 2,
            end_time: 200,
            ..Default::default()
        };
        merge_metadata_only_project(&mut map, &meta1);
        merge_metadata_only_project(&mut map, &meta2);

        let entry = map.get("/proj").expect("project entry");
        assert_eq!(entry.stats.tool_breakdown.len(), 2);
        let codex = &entry.stats.tool_breakdown[0];
        assert_eq!(codex.tool, "codex");
        assert_eq!(codex.request_count, 4);
        assert_eq!(entry.stats.tool_breakdown[1].request_count, 2);
    }

    #[test]
    fn build_metadata_only_session_stats_maps_meta_fields() {
        let meta = SessionMeta {
            session_id: "sess-map".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/proj".to_string()),
            project_name: Some("proj".to_string()),
            message_count: 7,
            total_input_tokens: 111,
            total_output_tokens: 222,
            total_cache_create_tokens: 11,
            total_cache_read_tokens: 22,
            total_elapsed_ms: 5_000,
            start_time: 100,
            end_time: 200,
            last_modified: 150,
            models: vec!["gpt-4o".to_string()],
            ..Default::default()
        };
        let currency = CurrencySettings::default();
        let stats = build_metadata_only_session_stats(&meta, &currency, 1_000);

        assert_eq!(stats.session_id, "sess-map");
        assert_eq!(stats.tool, "claude_code");
        assert_eq!(stats.total_requests, 7);
        assert_eq!(stats.total_input_tokens, 111);
        assert_eq!(stats.total_output_tokens, 222);
        assert_eq!(stats.total_cache_create_tokens, 11);
        assert_eq!(stats.total_cache_read_tokens, 22);
        assert_eq!(stats.total_duration_ms, 5_000);
        assert_eq!(stats.first_request_time, 100);
        assert_eq!(stats.last_request_time, 200); // max(end_time, last_modified)
        assert_eq!(stats.models, vec!["gpt-4o"]);
        // 无显式费用 → 估算为 0 且标记 estimated。
        assert_eq!(stats.estimated_cost, 0.0);
        assert!(stats.is_cost_estimated);
        // 非 reasonix 工具 → 完整覆盖。
        assert!(stats.usage_fully_covered);
        assert_eq!(stats.covered_requests, 0);
        assert_eq!(stats.uncovered_requests, 7);
        assert_eq!(stats.project_identity.as_deref(), Some("project"));
        assert_eq!(stats.project_name.as_deref(), Some("proj"));
    }

    #[test]
    fn build_metadata_only_session_stats_uses_explicit_estimated_cost() {
        let meta = SessionMeta {
            session_id: "rx-sess".to_string(),
            tool: "reasonix".to_string(),
            message_count: 5,
            end_time: 1_000,
            last_modified: 1_000,
            explicit_estimated_cost: Some(10.0),
            ..Default::default()
        };
        let currency = CurrencySettings::default();
        let stats = build_metadata_only_session_stats(&meta, &currency, 1_100);

        assert_eq!(stats.estimated_cost, 10.0);
        assert!(!stats.is_cost_estimated);
        assert!(stats.usage_fully_covered);
        assert_eq!(stats.uncovered_requests, 5);
    }

    #[test]
    fn build_metadata_only_session_stats_ignores_explicit_cost_without_estimated_alias() {
        // ReasonX 本地会话链路已移除（v27）：explicit_cost + 币种换算不再参与；
        // 只有明确的 explicit_estimated_cost（已确认 USD）才被采用。
        let meta = SessionMeta {
            session_id: "rx-sess-cny".to_string(),
            tool: "reasonix".to_string(),
            message_count: 2,
            end_time: 1_000,
            last_modified: 1_000,
            explicit_cost: Some(700.0),
            explicit_cost_currency: Some("CNY".to_string()),
            ..Default::default()
        };
        let mut currency = CurrencySettings::default();
        currency.exchange_rates.insert("CNY".to_string(), 7.0);
        let stats = build_metadata_only_session_stats(&meta, &currency, 1_100);
        assert_eq!(stats.estimated_cost, 0.0);
        assert!(stats.is_cost_estimated);
    }
}
