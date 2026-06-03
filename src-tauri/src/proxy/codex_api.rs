use hyper::Method;

pub(super) fn canonical_codex_api_path(path: &str) -> &str {
    let path = path.trim_start_matches('/');
    path.strip_prefix("v1/v1/").unwrap_or(path)
}

pub(super) fn is_codex_api_path(path: &str) -> bool {
    let path = canonical_codex_api_path(path);
    path == "v1/chat/completions"
        || path == "chat/completions"
        || path == "v1/responses"
        || path == "responses"
        || path == "v1/responses/compact"
        || path == "responses/compact"
        || path.starts_with("v1/responses/")
        || path.starts_with("responses/")
}

pub(super) fn is_codex_endpoint(path: &str, method: &Method) -> bool {
    *method == Method::POST && is_codex_api_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_endpoint_detection_uses_shared_path_classifier() {
        assert!(is_codex_api_path("/v1/responses"));
        assert!(is_codex_api_path("/v1/v1/responses"));
        assert!(is_codex_api_path("/responses/resp_123"));
        assert!(is_codex_endpoint("/v1/v1/chat/completions", &Method::POST));
        assert!(!is_codex_endpoint("/v1/responses", &Method::GET));
        assert!(!is_codex_api_path("/v1/models"));
    }
}
