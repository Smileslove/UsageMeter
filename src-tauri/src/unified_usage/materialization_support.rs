use crate::models::AppSettings;

pub(super) fn materialization_state_matches(
    state: &crate::local_usage::UnifiedDayMaterializationState,
    local_snapshot: &crate::local_usage::UnifiedDayLocalSnapshot,
    proxy_snapshot: crate::proxy::ProxyDayDependencySnapshot,
    pricing_fingerprint: u64,
    settings: &AppSettings,
) -> bool {
    state.is_finalized
        && state.day_boundary_mode
            == crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
        && state.pricing_fingerprint == pricing_fingerprint
        && state.local_request_count == local_snapshot.local_request_count
        && state.local_max_sync_version == local_snapshot.local_max_sync_version
        && state.local_max_timestamp == local_snapshot.local_max_timestamp
        && state.remote_request_count == local_snapshot.remote_request_count
        && state.remote_max_export_seq == local_snapshot.remote_max_export_seq
        && state.remote_max_timestamp == local_snapshot.remote_max_timestamp
        && state.proxy_record_count == proxy_snapshot.record_count
        && state.proxy_max_timestamp_ms == proxy_snapshot.max_timestamp_ms
        && state.proxy_max_updated_at == proxy_snapshot.max_updated_at
}

pub(super) struct MaterializationStateBuildContext<'a> {
    pub(super) local_date: &'a str,
    pub(super) fact_count: usize,
    pub(super) local_snapshot: &'a crate::local_usage::UnifiedDayLocalSnapshot,
    pub(super) proxy_snapshot: crate::proxy::ProxyDayDependencySnapshot,
    pub(super) pricing_fingerprint: u64,
    pub(super) max_fact_timestamp_ms: i64,
    pub(super) materialized_at: i64,
    pub(super) settings: &'a AppSettings,
}

pub(super) fn build_materialization_state(
    ctx: MaterializationStateBuildContext<'_>,
) -> crate::local_usage::UnifiedDayMaterializationState {
    let MaterializationStateBuildContext {
        local_date,
        fact_count,
        local_snapshot,
        proxy_snapshot,
        pricing_fingerprint,
        max_fact_timestamp_ms,
        materialized_at,
        settings,
    } = ctx;
    crate::local_usage::UnifiedDayMaterializationState {
        local_date: local_date.to_string(),
        day_boundary_mode: crate::utils::business_time::normalize_day_boundary_mode(
            &settings.day_boundary_mode,
        ),
        fact_count: fact_count as u64,
        local_request_count: local_snapshot.local_request_count,
        local_max_sync_version: local_snapshot.local_max_sync_version,
        local_max_timestamp: local_snapshot.local_max_timestamp,
        remote_request_count: local_snapshot.remote_request_count,
        remote_max_export_seq: local_snapshot.remote_max_export_seq,
        remote_max_timestamp: local_snapshot.remote_max_timestamp,
        proxy_record_count: proxy_snapshot.record_count,
        proxy_all_record_count: proxy_snapshot.record_count,
        proxy_max_timestamp_ms: proxy_snapshot.max_timestamp_ms,
        proxy_max_updated_at: proxy_snapshot.max_updated_at,
        max_fact_timestamp_ms,
        pricing_fingerprint,
        is_finalized: true,
        finalized_at: Some(materialized_at),
        materialized_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependencies() -> (
        crate::local_usage::UnifiedDayLocalSnapshot,
        crate::proxy::ProxyDayDependencySnapshot,
    ) {
        (
            crate::local_usage::UnifiedDayLocalSnapshot {
                local_request_count: 2,
                local_max_sync_version: 3,
                local_max_timestamp: 4,
                remote_request_count: 5,
                remote_max_export_seq: 6,
                remote_max_timestamp: 7,
            },
            crate::proxy::ProxyDayDependencySnapshot {
                record_count: 8,
                max_timestamp_ms: 9,
                max_updated_at: 10,
            },
        )
    }

    fn state(
        local: &crate::local_usage::UnifiedDayLocalSnapshot,
        proxy: crate::proxy::ProxyDayDependencySnapshot,
    ) -> crate::local_usage::UnifiedDayMaterializationState {
        crate::local_usage::UnifiedDayMaterializationState {
            local_date: "2026-07-01".to_string(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 1,
            local_request_count: local.local_request_count,
            local_max_sync_version: local.local_max_sync_version,
            local_max_timestamp: local.local_max_timestamp,
            remote_request_count: local.remote_request_count,
            remote_max_export_seq: local.remote_max_export_seq,
            remote_max_timestamp: local.remote_max_timestamp,
            proxy_record_count: proxy.record_count,
            proxy_all_record_count: proxy.record_count,
            proxy_max_timestamp_ms: proxy.max_timestamp_ms,
            proxy_max_updated_at: proxy.max_updated_at,
            max_fact_timestamp_ms: 9,
            pricing_fingerprint: 11,
            is_finalized: true,
            finalized_at: Some(12),
            materialized_at: 12,
        }
    }

    #[test]
    fn rejects_changed_local_dependency_snapshot() {
        let settings = AppSettings::default();
        let (mut local, proxy) = dependencies();
        let stored = state(&local, proxy);
        assert!(materialization_state_matches(
            &stored, &local, proxy, 11, &settings
        ));

        local.local_request_count += 1;
        assert!(!materialization_state_matches(
            &stored, &local, proxy, 11, &settings
        ));
    }

    #[test]
    fn rejects_changed_proxy_dependency_snapshot() {
        let settings = AppSettings::default();
        let (local, mut proxy) = dependencies();
        let stored = state(&local, proxy);
        proxy.max_updated_at += 1;

        assert!(!materialization_state_matches(
            &stored, &local, proxy, 11, &settings
        ));
    }
}
