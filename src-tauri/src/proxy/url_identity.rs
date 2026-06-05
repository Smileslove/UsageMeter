use reqwest::Url;

pub const USAGEMETER_PATH_PREFIX: &str = "/usagemeter";

fn is_local_proxy_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "127.0.0.1" || host == "localhost"
}

fn normalized_path(url: &Url) -> &str {
    url.path().trim_end_matches('/')
}

pub fn is_usagemeter_proxy_url(base_url: &str, tool_prefixes: &[&str]) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    is_usagemeter_proxy_url_from_url(&url, tool_prefixes)
}

pub fn is_usagemeter_proxy_url_for_port(
    base_url: &str,
    proxy_port: u16,
    tool_prefixes: &[&str],
) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    if url.port() != Some(proxy_port) {
        return false;
    }
    is_usagemeter_proxy_url_from_url(&url, tool_prefixes)
}

pub fn extract_source_id_from_proxy_url(base_url: &str, tool_prefixes: &[&str]) -> Option<String> {
    let Ok(url) = Url::parse(base_url) else {
        return None;
    };
    if !is_usagemeter_proxy_url_from_url(&url, tool_prefixes) {
        return None;
    }

    let marker = "/source/";
    let path = url.path();
    let marker_index = path.find(marker)?;
    let rest = &path[(marker_index + marker.len())..];
    let source_id = rest
        .split('/')
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .trim();

    (!source_id.is_empty()).then(|| source_id.to_string())
}

pub fn prefixed_proxy_url(
    proxy_port: u16,
    tool_prefix: &str,
    source_id: &str,
    suffix: &str,
) -> String {
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        format!(
            "http://127.0.0.1:{proxy_port}{USAGEMETER_PATH_PREFIX}/{tool_prefix}/source/{source_id}"
        )
    } else {
        format!(
            "http://127.0.0.1:{proxy_port}{USAGEMETER_PATH_PREFIX}/{tool_prefix}/source/{source_id}/{suffix}"
        )
    }
}

fn is_usagemeter_proxy_url_from_url(url: &Url, tool_prefixes: &[&str]) -> bool {
    if !is_local_proxy_host(url) {
        return false;
    }

    let path = normalized_path(url);
    tool_prefixes.iter().any(|tool_prefix| {
        let new_root = format!("{USAGEMETER_PATH_PREFIX}/{tool_prefix}");
        let legacy_root = format!("/{tool_prefix}");
        path == new_root
            || path.starts_with(&format!("{new_root}/source/"))
            || path == legacy_root
            || path.starts_with(&format!("{legacy_root}/source/"))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        extract_source_id_from_proxy_url, is_usagemeter_proxy_url,
        is_usagemeter_proxy_url_for_port, prefixed_proxy_url,
    };

    #[test]
    fn new_prefixed_proxy_url_round_trips() {
        let url = prefixed_proxy_url(18765, "codex", "src_123", "v1");
        assert_eq!(
            url,
            "http://127.0.0.1:18765/usagemeter/codex/source/src_123/v1"
        );
        assert!(is_usagemeter_proxy_url(&url, &["codex"]));
        assert!(is_usagemeter_proxy_url_for_port(&url, 18765, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(&url, &["codex"]).as_deref(),
            Some("src_123")
        );
    }

    #[test]
    fn legacy_proxy_url_still_detected() {
        let url = "http://127.0.0.1:18765/codex/source/src_legacy/v1";
        assert!(is_usagemeter_proxy_url(url, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("src_legacy")
        );
    }
}
