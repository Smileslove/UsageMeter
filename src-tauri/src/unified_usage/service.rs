use super::types::{CoverageOrigin, MergeMode, MergedCoverage, MergedRequestFact};
use crate::models::{AppSettings, SourceFilter, ToolFilter, UsageQueryFilter};
use crate::proxy::{ProjectStats, ProxyDatabase, SessionStats, UsageRecord};
use crate::session::{LocalRequestRecord, SessionMeta};
use std::collections::{BTreeSet, HashMap, HashSet};

fn resolve_merge_mode(settings: &AppSettings) -> MergeMode {
    match settings.data_source.as_str() {
        "proxy" => {
            let source_filter = settings.source_aware.build_filter();
            if matches!(source_filter, SourceFilter::All) {
                MergeMode::ProxyWithLocalFallback
            } else {
                MergeMode::ProxyOnly
            }
        }
        _ => MergeMode::LocalOnly,
    }
}

fn request_key_for_local(record: &LocalRequestRecord) -> String {
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            record.tool,
            record.session_id,
            record.timestamp,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.total_tokens
        )
    } else {
        format!("{}:{}", record.tool, record.message_id)
    }
}

fn request_key_for_proxy(record: &UsageRecord) -> String {
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            record.client_tool,
            record.session_id.clone().unwrap_or_default(),
            record.timestamp / 1000,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.total_tokens
        )
    } else {
        format!("{}:{}", record.client_tool, record.message_id)
    }
}

fn local_tool_matches(record: &LocalRequestRecord, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => record.tool == *tool,
    }
}

fn session_meta_matches(meta: &SessionMeta, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => meta.tool == *tool,
    }
}

fn build_local_meta_index(sessions: &[SessionMeta]) -> HashMap<String, SessionMeta> {
    sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect()
}

fn build_message_to_session_index(local_records: &[LocalRequestRecord]) -> HashMap<String, String> {
    let mut message_to_session = HashMap::new();
    for record in local_records {
        if !record.message_id.trim().is_empty() {
            message_to_session.insert(record.message_id.clone(), record.session_id.clone());
        }
    }
    message_to_session
}

fn attach_proxy_session_ids(
    proxy_records: &mut [UsageRecord],
    message_to_session: &HashMap<String, String>,
) {
    for record in proxy_records.iter_mut() {
        let needs_fill = record
            .session_id
            .as_ref()
            .map(|id| id.trim().is_empty())
            .unwrap_or(true);
        if needs_fill {
            if let Some(session_id) = message_to_session.get(&record.message_id) {
                record.session_id = Some(session_id.clone());
            }
        }
    }
}

fn compute_local_request_cost(
    record: &LocalRequestRecord,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    crate::models::estimate_session_cost(
        record.input_tokens,
        record.output_tokens,
        record.cache_create_tokens,
        record.cache_read_tokens,
        &record.model,
        pricings,
        match_mode,
    )
}

fn build_coverage(facts: &[MergedRequestFact]) -> MergedCoverage {
    let mut coverage = MergedCoverage::default();

    for fact in facts {
        match fact.coverage_origin {
            CoverageOrigin::ProxyOnly => coverage.proxy_backed_requests += 1,
            CoverageOrigin::LocalOnly => coverage.local_only_requests += 1,
            CoverageOrigin::MergedProxyPreferred => {
                coverage.proxy_backed_requests += 1;
                coverage.merged_overlap_requests += 1;
            }
        }
    }

    coverage.has_partial_status_coverage =
        coverage.proxy_backed_requests > 0 && coverage.local_only_requests > 0;
    coverage.has_partial_performance_coverage =
        coverage.proxy_backed_requests > 0 && coverage.local_only_requests > 0;
    coverage
}

fn build_local_request_index(
    local_records: &[LocalRequestRecord],
) -> HashMap<String, LocalRequestRecord> {
    let mut map = HashMap::new();
    for record in local_records {
        map.insert(request_key_for_local(record), record.clone());
    }
    map
}

fn build_proxy_request_index(proxy_records: &[UsageRecord]) -> HashMap<String, UsageRecord> {
    let mut map = HashMap::new();
    for record in proxy_records {
        map.insert(request_key_for_proxy(record), record.clone());
    }
    map
}

async fn fetch_proxy_records(
    proxy_db: &ProxyDatabase,
    usage_filter: &UsageQueryFilter,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
) -> Result<Vec<UsageRecord>, String> {
    let start_ms = start_epoch.unwrap_or(0).saturating_mul(1000);
    let end_ms = end_epoch.unwrap_or(i64::MAX / 1000).saturating_mul(1000);

    proxy_db
        .get_records_between_with_source(start_ms, end_ms, true, usage_filter)
        .await
}

pub async fn get_merged_request_facts(
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Vec<MergedRequestFact>, MergedCoverage), String> {
    let merge_mode = resolve_merge_mode(settings);
    let tool_filter = settings.client_tools.build_filter();
    let usage_filter = UsageQueryFilter {
        source: settings.source_aware.build_filter(),
        tool: settings.client_tools.build_filter(),
    };
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let pricing_match_mode = settings.model_pricing.match_mode.clone();

    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let local_sessions_all = local_db.get_all_sessions(&tool_filter)?;
    let local_sessions: Vec<SessionMeta> = local_sessions_all
        .into_iter()
        .filter(|meta| session_meta_matches(meta, &tool_filter))
        .collect();
    let mut local_records = local_db.get_all_request_records(&tool_filter)?;
    if let Some(start_epoch) = start_epoch {
        local_records.retain(|record| record.timestamp >= start_epoch);
    }
    if let Some(end_epoch) = end_epoch {
        local_records.retain(|record| record.timestamp < end_epoch);
    }
    local_records.retain(|record| local_tool_matches(record, &tool_filter));
    let session_meta_by_id = build_local_meta_index(&local_sessions);
    let message_to_session = build_message_to_session_index(&local_records);

    if matches!(merge_mode, MergeMode::LocalOnly) {
        let facts: Vec<MergedRequestFact> = local_records
            .iter()
            .map(|record| {
                let cost = compute_local_request_cost(record, &pricings, &pricing_match_mode);
                let meta = session_meta_by_id.get(&record.session_id);
                MergedRequestFact::from_local(record, meta, cost)
            })
            .collect();
        let coverage = build_coverage(&facts);
        return Ok((facts, coverage));
    }

    let (all_proxy_records, proxy_records) = if let Some(proxy_db) = ProxyDatabase::get_global() {
        proxy_db.backfill_unlocked_costs().await?;
        let mut records =
            fetch_proxy_records(proxy_db.as_ref(), &usage_filter, start_epoch, end_epoch).await?;
        attach_proxy_session_ids(&mut records, &message_to_session);
        let visible_records = records
            .iter()
            .filter(|record| include_errors || (200..300).contains(&record.status_code))
            .cloned()
            .collect();
        (records, visible_records)
    } else {
        (Vec::new(), Vec::new())
    };

    if matches!(merge_mode, MergeMode::ProxyOnly) {
        let facts: Vec<MergedRequestFact> = proxy_records
            .iter()
            .map(MergedRequestFact::from_proxy)
            .collect();
        let coverage = build_coverage(&facts);
        return Ok((facts, coverage));
    }

    let all_proxy_index = build_proxy_request_index(&all_proxy_records);
    let proxy_index = build_proxy_request_index(&proxy_records);
    let local_index = build_local_request_index(&local_records);

    let mut keys = HashSet::new();
    keys.extend(proxy_index.keys().cloned());
    keys.extend(local_index.keys().cloned());

    let mut merged = Vec::new();
    for key in keys {
        match (proxy_index.get(&key), local_index.get(&key)) {
            (Some(proxy), Some(local)) => {
                let meta = session_meta_by_id.get(&local.session_id);
                let fallback_cost =
                    compute_local_request_cost(local, &pricings, &pricing_match_mode);
                merged.push(MergedRequestFact::merge_proxy_preferred(
                    proxy,
                    local,
                    meta,
                    fallback_cost,
                ));
            }
            (Some(proxy), None) => merged.push(MergedRequestFact::from_proxy(proxy)),
            (None, Some(local)) => {
                if all_proxy_index.contains_key(&key) {
                    continue;
                }
                let meta = session_meta_by_id.get(&local.session_id);
                let cost = compute_local_request_cost(local, &pricings, &pricing_match_mode);
                merged.push(MergedRequestFact::from_local(local, meta, cost));
            }
            (None, None) => {}
        }
    }

    merged.sort_by_key(|fact| fact.timestamp_ms);
    let coverage = build_coverage(&merged);
    Ok((merged, coverage))
}

pub async fn get_merged_sessions(
    settings: &AppSettings,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionStats>, String> {
    let include_errors = if settings.data_source == "proxy" {
        settings.proxy.include_error_requests
    } else {
        true
    };
    let (facts, _) = get_merged_request_facts(settings, None, None, include_errors).await?;
    let tool_filter = settings.client_tools.build_filter();
    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let local_sessions = local_db.get_all_sessions(&tool_filter)?;
    let meta_by_id: HashMap<String, SessionMeta> = local_sessions
        .into_iter()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();

    let mut by_session: HashMap<String, Vec<MergedRequestFact>> = HashMap::new();
    for fact in facts {
        if fact.session_id.trim().is_empty() {
            continue;
        }
        by_session
            .entry(fact.session_id.clone())
            .or_default()
            .push(fact);
    }

    let mut result = Vec::new();
    for (session_id, session_facts) in by_session {
        let meta = meta_by_id.get(&session_id);
        let mut models = BTreeSet::new();
        let mut total_input_tokens = 0_u64;
        let mut total_output_tokens = 0_u64;
        let mut total_cache_create_tokens = 0_u64;
        let mut total_cache_read_tokens = 0_u64;
        let mut estimated_cost = 0.0_f64;
        let mut first_request_time = i64::MAX;
        let mut last_request_time = 0_i64;
        let mut total_duration_ms = 0_u64;
        let mut rate_sum = 0.0_f64;
        let mut rate_count = 0_u64;
        let mut ttft_sum = 0.0_f64;
        let mut ttft_count = 0_u64;
        let mut success_requests = 0_u64;
        let mut error_requests = 0_u64;
        let mut has_partial_status_coverage = false;
        let mut has_partial_performance_coverage = false;

        for fact in &session_facts {
            if !fact.model.trim().is_empty() {
                models.insert(fact.model.clone());
            }
            total_input_tokens += fact.input_tokens;
            total_output_tokens += fact.output_tokens;
            total_cache_create_tokens += fact.cache_create_tokens;
            total_cache_read_tokens += fact.cache_read_tokens;
            estimated_cost += fact.estimated_cost;
            first_request_time = first_request_time.min(fact.timestamp_sec);
            last_request_time = last_request_time.max(fact.timestamp_sec);

            if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
                has_partial_status_coverage = true;
                has_partial_performance_coverage = true;
            }

            if let Some(duration_ms) = fact.duration_ms {
                total_duration_ms += duration_ms;
            }
            if let Some(rate) = fact.output_tokens_per_second {
                if rate > 0.0 {
                    rate_sum += rate;
                    rate_count += 1;
                }
            }
            if let Some(ttft_ms) = fact.ttft_ms {
                if ttft_ms > 0 {
                    ttft_sum += ttft_ms as f64;
                    ttft_count += 1;
                }
            }
            if let Some(status_code) = fact.status_code {
                if status_code < 400 {
                    success_requests += 1;
                } else {
                    error_requests += 1;
                }
            }
        }

        result.push(SessionStats {
            session_id: session_id.clone(),
            tool: meta.map(|m| m.tool.clone()).unwrap_or_else(|| {
                settings
                    .client_tools
                    .active_tool_filter
                    .clone()
                    .unwrap_or_default()
            }),
            total_requests: session_facts.len() as u64,
            total_input_tokens,
            total_output_tokens,
            total_cache_create_tokens,
            total_cache_read_tokens,
            total_duration_ms: if has_partial_performance_coverage {
                0
            } else {
                total_duration_ms
            },
            avg_output_tokens_per_second: if has_partial_performance_coverage {
                0.0
            } else if rate_count > 0 {
                rate_sum / rate_count as f64
            } else {
                0.0
            },
            first_request_time: if first_request_time == i64::MAX {
                0
            } else {
                first_request_time
            },
            last_request_time,
            models: models.into_iter().collect(),
            avg_ttft_ms: if has_partial_performance_coverage {
                0.0
            } else if ttft_count > 0 {
                ttft_sum / ttft_count as f64
            } else {
                0.0
            },
            success_requests: if has_partial_status_coverage {
                0
            } else {
                success_requests
            },
            error_requests: if has_partial_status_coverage {
                0
            } else {
                error_requests
            },
            estimated_cost,
            is_cost_estimated: true,
            cwd: meta.and_then(|m| m.cwd.clone()),
            project_name: meta.and_then(|m| m.project_name.clone()),
            topic: meta.and_then(|m| m.topic.clone()),
            last_prompt: meta.and_then(|m| m.last_prompt.clone()),
            session_name: meta.and_then(|m| m.session_name.clone()),
        });
    }

    result.sort_by_key(|session| std::cmp::Reverse(session.last_request_time));
    Ok(result
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit.max(0) as usize)
        .collect())
}

pub async fn get_merged_session_detail(
    settings: &AppSettings,
    session_id: &str,
) -> Result<Option<SessionStats>, String> {
    let sessions = get_merged_sessions(settings, i64::MAX / 4, 0).await?;
    Ok(sessions
        .into_iter()
        .find(|session| session.session_id == session_id))
}

pub async fn get_merged_project_stats(settings: &AppSettings) -> Result<Vec<ProjectStats>, String> {
    let include_errors = if settings.data_source == "proxy" {
        settings.proxy.include_error_requests
    } else {
        true
    };
    let (facts, _) = get_merged_request_facts(settings, None, None, include_errors).await?;
    let mut map: HashMap<String, (ProjectStats, HashSet<String>)> = HashMap::new();

    for fact in facts {
        let project_name = fact
            .project_name
            .clone()
            .unwrap_or_else(|| "未命名项目".to_string());
        let entry = map.entry(project_name.clone()).or_insert_with(|| {
            (
                ProjectStats {
                    name: project_name.clone(),
                    session_count: 0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cache_create_tokens: 0,
                    total_cache_read_tokens: 0,
                    total_cost: 0.0,
                    last_active: 0,
                },
                HashSet::new(),
            )
        });

        entry.0.total_input_tokens += fact.input_tokens;
        entry.0.total_output_tokens += fact.output_tokens;
        entry.0.total_cache_create_tokens += fact.cache_create_tokens;
        entry.0.total_cache_read_tokens += fact.cache_read_tokens;
        entry.0.total_cost += fact.estimated_cost;
        entry.0.last_active = entry.0.last_active.max(fact.timestamp_sec);
        if !fact.session_id.trim().is_empty() {
            entry.1.insert(fact.session_id.clone());
        }
    }

    let mut projects: Vec<ProjectStats> = map
        .into_iter()
        .map(|(_, (mut stats, sessions))| {
            stats.session_count = sessions.len() as u64;
            stats
        })
        .collect();
    projects.sort_by_key(|project| std::cmp::Reverse(project.last_active));
    Ok(projects)
}
