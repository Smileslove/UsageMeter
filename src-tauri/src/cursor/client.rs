use super::events::{digest, parse_json_event, CursorEvent, MAX_EVENTS, MAX_IMPORT_BYTES};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::StreamExt;
use rusqlite::{types::ValueRef, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub(crate) struct CursorAuth {
    pub account_key: String,
    pub subject: String,
    pub user_id: String,
    cookie: reqwest::header::HeaderValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorFailure {
    pub code: String,
    pub next_retry_ms: Option<i64>,
}
impl From<&str> for CursorFailure {
    fn from(code: &str) -> Self {
        Self {
            code: code.into(),
            next_retry_ms: None,
        }
    }
}
impl From<String> for CursorFailure {
    fn from(code: String) -> Self {
        Self {
            code,
            next_retry_ms: None,
        }
    }
}

pub(crate) fn read_auth(path: &Path, now_sec: i64) -> Result<CursorAuth, CursorFailure> {
    if !path.exists() {
        return Err("cursor_not_found".into());
    }
    let conn = crate::session::cursor_reader::open_readonly(path)?;
    let token: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key='cursorAuth/accessToken'",
            [],
            |row| match row.get_ref(0)? {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Ok(bytes.to_vec()),
                ValueRef::Null => Ok(Vec::new()),
                _ => Err(rusqlite::Error::InvalidColumnType(
                    0,
                    "value".into(),
                    row.get_ref(0)?.data_type(),
                )),
            },
        )
        .optional()
        .map_err(|_| "cursor_schema_unsupported")?;
    let bytes = token
        .filter(|bytes| !bytes.is_empty())
        .ok_or("cursor_not_signed_in")?;
    if bytes.len() > 32768 {
        return Err("cursor_invalid_credentials".into());
    }
    let text = if bytes.contains(&0) {
        if bytes.len() % 2 != 0 {
            return Err("cursor_invalid_credentials".into());
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| "cursor_invalid_credentials")?
    } else {
        String::from_utf8(bytes).map_err(|_| "cursor_invalid_credentials")?
    };
    auth_from_token(text.trim().trim_start_matches('\u{feff}'), now_sec)
}

fn auth_from_token(token: &str, now_sec: i64) -> Result<CursorAuth, CursorFailure> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
    {
        return Err("cursor_invalid_credentials".into());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| "cursor_invalid_credentials")?;
    let claims: Value =
        serde_json::from_slice(&payload).map_err(|_| "cursor_invalid_credentials")?;
    let expires_at = claims["exp"].as_i64().ok_or("cursor_invalid_credentials")?;
    if expires_at <= now_sec.saturating_add(60) {
        return Err("cursor_expired".into());
    }
    let subject = claims["sub"]
        .as_str()
        .filter(|subject| !subject.is_empty() && subject.len() <= 256)
        .ok_or("cursor_invalid_credentials")?;
    let user_id = subject
        .strip_prefix("auth0|")
        .filter(|id| id.starts_with("user_"))
        .unwrap_or(subject);
    let known = user_id.starts_with("user_")
        || ["google-oauth2|", "github|", "oidc|"]
            .iter()
            .any(|prefix| user_id.starts_with(prefix));
    if !known
        || !user_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-|".contains(&byte))
    {
        return Err("cursor_identity_unsupported".into());
    }
    let encoded = user_id.replace('|', "%7C");
    let mut cookie = reqwest::header::HeaderValue::from_str(&format!(
        "WorkosCursorSessionToken={encoded}%3A%3A{token}"
    ))
    .map_err(|_| "cursor_invalid_credentials")?;
    cookie.set_sensitive(true);
    Ok(CursorAuth {
        account_key: digest(format!("cursor:{user_id}").as_bytes()),
        subject: subject.into(),
        user_id: user_id.into(),
        cookie,
    })
}

pub(crate) struct CursorClient {
    client: reqwest::Client,
    #[cfg(test)]
    events_endpoint: Option<String>,
}
impl CursorClient {
    pub fn new() -> Self {
        // Factory's long client disables redirects, preserving the fixed credential boundary.
        Self {
            client: crate::net::HttpClientFactory::global().long(),
            #[cfg(test)]
            events_endpoint: None,
        }
    }

    async fn read_response(
        response: reqwest::Response,
        byte_budget: usize,
    ) -> Result<Vec<u8>, CursorFailure> {
        let status = response.status();
        if status.as_u16() == 429 {
            let retry_seconds = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|header| header.to_str().ok())
                .and_then(|text| {
                    text.parse::<i64>()
                        .ok()
                        .filter(|seconds| *seconds >= 0)
                        .or_else(|| {
                            chrono::DateTime::parse_from_rfc2822(text).ok().map(|time| {
                                (time.timestamp() - chrono::Utc::now().timestamp()).max(0)
                            })
                        })
                })
                .unwrap_or(300)
                .min(24 * 60 * 60);
            return Err(CursorFailure {
                code: "cursor_rate_limited".into(),
                next_retry_ms: Some(
                    chrono::Utc::now()
                        .timestamp_millis()
                        .saturating_add(retry_seconds * 1000),
                ),
            });
        }
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 => "cursor_expired",
                403 => "cursor_permission_denied",
                500..=599 => "cursor_service_unavailable",
                _ => "cursor_schema_unsupported",
            }
            .into());
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > byte_budget as u64)
        {
            return Err("cursor_response_too_large".into());
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "cursor_network_error")?;
            if chunk.len() > byte_budget.saturating_sub(bytes.len()) {
                return Err("cursor_response_too_large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub async fn fetch_summary(&self, auth: &CursorAuth) -> Result<Value, CursorFailure> {
        let response = self
            .client
            .get("https://cursor.com/api/usage-summary")
            .header(reqwest::header::COOKIE, auth.cookie.clone())
            .header(reqwest::header::ORIGIN, "https://cursor.com")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| "cursor_network_error")?;
        let bytes = Self::read_response(response, 4 * 1024 * 1024).await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "cursor_schema_unsupported")?;
        if !value.is_object() {
            return Err("cursor_schema_unsupported".into());
        }
        Ok(value)
    }

    pub async fn fetch_events(
        &self,
        auth: &CursorAuth,
        start_ms: i64,
        end_ms: i64,
        manual: bool,
    ) -> Result<Vec<CursorEvent>, CursorFailure> {
        let operation = async {
            let mut bytes_remaining = MAX_IMPORT_BYTES;
            let mut pages_remaining = 200;
            let first = self
                .fetch_window(
                    auth,
                    start_ms,
                    end_ms,
                    &mut bytes_remaining,
                    &mut pages_remaining,
                )
                .await;
            match first {
                Ok(events) => Ok(events),
                Err(failure)
                    if matches!(
                        failure.code.as_str(),
                        "cursor_ambiguous_duplicate_page" | "cursor_sync_count_changed"
                    ) && end_ms - start_ms > 2 * 86_400_000 =>
                {
                    // One bounded retry on two disjoint UTC windows. Both must be complete
                    // before the caller can publish; retries share the original budgets.
                    let middle = ((start_ms + (end_ms - start_ms) / 2) / 86_400_000) * 86_400_000;
                    let mut events = self
                        .fetch_window(
                            auth,
                            start_ms,
                            middle,
                            &mut bytes_remaining,
                            &mut pages_remaining,
                        )
                        .await?;
                    events.extend(
                        self.fetch_window(
                            auth,
                            middle,
                            end_ms,
                            &mut bytes_remaining,
                            &mut pages_remaining,
                        )
                        .await?,
                    );
                    if events.len() > MAX_EVENTS {
                        return Err("cursor_response_too_large".into());
                    }
                    super::events::occurrence_keys(&events)?;
                    Ok(events)
                }
                Err(failure) => Err(failure),
            }
        };
        tokio::time::timeout(
            Duration::from_secs(if manual { 120 } else { 60 }),
            operation,
        )
        .await
        .map_err(|_| CursorFailure::from("cursor_sync_timeout"))?
    }
    async fn fetch_window(
        &self,
        auth: &CursorAuth,
        start_ms: i64,
        end_ms: i64,
        bytes_remaining: &mut usize,
        pages_remaining: &mut usize,
    ) -> Result<Vec<CursorEvent>, CursorFailure> {
        let mut pagination = Pagination::new();
        for page in 1..=200 {
            if *pages_remaining == 0 || *bytes_remaining == 0 {
                return Err("cursor_sync_incomplete".into());
            }
            *pages_remaining -= 1;
            let endpoint = "https://cursor.com/api/dashboard/get-filtered-usage-events";
            #[cfg(test)]
            let endpoint = self.events_endpoint.as_deref().unwrap_or(endpoint);
            let response=self.client.post(endpoint)
                .header(reqwest::header::COOKIE,auth.cookie.clone()).header(reqwest::header::ORIGIN,"https://cursor.com")
                .timeout(Duration::from_secs(15)).json(&json!({"teamId":0,"page":page,"pageSize":500,"startDate":start_ms,"endDate":end_ms}))
                .send().await.map_err(|_|"cursor_network_error")?;
            let bytes =
                Self::read_response(response, (*bytes_remaining).min(4 * 1024 * 1024)).await?;
            *bytes_remaining = bytes_remaining.saturating_sub(bytes.len());
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| "cursor_schema_unsupported")?;
            if pagination.add_page(&value, auth, start_ms, end_ms)? {
                return Ok(pagination.events);
            }
        }
        Err("cursor_sync_incomplete".into())
    }
}

struct Pagination {
    total: Option<usize>,
    events: Vec<CursorEvent>,
    page_signatures: std::collections::HashSet<String>,
}
impl Pagination {
    fn new() -> Self {
        Self {
            total: None,
            events: Vec::new(),
            page_signatures: std::collections::HashSet::new(),
        }
    }
    fn add_page(
        &mut self,
        value: &Value,
        auth: &CursorAuth,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<bool, CursorFailure> {
        if value.get("error").is_some() || value.get("errorMessage").is_some() {
            return Err("cursor_schema_unsupported".into());
        }
        let rows = value["usageEventsDisplay"]
            .as_array()
            .ok_or("cursor_schema_unsupported")?;
        let total = value["totalUsageEventsCount"]
            .as_u64()
            .filter(|count| *count <= MAX_EVENTS as u64)
            .ok_or("cursor_schema_unsupported")? as usize;
        if self.total.is_some_and(|expected| expected != total) {
            return Err("cursor_sync_count_changed".into());
        }
        self.total = Some(total);
        if rows.len() > 500
            || self.events.len() + rows.len() > total
            || (rows.is_empty() && self.events.len() < total)
        {
            return Err("cursor_sync_incomplete".into());
        }
        let signature = digest(&serde_json::to_vec(rows).map_err(|_| "cursor_schema_unsupported")?);
        if !rows.is_empty() && !self.page_signatures.insert(signature) {
            return Err("cursor_ambiguous_duplicate_page".into());
        }
        for row in rows {
            if let Some(owner) = row["owningUser"].as_str() {
                if owner != auth.user_id && owner != auth.subject {
                    return Err("cursor_identity_mismatch".into());
                }
            } else if !row["owningUser"].is_null() {
                return Err("cursor_identity_unsupported".into());
            }
            if !row["owningTeam"].is_null() && row["owningTeam"].as_i64() != Some(0) {
                return Err("cursor_team_scope_unsupported".into());
            }
            let event = parse_json_event(row)?;
            if event.timestamp_ms < start_ms || event.timestamp_ms >= end_ms {
                return Err("cursor_range_unsupported".into());
            }
            self.events.push(event);
        }
        Ok(self.events.len() == total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token(subject: &str, expiry: i64) -> String {
        format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({"sub":subject,"exp":expiry})).unwrap())
        )
    }
    #[test]
    fn cursor_auth_is_stable_across_refresh_and_does_not_expose_credentials() {
        let first = auth_from_token(&token("auth0|user_test", 1000), 0).unwrap();
        let refreshed = auth_from_token(&token("user_test", 2000), 0).unwrap();
        assert_eq!(first.account_key, refreshed.account_key);
        assert!(first.cookie.is_sensitive());
        assert!(auth_from_token(&token("user_test", 59), 0).is_err());
        let oauth = auth_from_token(&token("google-oauth2|123", 2000), 0).unwrap();
        assert_eq!(oauth.user_id, "google-oauth2|123");
        assert!(oauth
            .cookie
            .to_str()
            .unwrap()
            .contains("google-oauth2%7C123"));
        assert!(auth_from_token(&token("unknown|123", 2000), 0).is_err());
    }
    #[test]
    fn cursor_pagination_rejects_incomplete_drifting_or_ambiguous_pages() {
        let auth = auth_from_token(&token("user_test", 2000), 0).unwrap();
        let row = json!({"timestamp":1750000000001_i64,"owningUser":"user_test"});
        let page = json!({"usageEventsDisplay":[row.clone(),row],"totalUsageEventsCount":2});
        let mut pagination = Pagination::new();
        assert!(pagination
            .add_page(&page, &auth, 1750000000000, 1750000000010)
            .unwrap());
        assert_eq!(pagination.events.len(), 2);
        assert!(Pagination::new()
            .add_page(&json!({}), &auth, 0, i64::MAX)
            .is_err());
        assert!(Pagination::new()
            .add_page(
                &json!({"usageEventsDisplay":[],"totalUsageEventsCount":2}),
                &auth,
                0,
                i64::MAX
            )
            .is_err());
        let page = json!({"usageEventsDisplay":[{"timestamp":1750000000001_i64}],"totalUsageEventsCount":2});
        let mut pagination = Pagination::new();
        assert!(!pagination.add_page(&page, &auth, 0, i64::MAX).unwrap());
        assert!(pagination.add_page(&page, &auth, 0, i64::MAX).is_err());
        let drift = json!({"usageEventsDisplay":[{"timestamp":1750000000002_i64}],"totalUsageEventsCount":3});
        assert!(pagination.add_page(&drift, &auth, 0, i64::MAX).is_err());
        let wrong = json!({"usageEventsDisplay":[{"timestamp":1750000000001_i64,"owningUser":"user_other"}],"totalUsageEventsCount":1});
        assert!(Pagination::new()
            .add_page(&wrong, &auth, 0, i64::MAX)
            .is_err());
        let mut repeated = Pagination::new();
        let first = json!({"usageEventsDisplay":[{"timestamp":1750000000001_i64}],"totalUsageEventsCount":3});
        let second = json!({"usageEventsDisplay":[{"timestamp":1750000000002_i64}],"totalUsageEventsCount":3});
        assert!(!repeated.add_page(&first, &auth, 0, i64::MAX).unwrap());
        assert!(!repeated.add_page(&second, &auth, 0, i64::MAX).unwrap());
        assert_eq!(
            repeated
                .add_page(&first, &auth, 0, i64::MAX)
                .unwrap_err()
                .code,
            "cursor_ambiguous_duplicate_page"
        );
    }

    #[tokio::test]
    async fn cursor_window_retry_requires_both_halves_and_never_returns_partial_pages() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let start = 20_000 * 86_400_000_i64;
        let end = start + 4 * 86_400_000;
        let middle = start + 2 * 86_400_000;
        let row = |time| json!({"timestamp":time,"owningUser":"user_test"});
        let page = |time, count| {
            json!({"usageEventsDisplay":[row(time)],"totalUsageEventsCount":count}).to_string()
        };
        // The endpoint override exists only in test builds; production always uses cursor.com.
        async fn server(
            replies: Vec<(u16, String)>,
        ) -> (CursorClient, tokio::task::JoinHandle<Vec<Value>>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let mut requests = Vec::new();
                for (status, body) in replies {
                    let (mut stream, _) =
                        tokio::time::timeout(Duration::from_secs(5), listener.accept())
                            .await
                            .unwrap()
                            .unwrap();
                    let mut bytes = Vec::new();
                    loop {
                        let mut buffer = [0_u8; 4096];
                        let size = stream.read(&mut buffer).await.unwrap();
                        assert!(size > 0);
                        bytes.extend_from_slice(&buffer[..size]);
                        assert!(bytes.len() <= 8192);
                        if let Some(header_end) =
                            bytes.windows(4).position(|part| part == b"\r\n\r\n")
                        {
                            let headers = String::from_utf8_lossy(&bytes[..header_end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap();
                            if bytes.len() >= header_end + 4 + length {
                                requests.push(
                                    serde_json::from_slice(
                                        &bytes[header_end + 4..header_end + 4 + length],
                                    )
                                    .unwrap(),
                                );
                                break;
                            }
                        }
                    }
                    let wire = format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                    stream.write_all(wire.as_bytes()).await.unwrap();
                }
                requests
            });
            (
                CursorClient {
                    client: reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .unwrap(),
                    events_endpoint: Some(format!(
                        "http://{address}/api/dashboard/get-filtered-usage-events"
                    )),
                },
                task,
            )
        }
        let auth = auth_from_token(&token("user_test", 2000), 0).unwrap();
        for (second, split_status, expected) in [
            (page(start + 1, 2), 200, None),
            (page(start + 2, 3), 200, None),
            (page(start + 1, 2), 503, Some("cursor_service_unavailable")),
        ] {
            let (client, task) = server(vec![
                (200, page(start + 1, 2)),
                (200, second),
                (200, page(start + 1, 1)),
                (split_status, page(middle + 1, 1)),
            ])
            .await;
            let result = client.fetch_events(&auth, start, end, true).await;
            if let Some(code) = expected {
                assert_eq!(result.unwrap_err().code, code);
            } else {
                let events = result.unwrap();
                assert_eq!(events.len(), 2);
                assert_eq!(events[0].timestamp_ms, start + 1);
                assert_eq!(events[1].timestamp_ms, middle + 1);
            }
            let requests = task.await.unwrap();
            for (request, (page_number, from, to)) in requests.iter().zip([
                (1, start, end),
                (2, start, end),
                (1, start, middle),
                (1, middle, end),
            ]) {
                assert_eq!(request["page"], page_number);
                assert_eq!(request["pageSize"], 500);
                assert_eq!(request["teamId"], 0);
                assert_eq!(request["startDate"], from);
                assert_eq!(request["endDate"], to);
            }
        }
        for (second_status, requested_end, expected) in [
            (503, end, "cursor_service_unavailable"),
            (200, middle, "cursor_ambiguous_duplicate_page"),
        ] {
            let (client, task) = server(vec![
                (200, page(start + 1, 2)),
                (second_status, page(start + 1, 2)),
            ])
            .await;
            assert_eq!(
                client
                    .fetch_events(&auth, start, requested_end, true)
                    .await
                    .unwrap_err()
                    .code,
                expected
            );
            assert_eq!(task.await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn cursor_http_errors_and_byte_limits_preserve_safe_codes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn response(status: &str, headers: &str, body: &str) -> reqwest::Response {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let wire = format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len());
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
                stream.write_all(wire.as_bytes()).await.unwrap();
            });
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap()
        }
        for (status, expected) in [
            ("401 Unauthorized", "cursor_expired"),
            ("403 Forbidden", "cursor_permission_denied"),
            ("503 Unavailable", "cursor_service_unavailable"),
            ("302 Found", "cursor_schema_unsupported"),
        ] {
            assert_eq!(
                CursorClient::read_response(response(status, "", "SECRET response").await, 1024)
                    .await
                    .unwrap_err()
                    .code,
                expected
            );
        }
        let rate = CursorClient::read_response(
            response(
                "429 Too Many Requests",
                "Retry-After: 600\r\n",
                "SECRET response",
            )
            .await,
            1024,
        )
        .await
        .unwrap_err();
        assert_eq!(rate.code, "cursor_rate_limited");
        assert!(rate.next_retry_ms.unwrap() >= chrono::Utc::now().timestamp_millis() + 590_000);
        assert!(!serde_json::to_string(&rate).unwrap().contains("SECRET"));
        assert_eq!(
            CursorClient::read_response(response("200 OK", "", "0123456789").await, 4)
                .await
                .unwrap_err()
                .code,
            "cursor_response_too_large"
        );
    }
}
