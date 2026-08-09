use reqwest::Url;

pub const USAGEMETER_PATH_PREFIX: &str = "/usagemeter";

/// 新版短路径前缀：`/umg/{tool}/s/{handle_id}/{suffix}`
pub const UMG_PATH_PREFIX: &str = "/umg";

fn is_local_proxy_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]"
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

    let path = url.path();
    // 先剥掉 tool 前缀，再在剩余段内定界找 marker（新 `/s/`、旧 `/source/`），
    // 避免 suffix 里出现 `/s/` 段时误提取。
    let rest = tool_prefixes.iter().find_map(|tool_prefix| {
        [
            format!("{UMG_PATH_PREFIX}/{tool_prefix}"),
            format!("{USAGEMETER_PATH_PREFIX}/{tool_prefix}"),
            format!("/{tool_prefix}"),
        ]
        .iter()
        .find_map(|root| path.strip_prefix(root))
    })?;
    let marker = if rest.starts_with("/s/") {
        "/s/"
    } else if rest.starts_with("/source/") {
        "/source/"
    } else {
        return None;
    };
    let source_id = rest[marker.len()..]
        .split('/')
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .trim();

    if source_id.is_empty() {
        return None;
    }
    // 旧格式 URL 中可能是十进制旧 handle id（或 16 位旧 source id），归一为
    // 8 位新格式，保证与注册表（读取时已归一）等值匹配；非 handle 前缀原样返回。
    Some(crate::proxy::normalize_handle_id(source_id))
}

pub fn prefixed_proxy_url(
    proxy_port: u16,
    tool_prefix: &str,
    source_id: &str,
    suffix: &str,
) -> String {
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        format!("http://127.0.0.1:{proxy_port}{UMG_PATH_PREFIX}/{tool_prefix}/s/{source_id}")
    } else {
        format!(
            "http://127.0.0.1:{proxy_port}{UMG_PATH_PREFIX}/{tool_prefix}/s/{source_id}/{suffix}"
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
        let short_root = format!("{UMG_PATH_PREFIX}/{tool_prefix}");
        let legacy_root = format!("/{tool_prefix}");
        path == new_root
            || path.starts_with(&format!("{new_root}/source/"))
            || path.starts_with(&format!("{new_root}/provider/"))
            || path == short_root
            || path.starts_with(&format!("{short_root}/s/"))
            || path == legacy_root
            || path.starts_with(&format!("{legacy_root}/source/"))
            || path.starts_with(&format!("{legacy_root}/provider/"))
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
        assert_eq!(url, "http://127.0.0.1:18765/umg/codex/s/src_123/v1");
        assert!(is_usagemeter_proxy_url(&url, &["codex"]));
        assert!(is_usagemeter_proxy_url_for_port(&url, 18765, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(&url, &["codex"]).as_deref(),
            Some("src_123")
        );
    }

    #[test]
    fn short_umg_proxy_url_detected() {
        let url = "http://127.0.0.1:18765/umg/codex/s/h_a3x9kq2/v1";
        assert!(is_usagemeter_proxy_url(url, &["codex"]));
        assert!(is_usagemeter_proxy_url_for_port(url, 18765, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("h_a3x9kq2")
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

    #[test]
    fn long_usagemeter_proxy_url_still_detected() {
        let url = "http://127.0.0.1:18765/usagemeter/codex/source/h_old_long_id_16hex/v1";
        assert!(is_usagemeter_proxy_url(url, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("h_old_long_id_16hex")
        );
    }

    #[test]
    fn legacy_decimal_handle_id_normalized_on_extract() {
        // 旧格式 URL 中的十进制 handle id 提取后归一为 8 位 hex，与注册表匹配。
        let url = "http://127.0.0.1:18765/usagemeter/codex/source/h_183789673287381192/v1";
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("h_028cf3bb")
        );
        // 非 handle 前缀（src_* / 自定义）原样返回
        let url2 = "http://127.0.0.1:18765/umg/codex/s/src_custom/v1";
        assert_eq!(
            extract_source_id_from_proxy_url(url2, &["codex"]).as_deref(),
            Some("src_custom")
        );
    }

    #[test]
    fn suffix_with_s_segment_does_not_confuse_marker() {
        // 旧格式 suffix 含 /s/ 段时，marker 仍在 tool 前缀之后定界，不被误伤。
        let url = "http://127.0.0.1:18765/usagemeter/codex/source/h_183789673287381192/v1/s/extra";
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("h_028cf3bb")
        );
    }

    #[test]
    fn ipv6_loopback_proxy_url_is_detected() {
        let url = "http://[::1]:18765/usagemeter/codex/source/src_ipv6/v1";
        assert!(is_usagemeter_proxy_url(url, &["codex"]));
        assert!(is_usagemeter_proxy_url_for_port(url, 18765, &["codex"]));
        assert_eq!(
            extract_source_id_from_proxy_url(url, &["codex"]).as_deref(),
            Some("src_ipv6")
        );
    }
}
