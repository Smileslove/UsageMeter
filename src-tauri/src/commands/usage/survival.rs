//! 限额生存层 —— 本地事实派生信号
//!
//! 仅基于本地统一事实计算「会话锚定 5h 块 / 燃烧速率 / 历史基线」三类信号，
//! 不发任何网络请求、不依赖来源归因。真实配额(T1)由前端用 `subscriptionQuota`
//! 叠加，余额型(Balance)留待后续阶段。
//!
//! 设计依据：`doc/限额生存层详细设计.md`。

use serde::Serialize;

use crate::unified_usage::MergedRequestFact;

/// 5 小时块长度（秒）。与 Claude/Codex 真实会话窗口一致。
const BLOCK_SECONDS: i64 = 5 * 3600;
/// 燃烧速率取样窗口（秒），默认最近 90 分钟。
const BURN_SAMPLE_SECONDS: i64 = 90 * 60;
/// 基线回看窗口（秒），最近 7 天。
const BASELINE_LOOKBACK_SECONDS: i64 = 7 * 24 * 3600;
/// 基线最少需要的「活跃小时」数，不足则不产出基线。
const BASELINE_MIN_ACTIVE_HOURS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// 本地信号的可用档位（T1/T2 由上层决定，这里只反映本地能算出什么）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SurvivalSourceKind {
    /// 有足够历史，可给出相对基线信号。
    Baseline,
    /// 样本不足。
    None,
}

/// 会话锚定的当前 5h 块。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchoredBlock {
    pub start_epoch: i64,
    pub resets_at_epoch: i64,
    pub used_tokens: u64,
    pub used_requests: u64,
    pub elapsed_seconds: i64,
    pub remaining_seconds: i64,
    /// 按当前 burn 外推到块结束时的预计用量（token）。
    pub projected_end_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnRate {
    pub tokens_per_hour: f64,
    pub requests_per_hour: f64,
    pub sample_seconds: i64,
    pub sample_requests: u64,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    /// 近 7 天「活跃小时」平均 token 速率。
    pub avg_tokens_per_hour: f64,
    /// 当前 burn 相对活跃均速的倍数（如 1.8 表示比平时快 80%）。
    pub relative_to_baseline: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitSurvivalSnapshot {
    pub generated_at_epoch: i64,
    pub source_kind: SurvivalSourceKind,
    pub block: Option<AnchoredBlock>,
    pub burn: BurnRate,
    pub baseline: Option<Baseline>,
}

/// 装配本地生存快照。`facts` 需按 `timestamp_sec` 升序，`now_epoch` 为秒。
pub fn build_limit_survival(facts: &[MergedRequestFact], now_epoch: i64) -> LimitSurvivalSnapshot {
    let burn = compute_burn_rate(facts, now_epoch);
    let baseline = compute_baseline(facts, now_epoch, burn.tokens_per_hour);
    let block = compute_current_block(facts, now_epoch, burn.tokens_per_hour);

    let source_kind = if baseline.is_some() {
        SurvivalSourceKind::Baseline
    } else {
        SurvivalSourceKind::None
    };

    LimitSurvivalSnapshot {
        generated_at_epoch: now_epoch,
        source_kind,
        block,
        burn,
        baseline,
    }
}

/// 计算当前活跃的会话锚定 5h 块。
///
/// 规则（与 ccusage 一致）：从最早事实起，遇到「距块起点 ≥ 5h」或「距上一条
/// ≥ 5h 的空档」即开启新块；最后一个块若已越过 start+5h，则视为空闲（返回 None）。
fn compute_current_block(
    facts: &[MergedRequestFact],
    now_epoch: i64,
    tokens_per_hour: f64,
) -> Option<AnchoredBlock> {
    let first = facts.first()?;
    let mut block_start = first.timestamp_sec;
    let mut prev = first.timestamp_sec;

    for fact in facts.iter().skip(1) {
        let ts = fact.timestamp_sec;
        if ts - block_start >= BLOCK_SECONDS || ts - prev >= BLOCK_SECONDS {
            block_start = ts;
        }
        prev = ts;
    }

    let resets_at_epoch = block_start + BLOCK_SECONDS;
    // 当前块已过期（用户已空闲 ≥ 5h）→ 无活跃块。
    if now_epoch >= resets_at_epoch {
        return None;
    }

    let mut used_tokens: u64 = 0;
    let mut used_requests: u64 = 0;
    for fact in facts.iter().rev() {
        if fact.timestamp_sec < block_start {
            break;
        }
        used_tokens = used_tokens.saturating_add(fact.total_tokens);
        used_requests = used_requests.saturating_add(1);
    }

    let elapsed_seconds = (now_epoch - block_start).max(0);
    let remaining_seconds = (resets_at_epoch - now_epoch).max(0);
    let projected_added = (tokens_per_hour * (remaining_seconds as f64 / 3600.0)).max(0.0) as u64;
    let projected_end_tokens = used_tokens.saturating_add(projected_added);

    Some(AnchoredBlock {
        start_epoch: block_start,
        resets_at_epoch,
        used_tokens,
        used_requests,
        elapsed_seconds,
        remaining_seconds,
        projected_end_tokens,
    })
}

/// 计算燃烧速率（最近 ~90 分钟，按实际样本跨度归一化到每小时）。
fn compute_burn_rate(facts: &[MergedRequestFact], now_epoch: i64) -> BurnRate {
    let cutoff = now_epoch - BURN_SAMPLE_SECONDS;
    let sample: Vec<&MergedRequestFact> = facts
        .iter()
        .rev()
        .take_while(|f| f.timestamp_sec >= cutoff)
        .collect();

    if sample.is_empty() {
        return BurnRate {
            tokens_per_hour: 0.0,
            requests_per_hour: 0.0,
            sample_seconds: 0,
            sample_requests: 0,
            confidence: Confidence::Low,
        };
    }

    // sample 是逆序收集的，最早一条在末尾。
    let first_ts = sample.last().map(|f| f.timestamp_sec).unwrap_or(now_epoch);
    let span_seconds = (now_epoch - first_ts).max(1);
    let span_hours = span_seconds as f64 / 3600.0;

    let total_tokens: u64 = sample.iter().map(|f| f.total_tokens).sum();
    let request_count = sample.len() as u64;

    let tokens_per_hour = total_tokens as f64 / span_hours;
    let requests_per_hour = request_count as f64 / span_hours;

    let confidence = if request_count >= 8 && span_seconds >= 1800 {
        Confidence::High
    } else if request_count >= 3 {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    BurnRate {
        tokens_per_hour,
        requests_per_hour,
        sample_seconds: span_seconds,
        sample_requests: request_count,
        confidence,
    }
}

/// 计算近 7 天「活跃小时」均速，并给出当前 burn 的相对倍数。
fn compute_baseline(
    facts: &[MergedRequestFact],
    now_epoch: i64,
    current_tokens_per_hour: f64,
) -> Option<Baseline> {
    let cutoff = now_epoch - BASELINE_LOOKBACK_SECONDS;

    // 按「小时桶」累计 token，只统计有活动的小时，避免被大量空闲时间稀释。
    use std::collections::HashMap;
    let mut hour_buckets: HashMap<i64, u64> = HashMap::new();
    for fact in facts.iter().rev() {
        if fact.timestamp_sec < cutoff {
            break;
        }
        let hour_index = fact.timestamp_sec / 3600;
        *hour_buckets.entry(hour_index).or_insert(0) += fact.total_tokens;
    }

    let active_hours: Vec<u64> = hour_buckets.values().copied().filter(|&t| t > 0).collect();
    if active_hours.len() < BASELINE_MIN_ACTIVE_HOURS {
        return None;
    }

    let sum: u64 = active_hours.iter().sum();
    let avg_tokens_per_hour = sum as f64 / active_hours.len() as f64;

    let relative_to_baseline = if avg_tokens_per_hour > 0.0 && current_tokens_per_hour > 0.0 {
        Some(current_tokens_per_hour / avg_tokens_per_hour)
    } else {
        None
    };

    Some(Baseline {
        avg_tokens_per_hour,
        relative_to_baseline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小事实，只填本测试关心的字段。
    fn fact(ts: i64, tokens: u64) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("k{ts}"),
            session_id: "s".into(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".into(),
            timestamp_sec: ts,
            timestamp_ms: ts * 1000,
            model: "claude".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: tokens,
            request_count: 1,
            estimated_cost: 0.0,
            coverage_origin: crate::unified_usage::CoverageOrigin::LocalOnly,
            status_code: None,
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            source_label: None,
        }
    }

    #[test]
    fn block_anchors_to_first_request_and_counts_usage() {
        let now = 100_000;
        // 块起点在 now-3600（1 小时前），块内两条请求。
        let facts = vec![fact(now - 3600, 1000), fact(now - 600, 500)];
        let block = compute_current_block(&facts, now, 0.0).expect("active block");
        assert_eq!(block.start_epoch, now - 3600);
        assert_eq!(block.resets_at_epoch, now - 3600 + BLOCK_SECONDS);
        assert_eq!(block.used_tokens, 1500);
        assert_eq!(block.used_requests, 2);
        assert_eq!(block.remaining_seconds, BLOCK_SECONDS - 3600);
    }

    #[test]
    fn idle_over_5h_has_no_active_block() {
        let now = 1_000_000;
        // 最后一条在 6 小时前 → 当前块已过期。
        let facts = vec![fact(now - 6 * 3600, 1000)];
        assert!(compute_current_block(&facts, now, 0.0).is_none());
    }

    #[test]
    fn gap_over_5h_starts_new_block() {
        let now = 1_000_000;
        // 旧块在很久以前，新块在 30 分钟前（间隔 > 5h）。
        let facts = vec![
            fact(now - 10 * 3600, 9999),
            fact(now - 1800, 200),
            fact(now - 600, 300),
        ];
        let block = compute_current_block(&facts, now, 0.0).expect("active block");
        assert_eq!(block.start_epoch, now - 1800);
        // 旧块那条 9999 不应计入当前块。
        assert_eq!(block.used_tokens, 500);
        assert_eq!(block.used_requests, 2);
    }

    #[test]
    fn long_session_rolls_into_new_block_after_5h() {
        let now = 1_000_000;
        // 连续活动跨越 5h：t0 与 t0+5h 应分属不同块。
        let t0 = now - 5 * 3600 - 600; // 略早于 5h10m 前
        let facts = vec![
            fact(t0, 100),
            fact(t0 + 3600, 100),
            fact(t0 + BLOCK_SECONDS, 700), // 达到 5h → 新块起点
            fact(now - 60, 50),
        ];
        let block = compute_current_block(&facts, now, 0.0).expect("active block");
        assert_eq!(block.start_epoch, t0 + BLOCK_SECONDS);
        assert_eq!(block.used_tokens, 750);
    }

    #[test]
    fn burn_rate_and_confidence() {
        let now = 100_000;
        // 10 条请求均匀分布在最近 60 分钟。
        let mut facts = Vec::new();
        for i in 0..10 {
            facts.push(fact(now - 3600 + i * 360, 600));
        }
        let burn = compute_burn_rate(&facts, now);
        assert_eq!(burn.sample_requests, 10);
        // 6000 tokens / 1h（span ≈ 3600−... 取首条到 now）
        assert!(burn.tokens_per_hour > 0.0);
        assert_eq!(burn.confidence, Confidence::High);
    }

    #[test]
    fn burn_rate_low_confidence_when_sparse() {
        let now = 100_000;
        let facts = vec![fact(now - 120, 500)];
        let burn = compute_burn_rate(&facts, now);
        assert_eq!(burn.sample_requests, 1);
        assert_eq!(burn.confidence, Confidence::Low);
    }

    #[test]
    fn burn_rate_empty_sample() {
        let now = 100_000;
        // 最近一条在 3 小时前，超出 90 分钟取样窗。
        let facts = vec![fact(now - 3 * 3600, 500)];
        let burn = compute_burn_rate(&facts, now);
        assert_eq!(burn.sample_requests, 0);
        assert_eq!(burn.tokens_per_hour, 0.0);
    }

    #[test]
    fn baseline_requires_min_active_hours() {
        let now = 1_000_000;
        // 仅 2 个活跃小时 → 不足以产出基线。
        let facts = vec![fact(now - 4000, 100), fact(now - 8000, 100)];
        assert!(compute_baseline(&facts, now, 100.0).is_none());
    }

    #[test]
    fn baseline_relative_multiple() {
        let now = 1_000_000;
        // 3 个不同小时，每小时各 1000 token → 均速 1000/h。
        let facts = vec![
            fact(now - 3600 * 2, 1000),
            fact(now - 3600 * 4, 1000),
            fact(now - 3600 * 6, 1000),
        ];
        let baseline = compute_baseline(&facts, now, 2000.0).expect("baseline");
        assert_eq!(baseline.avg_tokens_per_hour, 1000.0);
        // 当前 2000/h 相对均速 1000/h = 2.0×
        assert_eq!(baseline.relative_to_baseline, Some(2.0));
    }

    #[test]
    fn build_snapshot_sets_source_kind() {
        let now = 1_000_000;
        let facts = vec![
            fact(now - 3600 * 2, 1000),
            fact(now - 3600 * 4, 1000),
            fact(now - 3600 * 6, 1000),
            fact(now - 600, 500),
        ];
        let snap = build_limit_survival(&facts, now);
        assert_eq!(snap.source_kind, SurvivalSourceKind::Baseline);
        assert!(snap.block.is_some());
    }

    #[test]
    fn build_snapshot_none_when_insufficient() {
        let now = 1_000_000;
        let facts = vec![fact(now - 600, 500)];
        let snap = build_limit_survival(&facts, now);
        assert_eq!(snap.source_kind, SurvivalSourceKind::None);
    }
}
