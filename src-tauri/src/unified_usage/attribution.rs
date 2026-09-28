use super::{AttributionMethod, CoverageOrigin, MergedRequestFact};
use crate::local_usage::{
    LocalUsageDatabase, ManualAttributionOverride, PassiveAttributionInterval,
};
use crate::models::{
    ApiSource, AppSettings, GEMINI_OAUTH_PLAN_LABEL_PREFIX,
    OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID, OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
    OFFICIAL_OPENAI_OAUTH_SOURCE_ID, OPENAI_OAUTH_PLAN_LABEL_PREFIX,
};
use std::collections::HashMap;

fn matching_interval<'a>(
    intervals: &'a [PassiveAttributionInterval],
    fact: &MergedRequestFact,
) -> Option<&'a PassiveAttributionInterval> {
    let timestamp_ms = fact.timestamp_ms.max(0);
    let mut matches = intervals.iter().filter(|interval| {
        interval.tool == fact.tool
            && matches!(
                interval.auth_mode.as_str(),
                "api_key" | "chatgpt_oauth" | "gemini_oauth" | "claude_oauth"
            )
            && interval.valid_from_ms <= timestamp_ms
            && interval.confirmed_until_ms >= timestamp_ms
    });
    let interval = matches.next()?;
    matches.next().is_none().then_some(interval)
}

fn first_party_oauth_source_label(source_id: &str, plan_type: Option<&str>) -> String {
    let plan_prefix = match source_id {
        OFFICIAL_OPENAI_OAUTH_SOURCE_ID => OPENAI_OAUTH_PLAN_LABEL_PREFIX,
        OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID => GEMINI_OAUTH_PLAN_LABEL_PREFIX,
        _ => return source_id.to_string(),
    };
    plan_type
        .map(|plan_type| format!("{plan_prefix}{plan_type}"))
        .unwrap_or_else(|| source_id.to_string())
}

fn apply_source(fact: &mut MergedRequestFact, source: &ApiSource, method: AttributionMethod) {
    fact.attribution_source_id = Some(source.id.clone());
    fact.attribution_method = method;
    fact.source_label = source
        .display_name
        .clone()
        .or_else(|| source.base_url.clone());
}

fn apply_manual_override(
    fact: &mut MergedRequestFact,
    override_row: &ManualAttributionOverride,
    settings: &AppSettings,
) {
    fact.attribution_method = AttributionMethod::Manual;
    let source = override_row.source_id.as_deref().and_then(|source_id| {
        settings
            .source_aware
            .sources
            .iter()
            .find(|source| source.id == source_id)
    });
    // A deleted source must fall back to the unknown bucket. Keeping its stale ID would make
    // the request invisible from both the active source and the unknown-source filters.
    fact.attribution_source_id = source.map(|source| source.id.clone());
    fact.source_label = source.and_then(|source| {
        source
            .display_name
            .clone()
            .or_else(|| source.base_url.clone())
    });
}

/// Enriches merged facts without mutating scanner facts. Only local-only facts may receive
/// automatic attribution; a manual override always wins, including an explicit manual clear.
pub(crate) fn apply_passive_attribution(
    database: &LocalUsageDatabase,
    facts: &mut [MergedRequestFact],
    settings: &AppSettings,
) -> Result<(), String> {
    if facts.is_empty() {
        return Ok(());
    }
    let request_keys: Vec<String> = facts
        .iter()
        .map(|fact| fact.canonical_request_key.clone())
        .collect();
    let overrides: HashMap<String, ManualAttributionOverride> = database
        .manual_attribution_overrides(&request_keys)?
        .into_iter()
        .map(|row| (row.request_key.clone(), row))
        .collect();
    let start_ms = facts
        .iter()
        .map(|fact| fact.timestamp_ms)
        .min()
        .unwrap_or(0);
    let end_ms = facts
        .iter()
        .map(|fact| fact.timestamp_ms)
        .max()
        .unwrap_or(0);
    let intervals = database.passive_attribution_intervals_for_range(start_ms, end_ms)?;

    for fact in facts {
        // Materialized local facts may carry an earlier derived label. Clear it before each
        // resolution so deleting a source or exposing a configuration gap never leaves stale UI.
        if fact.coverage_origin == CoverageOrigin::LocalOnly {
            fact.attribution_source_id = None;
            fact.attribution_method = AttributionMethod::Unattributed;
            fact.source_label = None;
        }
        if let Some(override_row) = overrides.get(&fact.canonical_request_key) {
            apply_manual_override(fact, override_row, settings);
            continue;
        }
        if fact.coverage_origin != CoverageOrigin::LocalOnly {
            continue;
        }
        let Some(interval) = matching_interval(&intervals, fact) else {
            continue;
        };
        let Some(source_id) = interval.source_id.as_deref() else {
            continue;
        };
        if matches!(
            source_id,
            OFFICIAL_OPENAI_OAUTH_SOURCE_ID
                | OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID
                | OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID
        ) {
            fact.request_base_url = Some(interval.base_url.clone());
            fact.attribution_source_id = Some(source_id.to_string());
            fact.attribution_method = AttributionMethod::ConfigInferred;
            fact.source_label = Some(first_party_oauth_source_label(
                source_id,
                interval.plan_type.as_deref(),
            ));
            continue;
        }
        fact.request_base_url = Some(interval.base_url.clone());
        if let Some(source) = settings
            .source_aware
            .sources
            .iter()
            .find(|source| source.id == source_id)
        {
            apply_source(fact, source, AttributionMethod::ConfigInferred);
        } else {
            // A direct provider may have been auto-registered during this same refresh while
            // the caller still holds the previous settings snapshot. Keep the resolved source
            // visible instead of dropping it into the unknown bucket until the next reload.
            fact.attribution_source_id = Some(source_id.to_string());
            fact.attribution_method = AttributionMethod::ConfigInferred;
            fact.source_label = Some(interval.base_url.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_usage::PassiveAttributionSnapshot;
    use crate::models::{ApiSource, SourceAwareSettings, SourceFilter};

    fn temp_db() -> (tempfile::TempDir, LocalUsageDatabase) {
        let directory = tempfile::tempdir().expect("create temp dir");
        let path = directory.path().join("local_usage.db");
        let database = LocalUsageDatabase::new_with_path(&path).expect("open temp db");
        (directory, database)
    }

    fn snapshot<'a>(
        source_id: Option<&'a str>,
        observed_at_ms: i64,
    ) -> PassiveAttributionSnapshot<'a> {
        PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: "openai",
            base_url: "https://api.example.com/v1",
            auth_mode: "api_key",
            credential_id: "credential-a",
            source_id,
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        }
    }

    fn local_fact() -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: "codex:request-1".to_string(),
            session_id: "codex::session-1".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "codex".to_string(),
            timestamp_sec: 1,
            timestamp_ms: 1_500,
            model: "gpt-5".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 30,
            request_count: 1,
            estimated_cost: 0.0,
            estimated: false,
            coverage_origin: CoverageOrigin::LocalOnly,
            status_code: Some(200),
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            source_label: None,
            attribution_source_id: None,
            attribution_method: AttributionMethod::Unattributed,
        }
    }

    #[test]
    fn an_interval_without_a_bound_source_stays_unattributed() {
        let (_directory, database) = temp_db();
        database
            .record_passive_attribution_snapshot(snapshot(None, 1_000))
            .expect("record ambiguous route");
        let mut settings = AppSettings::default();
        settings.source_aware = SourceAwareSettings {
            sources: vec![source("work", "https://api.example.com/v1")],
            active_source_filter: None,
        };
        let mut facts = vec![local_fact()];

        apply_passive_attribution(&database, &mut facts, &settings).expect("resolve source");
        assert_eq!(facts[0].attribution_method, AttributionMethod::Unattributed);
        assert_eq!(facts[0].attribution_source_id, None);
    }

    #[test]
    fn newly_registered_source_remains_visible_before_settings_reload() {
        let (_directory, database) = temp_db();
        database
            .record_passive_attribution_snapshot(snapshot(Some("new-source"), 1_000))
            .expect("record route");
        database
            .record_passive_attribution_snapshot(snapshot(Some("new-source"), 2_000))
            .expect("confirm route");
        let mut facts = vec![local_fact()];

        apply_passive_attribution(&database, &mut facts, &AppSettings::default())
            .expect("resolve source without refreshed settings");

        assert_eq!(
            facts[0].attribution_source_id.as_deref(),
            Some("new-source")
        );
        assert_eq!(
            facts[0].source_label.as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(
            facts[0].attribution_method,
            AttributionMethod::ConfigInferred
        );
    }

    #[test]
    fn manual_override_beats_a_stable_config_inference() {
        let (_directory, database) = temp_db();
        database
            .record_passive_attribution_snapshot(snapshot(Some("work"), 1_000))
            .expect("record route");
        database
            .record_passive_attribution_snapshot(snapshot(Some("work"), 2_000))
            .expect("confirm route");
        let mut settings = AppSettings::default();
        settings.source_aware.sources = vec![source("work", "https://api.example.com/v1")];
        let mut facts = vec![local_fact()];

        apply_passive_attribution(&database, &mut facts, &settings).expect("infer source");
        assert_eq!(facts[0].attribution_source_id.as_deref(), Some("work"));
        assert_eq!(
            facts[0].attribution_method,
            AttributionMethod::ConfigInferred
        );

        database
            .set_manual_attribution_overrides(
                &[facts[0].canonical_request_key.clone()],
                None,
                3_000,
            )
            .expect("save override");
        apply_passive_attribution(&database, &mut facts, &settings).expect("apply override");
        assert_eq!(facts[0].attribution_source_id, None);
        assert_eq!(facts[0].attribution_method, AttributionMethod::Manual);
    }

    #[test]
    fn deleted_manual_source_returns_to_the_unknown_bucket() {
        let (_directory, database) = temp_db();
        let mut facts = vec![local_fact()];
        database
            .set_manual_attribution_overrides(
                &[facts[0].canonical_request_key.clone()],
                Some("removed-source"),
                1_000,
            )
            .expect("save override");

        apply_passive_attribution(&database, &mut facts, &AppSettings::default())
            .expect("apply deleted source override");

        assert_eq!(facts[0].attribution_method, AttributionMethod::Manual);
        assert_eq!(facts[0].attribution_source_id, None);
        assert!(crate::unified_usage::matches_source_filter(
            &facts[0],
            &SourceFilter::Unknown {
                known_pairs: Vec::new()
            }
        ));
    }

    #[test]
    fn confirmed_oauth_plan_resolves_to_the_official_openai_source() {
        let (_directory, database) = temp_db();
        let oauth_snapshot = |observed_at_ms| PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: "openai",
            base_url: "https://chatgpt.com/backend-api/codex",
            auth_mode: "chatgpt_oauth",
            credential_id: "oauth-credential",
            source_id: Some(crate::models::OFFICIAL_OPENAI_OAUTH_SOURCE_ID),
            plan_type: Some("plus"),
            plan_is_confirmed: true,
            observed_at_ms,
        };
        database
            .record_passive_attribution_snapshot(oauth_snapshot(1_000))
            .expect("record plan");
        database
            .record_passive_attribution_snapshot(oauth_snapshot(2_000))
            .expect("confirm plan");
        let mut facts = vec![local_fact()];

        apply_passive_attribution(&database, &mut facts, &AppSettings::default())
            .expect("apply OAuth attribution");

        assert_eq!(
            facts[0].attribution_source_id.as_deref(),
            Some(crate::models::OFFICIAL_OPENAI_OAUTH_SOURCE_ID)
        );
        assert_eq!(
            facts[0].source_label.as_deref(),
            Some("__openai_oauth_plan:plus")
        );
        assert!(crate::unified_usage::matches_source_filter(
            &facts[0],
            &SourceFilter::OfficialOpenAiOAuth
        ));
    }

    #[test]
    fn local_facts_follow_historical_intervals_across_a_codex_config_switch() {
        let (_directory, database) = temp_db();
        let oauth_snapshot = |observed_at_ms| PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: "openai",
            base_url: "https://chatgpt.com/backend-api/codex",
            auth_mode: "chatgpt_oauth",
            credential_id: "oauth-credential",
            source_id: Some(OFFICIAL_OPENAI_OAUTH_SOURCE_ID),
            plan_type: Some("plus"),
            plan_is_confirmed: true,
            observed_at_ms,
        };
        database
            .record_passive_attribution_snapshot(oauth_snapshot(1_000))
            .expect("record OAuth route");
        database
            .record_passive_attribution_snapshot(oauth_snapshot(2_000))
            .expect("confirm OAuth route");
        database
            .record_passive_attribution_snapshot(PassiveAttributionSnapshot {
                tool: "codex",
                provider_id: "openai",
                base_url: "https://api.example.com/v1",
                auth_mode: "api_key",
                credential_id: "api-credential",
                source_id: Some("api-source"),
                plan_type: None,
                plan_is_confirmed: false,
                observed_at_ms: 3_000,
            })
            .expect("record API route");
        database
            .record_passive_attribution_snapshot(PassiveAttributionSnapshot {
                tool: "codex",
                provider_id: "openai",
                base_url: "https://api.example.com/v1",
                auth_mode: "api_key",
                credential_id: "api-credential",
                source_id: Some("api-source"),
                plan_type: None,
                plan_is_confirmed: false,
                observed_at_ms: 4_000,
            })
            .expect("confirm API route");

        let mut settings = AppSettings::default();
        settings.source_aware.sources = vec![source("api-source", "https://api.example.com/v1")];
        let mut gap_fact = local_fact();
        gap_fact.canonical_request_key = "codex:gap".to_string();
        gap_fact.timestamp_sec = 2;
        gap_fact.timestamp_ms = 2_500;
        let mut api_fact = local_fact();
        api_fact.canonical_request_key = "codex:api".to_string();
        api_fact.timestamp_sec = 3;
        api_fact.timestamp_ms = 3_500;
        let mut facts = vec![local_fact(), gap_fact, api_fact];

        apply_passive_attribution(&database, &mut facts, &settings)
            .expect("apply historical attribution");

        assert_eq!(
            facts[0].attribution_source_id.as_deref(),
            Some(OFFICIAL_OPENAI_OAUTH_SOURCE_ID)
        );
        assert_eq!(
            facts[0].request_base_url.as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );
        assert_eq!(facts[1].attribution_method, AttributionMethod::Unattributed);
        assert_eq!(facts[1].attribution_source_id, None);
        assert_eq!(facts[1].request_base_url, None);
        assert_eq!(
            facts[2].attribution_source_id.as_deref(),
            Some("api-source")
        );
        assert_eq!(
            facts[2].request_base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn confirmed_gemini_plan_resolves_to_the_official_google_source() {
        let (_directory, database) = temp_db();
        let oauth_snapshot = |observed_at_ms| PassiveAttributionSnapshot {
            tool: "gemini",
            provider_id: "google",
            base_url: "https://cloudcode-pa.googleapis.com",
            auth_mode: "gemini_oauth",
            credential_id: "oauth-credential",
            source_id: Some(crate::models::OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID),
            plan_type: Some("pro"),
            plan_is_confirmed: true,
            observed_at_ms,
        };
        database
            .record_passive_attribution_snapshot(oauth_snapshot(1_000))
            .expect("record plan");
        database
            .record_passive_attribution_snapshot(oauth_snapshot(2_000))
            .expect("confirm plan");
        let mut facts = vec![local_fact()];
        facts[0].tool = "gemini".to_string();

        apply_passive_attribution(&database, &mut facts, &AppSettings::default())
            .expect("apply OAuth attribution");

        assert_eq!(
            facts[0].attribution_source_id.as_deref(),
            Some(crate::models::OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID)
        );
        assert_eq!(
            facts[0].source_label.as_deref(),
            Some("__gemini_oauth_plan:pro")
        );
        assert!(crate::unified_usage::matches_source_filter(
            &facts[0],
            &SourceFilter::OfficialGoogleGeminiOAuth
        ));
    }

    #[test]
    fn claude_oauth_resolves_to_the_official_anthropic_source() {
        let (_directory, database) = temp_db();
        let oauth_snapshot = |observed_at_ms| PassiveAttributionSnapshot {
            tool: "claude_code",
            provider_id: "anthropic",
            base_url: "https://api.anthropic.com",
            auth_mode: "claude_oauth",
            credential_id: "oauth-credential",
            source_id: Some(crate::models::OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID),
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        };
        database
            .record_passive_attribution_snapshot(oauth_snapshot(1_000))
            .expect("record OAuth configuration");
        database
            .record_passive_attribution_snapshot(oauth_snapshot(2_000))
            .expect("confirm OAuth configuration");
        let mut facts = vec![local_fact()];
        facts[0].tool = "claude_code".to_string();

        apply_passive_attribution(&database, &mut facts, &AppSettings::default())
            .expect("apply OAuth attribution");

        assert_eq!(
            facts[0].attribution_source_id.as_deref(),
            Some(crate::models::OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID)
        );
        assert_eq!(
            facts[0].source_label.as_deref(),
            Some(crate::models::OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID)
        );
        assert!(crate::unified_usage::matches_source_filter(
            &facts[0],
            &SourceFilter::OfficialAnthropicClaudeOAuth
        ));
    }

    fn source(id: &str, base_url: &str) -> ApiSource {
        ApiSource {
            id: id.to_string(),
            display_name: Some(id.to_string()),
            base_url: Some(base_url.to_string()),
            api_key_prefixes: Vec::new(),
            api_key_notes: HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: None,
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }
}
