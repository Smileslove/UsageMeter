use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub(crate) const MAX_IMPORT_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_EVENTS: usize = 100_000;

/// A whitelist of billing fields, never a serialized API payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorEvent {
    pub external_id: Option<String>,
    pub conversation_id: Option<String>,
    pub timestamp_ms: i64,
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub usage_cost_usd: Option<f64>,
    pub charged_amount_usd: Option<f64>,
    pub kind: Option<String>,
}

impl CursorEvent {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.output_tokens.unwrap_or(0)
            + self.cache_write_tokens.unwrap_or(0)
            + self.cache_read_tokens.unwrap_or(0)
    }

    pub fn usage_complete(&self) -> bool {
        self.input_tokens.is_some()
            && self.output_tokens.is_some()
            && self.cache_write_tokens.is_some()
            && self.cache_read_tokens.is_some()
    }

    /// Keeps identical billable rows with distinct occurrence keys. Caller publishes a
    /// complete snapshot/window, so corrections replace rather than append generations.
    pub fn signature(&self) -> String {
        digest(&serde_json::to_vec(self).expect("finite validated Cursor event"))
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn occurrence_keys(events: &[CursorEvent]) -> Result<Vec<String>, String> {
    let mut occurrences = HashMap::<String, usize>::new();
    let mut external_ids = HashSet::new();
    events
        .iter()
        .map(|event| {
            if let Some(id) = &event.external_id {
                if !external_ids.insert(id) {
                    return Err("cursor_duplicate_event_id".into());
                }
                return Ok(format!("id:{}", digest(id.as_bytes())));
            }
            let signature = event.signature();
            let occurrence = occurrences.entry(signature.clone()).or_default();
            *occurrence += 1;
            Ok(format!("{signature}:{occurrence}"))
        })
        .collect()
}

fn bounded_string(value: &Value) -> Result<Option<String>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or("cursor_schema_unsupported")?;
    if text.len() > 512 || text.chars().any(char::is_control) {
        return Err("cursor_schema_unsupported".into());
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

fn token(value: &Value) -> Result<Option<u64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let count = value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|count| *count <= (i64::MAX as u64) / 4)
        .ok_or("cursor_invalid_token_count")?;
    Ok(Some(count))
}

fn money(value: &Value, cents: bool) -> Result<Option<f64>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let amount = value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 1e12)
        .ok_or("cursor_invalid_amount")?;
    Ok(Some(if cents { amount / 100.0 } else { amount }))
}

pub(crate) fn parse_json_event(value: &Value) -> Result<CursorEvent, String> {
    if !value.is_object() {
        return Err("cursor_schema_unsupported".into());
    }
    let timestamp_ms = value["timestamp"]
        .as_i64()
        .or_else(|| value["timestamp"].as_str()?.parse().ok())
        .filter(|value| *value > 0 && DateTime::from_timestamp_millis(*value).is_some())
        .ok_or("cursor_invalid_timestamp")?;
    let usage = &value["tokenUsage"];
    if !usage.is_null() && !usage.is_object() {
        return Err("cursor_schema_unsupported".into());
    }
    let event = CursorEvent {
        external_id: bounded_string(&value["eventId"])?,
        conversation_id: bounded_string(&value["conversationId"])?,
        timestamp_ms,
        model: bounded_string(&value["model"])?.unwrap_or_else(|| "unknown".into()),
        input_tokens: token(&usage["inputTokens"])?,
        output_tokens: token(&usage["outputTokens"])?,
        cache_write_tokens: token(&usage["cacheWriteTokens"])?,
        cache_read_tokens: token(&usage["cacheReadTokens"])?,
        usage_cost_usd: money(&usage["totalCents"], true)?,
        charged_amount_usd: money(&value["chargedCents"], true)?,
        kind: bounded_string(&value["kind"])?,
    };
    if event.usage_complete()
        && !usage["totalTokens"].is_null()
        && token(&usage["totalTokens"])? != Some(event.total_tokens())
    {
        return Err("cursor_csv_token_total_mismatch".into());
    }
    Ok(event)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CsvPreview {
    pub content_hash: String,
    pub event_count: usize,
    pub first_timestamp_ms: Option<i64>,
    pub last_timestamp_ms: Option<i64>,
    pub total_tokens: u64,
    pub usage_cost_usd: Option<f64>,
    pub unknown_cost_events: usize,
    pub cache_write_basis: String,
}

pub(crate) fn parse_csv(bytes: &[u8]) -> Result<(CsvPreview, Vec<CursorEvent>), String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("cursor_import_too_large".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(bytes);
    let headers = reader.headers().map_err(|_| "cursor_invalid_csv")?.clone();
    let mut columns = HashMap::new();
    for (index, header) in headers.iter().enumerate() {
        if columns.insert(header, index).is_some() {
            return Err("cursor_duplicate_csv_header".into());
        }
    }
    for required in [
        "Date",
        "Model",
        "Input (w/ Cache Write)",
        "Input (w/o Cache Write)",
        "Cache Read",
        "Output Tokens",
        "Total Tokens",
    ] {
        if !columns.contains_key(required) {
            return Err("cursor_schema_unsupported".into());
        }
    }
    let mut events = Vec::new();
    let mut with_write = Vec::new();
    let mut independent_valid = true;
    let mut cumulative_valid = true;
    for row in reader.records() {
        let row = row.map_err(|_| "cursor_invalid_csv")?;
        if events.len() == MAX_EVENTS {
            return Err("cursor_import_too_large".into());
        }
        let field = |name: &str| {
            columns
                .get(name)
                .and_then(|index| row.get(*index))
                .unwrap_or("")
        };
        let count = |name: &str| -> Result<u64, String> {
            token(&Value::String(field(name).replace(',', "")))?
                .ok_or_else(|| "cursor_invalid_token_count".into())
        };
        let timestamp_ms = DateTime::parse_from_rfc3339(field("Date"))
            .map_err(|_| "cursor_invalid_timestamp")?
            .timestamp_millis();
        if timestamp_ms <= 0 {
            return Err("cursor_invalid_timestamp".into());
        }
        let input = count("Input (w/o Cache Write)")?;
        let cache_write = count("Input (w/ Cache Write)")?;
        let cache_read = count("Cache Read")?;
        let output = count("Output Tokens")?;
        let total = count("Total Tokens")?;
        independent_valid &= total == input + cache_write + cache_read + output;
        cumulative_valid &= cache_write >= input && total == cache_write + cache_read + output;
        let csv_money = |text: &str| -> Result<Option<f64>, String> {
            if text.is_empty()
                || ["included", "included in pro", "free", "no charge", "-"]
                    .contains(&text.to_ascii_lowercase().as_str())
            {
                return Ok(None);
            }
            money(
                &Value::String(text.trim_start_matches('$').replace(',', "")),
                false,
            )
        };
        events.push(CursorEvent {
            external_id: None,
            conversation_id: bounded_string(&Value::String(field("Conversation ID").into()))?,
            timestamp_ms,
            model: bounded_string(&Value::String(field("Model").into()))?
                .ok_or("cursor_schema_unsupported")?,
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_write_tokens: Some(cache_write),
            cache_read_tokens: Some(cache_read),
            usage_cost_usd: csv_money(field("Cost"))?,
            charged_amount_usd: csv_money(field("Cost to you"))?,
            kind: bounded_string(&Value::String(field("Kind").into()))?,
        });
        with_write.push(cache_write);
    }
    if !independent_valid && !cumulative_valid {
        return Err("cursor_csv_token_total_mismatch".into());
    }
    // Select one schema interpretation for the entire file; never guess row by row.
    let cache_write_basis = if independent_valid {
        "independent"
    } else {
        "cumulative"
    };
    if !independent_valid {
        for (event, cache_write) in events.iter_mut().zip(with_write) {
            event.cache_write_tokens = Some(cache_write - event.input_tokens.unwrap_or(0));
        }
    }
    let known_costs: Vec<_> = events
        .iter()
        .filter_map(|event| event.usage_cost_usd)
        .collect();
    let total_tokens = events
        .iter()
        .try_fold(0u64, |total, event| total.checked_add(event.total_tokens()))
        .ok_or("cursor_invalid_token_count")?;
    Ok((
        CsvPreview {
            content_hash: digest(bytes),
            event_count: events.len(),
            first_timestamp_ms: events.iter().map(|event| event.timestamp_ms).min(),
            last_timestamp_ms: events.iter().map(|event| event.timestamp_ms).max(),
            total_tokens,
            usage_cost_usd: (!known_costs.is_empty()).then(|| known_costs.iter().sum()),
            unknown_cost_events: events.len() - known_costs.len(),
            cache_write_basis: cache_write_basis.into(),
        },
        events,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cursor_json_preserves_unknowns_and_distinct_money() {
        let event = parse_json_event(&json!({"timestamp":"1750000000123", "conversationId":"c1", "model":"auto", "chargedCents":5,
            "tokenUsage":{"inputTokens":10,"outputTokens":20,"cacheReadTokens":0,"cacheWriteTokens":3,"totalCents":25}})).unwrap();
        assert_eq!(event.total_tokens(), 33);
        assert_eq!(event.usage_cost_usd, Some(0.25));
        assert_eq!(event.charged_amount_usd, Some(0.05));
        assert!(event.usage_complete());
        let unknown =
            parse_json_event(&json!({"timestamp":1750000000123_i64,"chargedCents":0})).unwrap();
        assert_eq!(unknown.input_tokens, None);
        assert_eq!(unknown.usage_cost_usd, None);
        assert!(!unknown.usage_complete());
        assert!(parse_json_event(
            &json!({"timestamp":1750000000123_i64,"tokenUsage":{"inputTokens":-1}})
        )
        .is_err());
        assert!(parse_json_event(&json!({"timestamp":0})).is_err());
    }

    #[test]
    fn cursor_csv_header_mapping_quoting_bom_and_cache_schema() {
        let csv = "\u{feff}Model,Date,Output Tokens,Cache Read,Input (w/o Cache Write),Input (w/ Cache Write),Total Tokens,Cost,Kind,Extra\n\"claude,sonnet\",2025-06-15T15:06:40.123+00:00,20,4,10,3,37,$0.25,Included,\"two\nlines\"\n";
        let (preview, events) = parse_csv(csv.as_bytes()).unwrap();
        assert_eq!(preview.total_tokens, 37);
        assert_eq!(preview.cache_write_basis, "independent");
        assert_eq!(events[0].model, "claude,sonnet");
        assert_eq!(events[0].timestamp_ms, 1750000000123);
        let cumulative = csv.replace(",10,3,37,", ",10,13,37,");
        let (preview, events) = parse_csv(cumulative.as_bytes()).unwrap();
        assert_eq!(preview.cache_write_basis, "cumulative");
        assert_eq!(events[0].cache_write_tokens, Some(3));
        assert!(parse_csv(csv.replace(",37,", ",38,").as_bytes()).is_err());
        assert!(parse_csv(csv.replace("+00:00", "").as_bytes()).is_err());
    }

    #[test]
    fn cursor_identical_usage_events_retain_multiplicity() {
        let event = parse_json_event(&json!({"timestamp":1750000000123_i64})).unwrap();
        let keys = occurrence_keys(&[event.clone(), event]).unwrap();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
    }

    #[test]
    fn cursor_free_csv_rows_keep_tokens_without_inventing_cash_cost() {
        let csv = "Date,Model,Input (w/ Cache Write),Input (w/o Cache Write),Cache Read,Output Tokens,Total Tokens,Cost\n2025-06-15T15:06:40Z,auto,3,10,4,20,37,Included in Pro\n";
        let (preview, events) = parse_csv(csv.as_bytes()).unwrap();
        assert_eq!(preview.total_tokens, 37);
        assert_eq!(preview.unknown_cost_events, 1);
        assert_eq!(events[0].usage_cost_usd, None);
    }
}
