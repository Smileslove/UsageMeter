use super::super::types::{StatisticsBucket, StatisticsMetric, StatisticsTrendPoint};
use crate::models::AppSettings;
use crate::unified_usage::MergedRequestFact;
use chrono::{Local, NaiveDate, TimeZone};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

pub(super) type DayAccumulatorMap = HashMap<String, (StatAccumulator, HashSet<String>)>;
pub(super) type StatAccumulator = super::super::accumulator::FactAccumulator;

pub(super) fn add_fact_to_stat_acc(acc: &mut StatAccumulator, fact: &MergedRequestFact) {
    acc.add_fact(fact);
}

pub(super) fn normalize_range(query: &super::super::types::StatisticsQuery) -> (i64, i64) {
    let start = query.start_epoch.max(0);
    let end = query.end_epoch.max(start + 1);
    (start, end)
}

fn bucket_step_seconds(bucket: &StatisticsBucket) -> i64 {
    match bucket {
        StatisticsBucket::Hour => 60 * 60,
        StatisticsBucket::Day => 24 * 60 * 60,
    }
}

pub(super) fn bucket_name(bucket: &StatisticsBucket) -> String {
    match bucket {
        StatisticsBucket::Hour => "hour".to_string(),
        StatisticsBucket::Day => "day".to_string(),
    }
}

pub(super) fn bucket_start(epoch: i64, bucket: &StatisticsBucket) -> i64 {
    let step = bucket_step_seconds(bucket);
    (epoch / step) * step
}

fn bucket_label(epoch: i64, bucket: &StatisticsBucket) -> String {
    let dt = Local
        .timestamp_opt(epoch, 0)
        .single()
        .unwrap_or_else(Local::now);
    match bucket {
        StatisticsBucket::Hour => dt.format("%m-%d %H:00").to_string(),
        StatisticsBucket::Day => dt.format("%m-%d").to_string(),
    }
}

pub(super) fn make_empty_trend(
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let step = bucket_step_seconds(bucket);
    let mut points = Vec::new();
    let mut cursor = bucket_start(start_epoch, bucket);
    while cursor < end_epoch {
        points.push(StatisticsTrendPoint {
            start_epoch: cursor,
            label: bucket_label(cursor, bucket),
            ..Default::default()
        });
        cursor += step;
    }
    points
}

fn apply_acc_to_trend_point(point: &mut StatisticsTrendPoint, acc: &StatAccumulator) {
    point.request_count = acc.request_count;
    point.total_tokens = acc.total_tokens;
    point.input_tokens = acc.input_tokens;
    point.output_tokens = acc.output_tokens;
    point.cache_create_tokens = acc.cache_create_tokens;
    point.cache_read_tokens = acc.cache_read_tokens;
    point.cost = acc.cost;
    point.avg_tokens_per_second =
        (acc.rate_count > 0).then_some(acc.rate_sum / acc.rate_count as f64);
}

pub(super) fn trend_from_map(
    trend_map: &HashMap<i64, StatAccumulator>,
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let mut trend = make_empty_trend(start_epoch, end_epoch, bucket);
    for point in &mut trend {
        if let Some(acc) = trend_map.get(&point.start_epoch) {
            apply_acc_to_trend_point(point, acc);
        }
    }
    trend
}

pub(super) fn value_for_metric(point: &StatisticsTrendPoint, metric: &StatisticsMetric) -> f64 {
    match metric {
        StatisticsMetric::Cost => point.cost,
        StatisticsMetric::Requests => point.request_count as f64,
        StatisticsMetric::Tokens => point.total_tokens as f64,
    }
}

pub(super) fn collect_day_activity_from_facts(
    facts: Vec<MergedRequestFact>,
    day_map: &mut DayAccumulatorMap,
    settings: &AppSettings,
) {
    for fact in facts {
        let date =
            crate::utils::business_time::business_date_for_timestamp(fact.timestamp_sec, settings);
        let entry = day_map.entry(date).or_default();
        add_fact_to_stat_acc(&mut entry.0, &fact);
        if !fact.model.is_empty() {
            entry.1.insert(fact.model);
        }
    }
}

pub(super) fn to_date_key(timestamp_sec: i64, settings: &AppSettings) -> String {
    crate::utils::business_time::business_date_for_timestamp(timestamp_sec, settings)
}

pub(super) fn month_day_count(year: i32, month: u8) -> u32 {
    for day in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month as u32, day).is_some() {
            return day;
        }
    }
    30
}

pub(super) fn local_date_start_epoch(local_date: &str, settings: &AppSettings) -> i64 {
    crate::utils::business_time::business_date_epoch_bounds(local_date, settings)
        .map(|(start, _)| start)
        .unwrap_or_else(|_| Local::now().timestamp())
}

pub(super) fn fingerprint_pricings(pricings: &[crate::models::ModelPricingConfig]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for pricing in pricings {
        pricing.model_id.hash(&mut hasher);
        pricing.display_name.hash(&mut hasher);
        pricing.input_price.to_bits().hash(&mut hasher);
        pricing.output_price.to_bits().hash(&mut hasher);
        pricing.cache_read_price.map(f64::to_bits).hash(&mut hasher);
        pricing
            .cache_write_price
            .map(f64::to_bits)
            .hash(&mut hasher);
        pricing.source.hash(&mut hasher);
        pricing.last_updated.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn cache_key_for_source_filter(filter: &crate::models::SourceFilter) -> String {
    match filter {
        crate::models::SourceFilter::All => "all".to_string(),
        crate::models::SourceFilter::Unknown { known_pairs } => {
            format!("unknown:{known_pairs:?}")
        }
        crate::models::SourceFilter::Source {
            api_key_prefixes,
            base_url,
        } => format!("source:{api_key_prefixes:?}:{base_url:?}"),
    }
}

pub(super) fn cache_key_for_tool_filter(filter: &crate::models::ToolFilter) -> String {
    match filter {
        crate::models::ToolFilter::All => "all".to_string(),
        crate::models::ToolFilter::Tool(tool) => format!("tool:{tool}"),
        crate::models::ToolFilter::AnyOf(tools) => {
            let mut sorted = tools.clone();
            sorted.sort();
            format!("anyof:{}", sorted.join(","))
        }
    }
}

pub(super) fn normalized_day_boundary_mode(settings: &AppSettings) -> String {
    crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
}
