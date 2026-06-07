//! Google Generative Language API 路径分类。
//!
//! Gemini CLI 走 `https://generativelanguage.googleapis.com`，模型写在路径里，
//! 端点形如：
//! - `POST /v1beta/models/{model}:generateContent`
//! - `POST /v1beta/models/{model}:streamGenerateContent`（通常带 `?alt=sse`）
//! - `POST /v1beta/models/{model}:countTokens`（不计 usage）
//!
//! 这里只识别会产生 usageMetadata 的生成端点，并从路径里抽取模型名。

use hyper::Method;

fn strip_query(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

pub(super) fn is_gemini_generate_path(path: &str) -> bool {
    let path = strip_query(path);
    path.contains(":generateContent") || path.contains(":streamGenerateContent")
}

pub(super) fn is_gemini_streaming_path(path: &str) -> bool {
    strip_query(path).contains(":streamGenerateContent")
}

pub(super) fn is_gemini_endpoint(path: &str, method: &Method) -> bool {
    *method == Method::POST && is_gemini_generate_path(path)
}

/// 从 `.../models/{model}:generateContent` 中抽取模型名。
pub(super) fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let path = strip_query(path);
    let after_models = path.split("models/").nth(1)?;
    let model = after_models
        .split(':')
        .next()
        .unwrap_or("")
        .trim_matches('/')
        .trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_generate_endpoints() {
        assert!(is_gemini_generate_path(
            "/v1beta/models/gemini-2.5-pro:generateContent"
        ));
        assert!(is_gemini_generate_path(
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        ));
        assert!(is_gemini_streaming_path(
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        ));
        assert!(!is_gemini_streaming_path(
            "/v1beta/models/gemini-2.5-pro:generateContent"
        ));
        assert!(!is_gemini_generate_path(
            "/v1beta/models/gemini-2.5-pro:countTokens"
        ));
        assert!(!is_gemini_generate_path("/v1beta/models"));
    }

    #[test]
    fn endpoint_requires_post() {
        assert!(is_gemini_endpoint(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            &Method::POST
        ));
        assert!(!is_gemini_endpoint(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            &Method::GET
        ));
    }

    #[test]
    fn extracts_model_from_path() {
        assert_eq!(
            extract_gemini_model_from_path("/v1beta/models/gemini-2.5-pro:generateContent")
                .as_deref(),
            Some("gemini-2.5-pro")
        );
        assert_eq!(
            extract_gemini_model_from_path(
                "/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
            )
            .as_deref(),
            Some("gemini-2.5-flash")
        );
        assert_eq!(extract_gemini_model_from_path("/v1beta/models"), None);
    }
}
