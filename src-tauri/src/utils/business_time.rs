use crate::models::AppSettings;
use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone};

pub const DAY_BOUNDARY_MODE_STANDARD: &str = "standard";
pub const DAY_BOUNDARY_MODE_NIGHT_OWL: &str = "night_owl";
pub const STANDARD_DAY_START_HOUR: u32 = 0;
pub const NIGHT_OWL_DAY_START_HOUR: u32 = 4;

pub fn default_day_boundary_mode() -> String {
    DAY_BOUNDARY_MODE_STANDARD.to_string()
}

pub fn normalize_day_boundary_mode(value: &str) -> String {
    match value.trim() {
        DAY_BOUNDARY_MODE_NIGHT_OWL => DAY_BOUNDARY_MODE_NIGHT_OWL.to_string(),
        _ => DAY_BOUNDARY_MODE_STANDARD.to_string(),
    }
}

pub fn business_day_start_hour(settings: &AppSettings) -> u32 {
    match settings.day_boundary_mode.as_str() {
        DAY_BOUNDARY_MODE_NIGHT_OWL => NIGHT_OWL_DAY_START_HOUR,
        _ => STANDARD_DAY_START_HOUR,
    }
}

pub fn current_business_date(settings: &AppSettings) -> String {
    business_date_for_timestamp(Local::now().timestamp(), settings)
}

pub fn business_date_for_timestamp(timestamp_sec: i64, settings: &AppSettings) -> String {
    let dt = Local
        .timestamp_opt(timestamp_sec, 0)
        .single()
        .unwrap_or_else(Local::now);
    let shifted = dt - Duration::hours(business_day_start_hour(settings) as i64);
    shifted.format("%Y-%m-%d").to_string()
}

pub fn business_date_for_timestamp_ms(timestamp_ms: i64, settings: &AppSettings) -> String {
    business_date_for_timestamp(timestamp_ms / 1000, settings)
}

pub fn business_date_epoch_bounds(
    business_date: &str,
    settings: &AppSettings,
) -> Result<(i64, i64), String> {
    let date = parse_business_date(business_date)?;
    let next_date = date
        .succ_opt()
        .ok_or_else(|| format!("Invalid business date `{business_date}`"))?;
    let start = resolve_business_day_boundary(date, settings, business_date)?;
    let next_label = next_date.format("%Y-%m-%d").to_string();
    let end = resolve_business_day_boundary(next_date, settings, &next_label)?;
    Ok((start, end))
}

pub fn current_business_day_start_epoch(settings: &AppSettings) -> i64 {
    let today = current_business_date(settings);
    business_date_epoch_bounds(&today, settings)
        .map(|(start, _)| start)
        .unwrap_or_else(|_| Local::now().timestamp())
}

pub fn current_business_month_start_epoch(settings: &AppSettings) -> i64 {
    let current_date = parse_business_date(&current_business_date(settings))
        .unwrap_or_else(|_| Local::now().date_naive());
    let month_start = current_date.with_day(1).unwrap_or(current_date);
    resolve_business_day_boundary(month_start, settings, "current_month")
        .unwrap_or_else(|_| Local::now().timestamp())
}

pub fn enumerate_business_dates(
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Vec<String> {
    if end_epoch <= start_epoch {
        return Vec::new();
    }

    let mut dates = Vec::new();
    let mut current = parse_business_date(&business_date_for_timestamp(start_epoch, settings))
        .unwrap_or_else(|_| Local::now().date_naive());
    let end_date = parse_business_date(&business_date_for_timestamp(
        end_epoch.saturating_sub(1),
        settings,
    ))
    .unwrap_or_else(|_| Local::now().date_naive());

    while current <= end_date {
        dates.push(current.format("%Y-%m-%d").to_string());
        let Some(next) = current.succ_opt() else {
            break;
        };
        current = next;
    }

    dates
}

pub fn business_window_cutoff_epoch(window: &str, settings: &AppSettings) -> i64 {
    let now = Local::now();
    match window {
        "today" => current_business_day_start_epoch(settings),
        "current_month" => current_business_month_start_epoch(settings),
        "5h" => (now - Duration::hours(5)).timestamp(),
        "24h" => (now - Duration::hours(24)).timestamp(),
        "7d" => (now - Duration::days(7)).timestamp(),
        "30d" => (now - Duration::days(30)).timestamp(),
        _ => (now - Duration::hours(24)).timestamp(),
    }
}

fn parse_business_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|e| format!("Invalid business date `{value}`: {e}"))
}

fn resolve_business_day_boundary(
    date: NaiveDate,
    settings: &AppSettings,
    label: &str,
) -> Result<i64, String> {
    let hour = business_day_start_hour(settings);
    for minute in 0..60 {
        let Some(naive) = date.and_hms_opt(hour, minute, 0) else {
            continue;
        };
        match Local.from_local_datetime(&naive) {
            LocalResult::Single(dt) => return Ok(dt.timestamp()),
            LocalResult::Ambiguous(earliest, _) => return Ok(earliest.timestamp()),
            LocalResult::None => continue,
        }
    }

    Err(format!(
        "Failed to resolve business day boundary for `{label}` in local timezone"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: &str) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.day_boundary_mode = mode.to_string();
        settings
    }

    #[test]
    fn night_owl_business_date_shifts_before_four_am() {
        let settings = settings(DAY_BOUNDARY_MODE_NIGHT_OWL);
        let ts = Local
            .with_ymd_and_hms(2026, 6, 12, 1, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(business_date_for_timestamp(ts, &settings), "2026-06-11");
    }

    #[test]
    fn standard_business_date_keeps_same_date() {
        let settings = settings(DAY_BOUNDARY_MODE_STANDARD);
        let ts = Local
            .with_ymd_and_hms(2026, 6, 12, 1, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(business_date_for_timestamp(ts, &settings), "2026-06-12");
    }

    #[test]
    fn night_owl_bounds_start_at_four_am() {
        let settings = settings(DAY_BOUNDARY_MODE_NIGHT_OWL);
        let (start, end) = business_date_epoch_bounds("2026-06-11", &settings).unwrap();
        assert_eq!(
            Local
                .timestamp_opt(start, 0)
                .single()
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "2026-06-11 04:00:00"
        );
        assert_eq!(
            Local
                .timestamp_opt(end, 0)
                .single()
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "2026-06-12 04:00:00"
        );
    }
}
