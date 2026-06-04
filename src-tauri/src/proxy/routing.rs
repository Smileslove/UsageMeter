use super::forwarder::RequestForwarder;
use super::handlers::{claude, codex, opencode, reasonix};
use super::request_common::{
    append_query, detect_client_route, full, get_settings_snapshot, HandlerResult,
};
use super::types::ProxyState;
use hyper::{Method, Request, Response, StatusCode};
use std::sync::Arc;

pub(crate) async fn handle_request(
    req: Request<hyper::body::Incoming>,
    forwarder: Arc<RequestForwarder>,
    state: Arc<ProxyState>,
) -> HandlerResult {
    let method = req.method().clone();
    let raw_path = req.uri().path().to_string();
    let raw_query = req.uri().query().map(str::to_string);
    let settings = get_settings_snapshot(&state).await;
    let client_route = detect_client_route(&raw_path, &settings);
    let path = client_route.normalized_path.clone();
    let forward_path = append_query(&path, raw_query.as_deref());

    {
        let mut status = state.status.write().await;
        status.total_requests += 1;
    }

    if path == "/health" && method == Method::GET {
        let status = state.status.read().await.clone();
        let body = serde_json::to_string(&status).unwrap_or_default();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full(body))
            .unwrap());
    }

    if path == "/status" && method == Method::GET {
        let status = state.status.read().await.clone();
        let body = serde_json::to_string(&status).unwrap_or_default();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full(body))
            .unwrap());
    }

    match client_route.client_tool.as_str() {
        "codex" => codex::handle_codex_request(method, &path, &forward_path, client_route, req, &state).await,
        "opencode" => {
            opencode::handle_opencode_request(
                method,
                &path,
                &forward_path,
                client_route,
                req,
                forwarder,
                &state,
            )
            .await
        }
        "claude_code" => {
            claude::handle_claude_request(
                method,
                &path,
                &forward_path,
                client_route,
                req,
                forwarder,
                &state,
            )
            .await
        }
        "reasonix" => {
            reasonix::handle_reasonix_request(
                method,
                &path,
                &forward_path,
                client_route,
                req,
                forwarder,
                &state,
            )
            .await
        }
        "unknown" => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(full(
                r#"{"error":{"type":"proxy_route_not_matched","message":"Request path did not match any enabled proxy route"}}"#,
            ))
            .unwrap()),
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(full(
                r#"{"error":{"type":"not_found","message":"Endpoint not found"}}"#,
            ))
            .unwrap()),
    }
}
