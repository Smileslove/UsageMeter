//! 内容脱敏工具（纯函数，持久化之前与 on-demand 读取回传之前都必须经过）。
//!
//! 规则刻意保守：宁可多脱敏不可泄露（设计文档 15.1 P3/P4 分类）。
//! 本模块不允许输出任何原始 secret —— 所有规则只做“替换为占位符”，
//! 不保留被替换值的任何片段。

use serde_json::Value;

/// 常见 secret 值占位符。
const REDACTED: &str = "[redacted]";

/// 需要整值脱敏的 key-value 字段名（大小写不敏感，支持 `key=value` 与 `key: value`）。
const SECRET_KEYS: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "x-api-key",
    "x_api_key",
    "access_token",
    "access-token",
    "accesstoken",
    "auth_token",
    "auth-token",
    "authtoken",
    "refresh_token",
    "refresh-token",
    "client_secret",
    "client-secret",
    "clientsecret",
    "secret",
    "password",
    "passwd",
    "pwd",
    "token",
    "private_key",
    "private-key",
    "privatekey",
    "authorization",
    "proxy_authorization",
    "proxy-authorization",
];

fn normalized_secret_key(key: &str) -> String {
    key.chars()
        .filter(|ch| !matches!(ch, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_structured_secret_key(key: &str) -> bool {
    matches!(
        normalized_secret_key(key).as_str(),
        "apikey"
            | "xapikey"
            | "accesstoken"
            | "authorization"
            | "proxyauthorization"
            | "authtoken"
            | "refreshtoken"
            | "clientsecret"
            | "secret"
            | "password"
            | "passwd"
            | "pwd"
            | "token"
            | "privatekey"
    )
}

/// Recursively redact values under secret JSON object keys. Return `None` when
/// no key was changed so ordinary JSON/text keeps its original representation.
fn redact_structured_json(text: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(text.trim()).ok()?;
    let mut changed = false;
    fn visit(value: &mut Value, changed: &mut bool) {
        match value {
            Value::Object(object) => {
                for (key, child) in object.iter_mut() {
                    if is_structured_secret_key(key) {
                        *child = Value::String(REDACTED.to_string());
                        *changed = true;
                    } else {
                        visit(child, changed);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| visit(item, changed)),
            _ => {}
        }
    }
    visit(&mut value, &mut changed);
    changed
        .then(|| serde_json::to_string(&value).ok())
        .flatten()
}

/// 词边界判断：ASCII 字母/数字/下划线视为词内字符（用于 `sk-`、`Bearer`、
/// key 名的前后边界，避免把 `ask-why`、`myBearer`、`api_keyX` 误伤）。
fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// key token 字符：key 名/API key 值中常见字符。
fn is_key_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'+' | b'=')
}

fn utf8_char_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

/// 脱敏 Authorization / Proxy-Authorization 头值（`Header: Bearer xxx` 整值替换）。
fn redact_auth_headers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed_start = line.trim_start();
        let lower = trimmed_start.to_ascii_lowercase();
        let header = ["authorization", "proxy-authorization"]
            .iter()
            .find(|h| lower.starts_with(**h))
            .copied();
        if let Some(header_name) = header {
            let after_name = &lower[header_name.len()..];
            // `:` 与可选空白后即为敏感值；保留原行缩进与键名大小写。
            if let Some(colon_rel) = after_name.find(':') {
                let colon_abs = line.len() - trimmed_start.len() + header_name.len() + colon_rel;
                out.push_str(&line[..=colon_abs]);
                out.push(' ');
                out.push_str(REDACTED);
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
        if lines.peek().is_some() {
            out.push('\n');
        }
    }
    out
}

/// 脱敏 `sk-` 前缀 API key（Anthropic/OpenAI 风格：`sk-ant-...`、`sk-abc...`）。
fn redact_sk_keys(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        // 查找 "sk-" 出现位置；前一个字符不能是词内字符（避免 "ask-xxx" 误伤，
        // 但允许 `=`/`:`/空白等分隔符，如 "key=sk-xxx"）。
        if bytes[i..].starts_with(b"sk-") && (i == 0 || !is_word_char(bytes[i - 1])) {
            let mut j = i + 3;
            let mut token_len = 0usize;
            while j < bytes.len() && is_key_token_char(bytes[j]) {
                j += 1;
                token_len += 1;
            }
            // 保守阈值：至少 8 个 token 字符才视为 key；短词（如 "sk-why"）不脱敏。
            if token_len >= 8 {
                out.push_str("sk-");
                out.push_str(REDACTED);
                i = j;
                continue;
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// 脱敏具有稳定厂商前缀的独立 token。保留前缀便于用户判断凭据类型，
/// 但不保留任何 token 内容。
fn redact_known_prefixed_tokens(text: &str) -> String {
    const PREFIXES: &[(&str, usize)] = &[
        ("github_pat_", 20),
        ("ghp_", 20),
        ("gho_", 20),
        ("ghu_", 20),
        ("ghs_", 20),
        ("ghr_", 20),
        ("AIza", 20),
        ("AKIA", 16),
        ("ASIA", 16),
    ];
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let mut matched = false;
        for (prefix, min_tail) in PREFIXES {
            if bytes[i..].starts_with(prefix.as_bytes()) && (i == 0 || !is_word_char(bytes[i - 1]))
            {
                let mut end = i + prefix.len();
                let aws_access_key = matches!(*prefix, "AKIA" | "ASIA");
                while end < bytes.len()
                    && if aws_access_key {
                        bytes[end].is_ascii_uppercase() || bytes[end].is_ascii_digit()
                    } else {
                        bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b'-')
                    }
                {
                    end += 1;
                }
                let tail_len = end.saturating_sub(i + prefix.len());
                let valid = if aws_access_key {
                    tail_len == *min_tail && (end == bytes.len() || !is_word_char(bytes[end]))
                } else {
                    tail_len >= *min_tail
                };
                if valid {
                    out.push_str(prefix);
                    out.push_str(REDACTED);
                    i = end;
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn is_jwt_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// JWT 的三段均为 base64url。只匹配常见的 JSON header（`eyJ`）并要求
/// 三段都有合理长度，避免把版本号或普通点分标识误判成凭据。
fn redact_jwts(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"eyJ") && (i == 0 || !is_word_char(bytes[i - 1])) {
            let mut end = i;
            while end < bytes.len() && is_jwt_char(bytes[end]) {
                end += 1;
            }
            let token = &text[i..end];
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() == 3 && parts.iter().all(|part| part.len() >= 8) {
                out.push_str(REDACTED);
                i = end;
                continue;
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// 私钥 PEM 块必须整体移除；只替换主体会泄露密钥类型和尾部内容。
fn redact_private_key_pem(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(begin) = rest.find("-----BEGIN ") else {
            out.push_str(rest);
            break;
        };
        let header_end = rest[begin..]
            .find('\n')
            .map(|relative| begin + relative)
            .unwrap_or(rest.len());
        let header = &rest[begin..header_end];
        if !header.contains("PRIVATE KEY") {
            out.push_str(&rest[..header_end]);
            rest = &rest[header_end..];
            continue;
        }
        out.push_str(&rest[..begin]);
        out.push_str(REDACTED);
        let end_marker_start = rest[header_end..]
            .find("-----END ")
            .map(|relative| header_end + relative);
        let Some(end_marker_start) = end_marker_start else {
            break;
        };
        let end_marker_end = rest[end_marker_start..]
            .find('\n')
            .map(|relative| end_marker_start + relative + 1)
            .unwrap_or(rest.len());
        rest = &rest[end_marker_end..];
    }
    out
}

/// `Bearer <token>`（任意位置；token 为连续非空白字符且长度 ≥ 8）。
fn redact_bearer_tokens(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut search_from = 0;
    let mut copied_until = 0;
    while let Some(rel) = lower[search_from..].find("bearer") {
        let abs = search_from + rel;
        // 必须是独立词：前后不能是词内字符。
        let prev_ok = abs == 0 || !is_word_char(bytes[abs - 1]);
        let after = abs + "bearer".len();
        let next_ok = after >= text.len() || !is_word_char(bytes[after]);
        if prev_ok && next_ok {
            // 取 Bearer 后首个非空白到下一个空白/分隔符之间的 token。
            let mut t_start = after;
            while t_start < text.len() && bytes[t_start].is_ascii_whitespace() {
                t_start += 1;
            }
            let mut t_end = t_start;
            while t_end < text.len()
                && !bytes[t_end].is_ascii_whitespace()
                && !matches!(bytes[t_end], b',' | b')' | b']' | b'"' | b'\'')
            {
                t_end += 1;
            }
            if t_end.saturating_sub(t_start) >= 8 {
                out.push_str(&text[copied_until..t_start]);
                out.push_str(REDACTED);
                copied_until = t_end;
                search_from = t_end;
                continue;
            }
        }
        search_from = abs + "bearer".len();
    }
    out.push_str(&text[copied_until..]);
    out
}

/// 查找下一个 secret 键名位置（键名前/后必须是词边界）。
fn find_secret_key(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        for key in SECRET_KEYS {
            if lower[i..].starts_with(key) {
                let key_end = i + key.len();
                let prev_ok = i == 0 || !is_word_char(bytes[i - 1]);
                let next_ok = key_end >= bytes.len() || !is_word_char(bytes[key_end]);
                if prev_ok && next_ok {
                    return Some(i);
                }
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        i += ch_len;
    }
    None
}

/// `key=value` / `key: value` 形式的 secret 字段（键名大小写不敏感，值整段替换）。
fn redact_secret_kv(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = find_secret_key(rest) {
        let bytes = rest.as_bytes();
        // 键名结束：只吞键名字符（字母数字 + `_`/`-`），不吞 `=`/`:` 分隔符
        // 与值（is_key_token_char 含 `=`/`:`/`.`/`/` 等，不能用于此处，否则
        // 会把分隔符与值一并吞掉导致分隔符判定失败）。
        let mut k_end = pos;
        while k_end < bytes.len()
            && (bytes[k_end].is_ascii_alphanumeric() || matches!(bytes[k_end], b'_' | b'-'))
        {
            k_end += 1;
        }
        let mut after = k_end;
        // 允许 `=` 或 `:`（前后可有空白）。
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        if after >= bytes.len() || !matches!(bytes[after], b'=' | b':') {
            // 该 key 出现不是 kv 形式（如 `my_token is`）：保留字符后继续找。
            out.push_str(&rest[..pos + 1]);
            rest = &rest[pos + 1..];
            continue;
        }
        after += 1;
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        // 值结束：行尾、空白或 `,` `&` `\r`（保守：只替换单行内连续值）。
        let mut v_end = after;
        while v_end < bytes.len() && !bytes[v_end].is_ascii_whitespace() {
            if matches!(bytes[v_end], b',' | b'&' | b'\r') {
                break;
            }
            v_end += 1;
        }
        if v_end > after {
            out.push_str(&rest[..after]); // 键 + 分隔符 + 空白（保留原样）
            out.push_str(REDACTED);
            rest = &rest[v_end..];
            continue;
        }
        out.push_str(&rest[..pos + 1]);
        rest = &rest[pos + 1..];
    }
    out.push_str(rest);
    out
}

/// 脱敏常见 API key / Bearer token / Authorization 头值。
///
/// 覆盖：`Authorization`/`Proxy-Authorization` 头整值、`Bearer <token>`、
/// `sk-` 前缀 key、以及 `key=value`/`key: value` 形式的 secret 字段。
///
/// 顺序说明：`Bearer` 必须在 `sk-` 之前（`Bearer sk-xxx` 整 token 替换，
/// 避免 sk 规则先替换后 Bearer 扫描把 `[redacted]` 的 `]` 当终止符）；
/// kv 规则最后运行，把已替换占位符也整段覆盖（`api_key=sk-[redacted]` →
/// `api_key=[redacted]`）。
pub fn redact_secret(text: &str) -> String {
    let structured = redact_structured_json(text);
    let text = structured.as_deref().unwrap_or(text);
    let text = redact_auth_headers(text);
    let text = redact_private_key_pem(&text);
    let text = redact_bearer_tokens(&text);
    let text = redact_sk_keys(&text);
    let text = redact_known_prefixed_tokens(&text);
    let text = redact_jwts(&text);
    redact_secret_kv(&text)
}

/// 脱敏 URL query 参数值（`?a=1&b=2` → `?a=[redacted]&b=[redacted]`），
/// 无值的参数（`?flag`）保留。只处理首个 `?` 后的 query 段（保守规则）。
pub fn redact_url_query(url: &str) -> String {
    let Some(q_pos) = url.find('?') else {
        return url.to_string();
    };
    let head = &url[..q_pos];
    let rest = &url[q_pos + 1..];
    // 截断 fragment（`#...` 不属于 query）。
    let (query, fragment) = match rest.find('#') {
        Some(f) => (&rest[..f], &rest[f..]),
        None => (rest, ""),
    };
    let mut out = String::with_capacity(url.len());
    out.push_str(head);
    out.push('?');
    let mut first = true;
    for param in query.split('&') {
        if !first {
            out.push('&');
        }
        first = false;
        if param.is_empty() {
            continue;
        }
        match param.split_once('=') {
            Some((key, _)) => {
                out.push_str(key);
                out.push_str("=[redacted]");
            }
            None => out.push_str(param),
        }
    }
    out.push_str(fragment);
    out
}

/// 把 home 目录前缀替换为 `~`（P2 敏感元数据：路径显示时脱敏用户名）。
///
/// 支持文本任意位置的 home 前缀（如 `cwd=/Users/bob/app` → `cwd=~/app`），
/// 替换前校验边界：home 后必须是路径分隔符/行尾/空白/引号等常见边界，
/// home 前不能是路径字符（避免 `/not/Users/bob`、`/Users/bob2` 误伤）。
pub fn redact_home_path(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    let home_str = home.to_string_lossy();
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut search_from = 0;
    let mut rest = path;
    while let Some(rel) = rest.find(home_str.as_ref()) {
        let abs = search_from + rel;
        let after = abs + home_str.len();
        let boundary_ok = after >= path.len()
            || matches!(
                bytes[after],
                // `/` 与 `\`（Windows 风格 `C:\Users\name\...`）都是路径分隔符；
                // 其余为空白/引号/括号/分隔符等常见边界。
                b'/' | b'\\'
                    | b' '
                    | b'\t'
                    | b'\n'
                    | b'\r'
                    | b'"'
                    | b'\''
                    | b')'
                    | b']'
                    | b','
                    | b';'
                    | b'&'
                    | b'?'
            );
        let prev_ok = abs == 0
            || !matches!(
                bytes[abs - 1],
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'
            );
        if prev_ok && boundary_ok {
            out.push_str(&path[search_from..abs]);
            out.push('~');
            search_from = after;
            rest = &path[search_from..];
            continue;
        }
        // 边界校验拒绝：保留原文（含被拒匹配片段，避免推进时丢字符）。
        // 推进位置必须对齐 UTF-8 字符边界——home 前缀含非 ASCII（如
        // `/Users/张三`）且本次匹配被拒时，`abs + 1` 可能落在多字节字符
        // 内部，直接 `&path[search_from..]` 切片会 panic（此函数在查询
        // 出口每行调用，崩溃直达 IPC）。
        let mut next = abs + 1;
        while next < path.len() && !path.is_char_boundary(next) {
            next += 1;
        }
        out.push_str(&path[search_from..next]);
        search_from = next;
        rest = &path[search_from..];
    }
    out.push_str(&path[search_from..]);
    out
}

/// 通用脱敏：secret → URL query → home 路径（供事件摘要与 payload 使用）。
pub fn redact_text(text: &str) -> String {
    let text = redact_secret(text);
    let text = redact_url_query(&text);
    redact_home_path(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_authorization_header_value() {
        assert_eq!(
            redact_secret("Authorization: Bearer sk-ant-api03-abcdefgh123456"),
            "Authorization: [redacted]"
        );
        assert_eq!(
            redact_secret("proxy-authorization: Basic dXNlcjpwYXNz"),
            "proxy-authorization: [redacted]"
        );
        // 大小写不敏感，保留原键名
        assert_eq!(
            redact_secret("AUTHORIZATION: token abcdefgh123456"),
            "AUTHORIZATION: [redacted]"
        );
        // 多行文本中逐行处理
        let input = "line1\nAuthorization: Bearer abcdefgh12345678\nline3";
        assert_eq!(
            redact_secret(input),
            "line1\nAuthorization: [redacted]\nline3"
        );
    }

    #[test]
    fn redacts_sk_prefix_keys() {
        assert_eq!(
            redact_secret("key=sk-ant-api03-1234567890abcdef"),
            "key=sk-[redacted]"
        );
        assert_eq!(redact_secret("sk-abcdefgh12345678"), "sk-[redacted]");
        assert_eq!(
            redact_secret("Authorization: Bearer sk-ant-abcdefgh123456"),
            "Authorization: [redacted]"
        );
        // 短 token 不脱敏（避免 "ask-why" 之类误伤）
        assert_eq!(redact_secret("ask-why not"), "ask-why not");
        assert_eq!(redact_secret("sk-short"), "sk-short");
    }

    #[test]
    fn redacts_bearer_tokens_in_text() {
        assert_eq!(
            redact_secret("use Bearer abcdefgh12345678 please"),
            "use Bearer [redacted] please"
        );
        assert_eq!(
            redact_secret("x=Bearer sk-ant-abcdefgh123456"),
            "x=Bearer [redacted]"
        );
    }

    #[test]
    fn redacts_common_standalone_credentials() {
        let github = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";
        let google = "AIzaabcdefghijklmnopqrstuvwxyz1234567890";
        let aws = "AKIA1234567890ABCDEF";
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefgh12345678";
        let input = format!("{github}\n{google}\n{aws}\n{jwt}");
        let output = redact_secret(&input);
        assert!(!output.contains(github));
        assert!(!output.contains(google));
        assert!(!output.contains(aws));
        assert!(!output.contains(jwt));
        assert!(output.contains("ghp_[redacted]"));
        assert!(output.contains("AIza[redacted]"));
        assert!(output.contains("AKIA[redacted]"));
    }

    #[test]
    fn redacts_private_key_pem_as_one_block() {
        let input = "before\n-----BEGIN PRIVATE KEY-----\nsecret-material\n-----END PRIVATE KEY-----\nafter";
        let output = redact_secret(input);
        assert_eq!(output, "before\n[redacted]after");
        assert!(!output.contains("secret-material"));
    }

    #[test]
    fn does_not_redact_short_or_similar_identifiers() {
        assert_eq!(redact_secret("ghp_short"), "ghp_short");
        assert_eq!(
            redact_secret("AKIA-not-an-access-key"),
            "AKIA-not-an-access-key"
        );
        assert_eq!(redact_secret("eyJ.part.two"), "eyJ.part.two");
    }

    #[test]
    fn redacts_secret_kv_fields() {
        assert_eq!(
            redact_secret("api_key=sk-abcdefgh12345678"),
            "api_key=[redacted]"
        );
        assert_eq!(
            redact_secret("API_KEY: abcdefgh12345678"),
            "API_KEY: [redacted]"
        );
        assert_eq!(
            redact_secret("x-api-key: 1234567890abcdef"),
            "x-api-key: [redacted]"
        );
        assert_eq!(
            redact_secret("password=supersecret123"),
            "password=[redacted]"
        );
        assert_eq!(
            redact_secret("access_token=abcdefgh12345678&x=1"),
            "access_token=[redacted]&x=1"
        );
        // 非 secret 字段不受影响
        assert_eq!(
            redact_secret("model=claude-sonnet-4-5"),
            "model=claude-sonnet-4-5"
        );
        assert_eq!(redact_secret("filename=notes.txt"), "filename=notes.txt");
        // 键名大小写不敏感，允许 `=` 两侧空白
        assert_eq!(
            redact_secret("ApiKey = abcdefgh12345678"),
            "ApiKey = [redacted]"
        );
        assert_eq!(
            redact_secret("authorization=secret-header"),
            "authorization=[redacted]"
        );
    }

    #[test]
    fn redacts_nested_json_secret_fields_without_leaking_values() {
        let input = r#"{"apiKey":"secret-api","nested":{"accessToken":"secret-token"},"headers":{"Authorization":"secret-auth"},"items":[{"password":"secret-pass"}],"model":"safe"}"#;
        let output = redact_secret(input);
        assert!(!output.contains("secret-api"));
        assert!(!output.contains("secret-token"));
        assert!(!output.contains("secret-auth"));
        assert!(!output.contains("secret-pass"));
        assert!(output.contains(r#""apiKey":"[redacted]""#));
        assert!(output.contains(r#""accessToken":"[redacted]""#));
        assert!(output.contains(r#""Authorization":"[redacted]""#));
        assert!(output.contains(r#""model":"safe""#));
    }

    #[test]
    fn keeps_non_secret_json_text_format_when_no_structured_key_matches() {
        let input = r#"{ "model": "safe", "message": "hello" }"#;
        assert_eq!(redact_secret(input), input);
    }

    #[test]
    fn redacts_url_query_values() {
        assert_eq!(
            redact_url_query("https://api.example.com/v1?key=abcdefgh&q=hello"),
            "https://api.example.com/v1?key=[redacted]&q=[redacted]"
        );
        assert_eq!(
            redact_url_query("https://x.example.com/path?token=12345678#frag"),
            "https://x.example.com/path?token=[redacted]#frag"
        );
        // 无值的参数保留
        assert_eq!(
            redact_url_query("https://x.example.com/p?flag&id=1"),
            "https://x.example.com/p?flag&id=[redacted]"
        );
        // 无 query 的 URL 原样返回
        assert_eq!(
            redact_url_query("https://x.example.com/path"),
            "https://x.example.com/path"
        );
        // 空 query 安全
        assert_eq!(
            redact_url_query("https://x.example.com/p?"),
            "https://x.example.com/p?"
        );
    }

    #[test]
    fn redacts_home_path_prefix() {
        let _guard = crate::test_support::env_lock();
        let home = dirs::home_dir().expect("home dir in test env");
        let home_str = home.to_string_lossy().to_string();
        let input = format!("{home_str}/projects/foo/main.rs");
        assert_eq!(redact_home_path(&input), "~/projects/foo/main.rs");
        assert_eq!(redact_home_path(&home_str), "~");
        // 非 home 前缀路径不受影响
        let other = "/tmp/not-home/file.txt".to_string();
        assert_eq!(redact_home_path(&other), other);
    }

    #[test]
    fn redacts_home_path_with_backslash_boundary() {
        let _guard = crate::test_support::env_lock();
        let home = dirs::home_dir().expect("home dir in test env");
        let home_str = home.to_string_lossy().to_string();
        // Windows 风格路径分隔符 `\` 也是合法边界（C:\Users\name\... 形态）。
        let input = format!("{home_str}\\foo\\bar");
        assert_eq!(redact_home_path(&input), format!("~\\foo\\bar"));
    }

    #[test]
    fn redacts_home_path_rejected_match_keeps_text_without_panicking() {
        let _guard = crate::test_support::env_lock();
        let home = dirs::home_dir().expect("home dir in test env");
        let home_str = home.to_string_lossy().to_string();
        // 边界校验拒绝（home 后紧跟词内字符 X）：推进不得丢字符、不得 panic，
        // 原文原样返回。
        let rejected = format!("prefix {home_str}X/suffix");
        assert_eq!(redact_home_path(&rejected), rejected);
    }

    #[test]
    fn redacts_home_path_with_non_ascii_home_without_panicking() {
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", "/Users/张三");
        // home 含多字节字符且匹配被拒（home 后紧跟非边界字节 X）：推进
        // search_from 时对齐字符边界，不得 panic，原文原样返回。
        let rejected = "/Users/张三X/notes";
        assert_eq!(redact_home_path(rejected), rejected);
        // 正常匹配：整个 home 前缀替换为 ~。
        assert_eq!(redact_home_path("/Users/张三/notes"), "~/notes");
        // 文本中间出现 home 且被拒：前后文本都保留。
        let mid = "cwd=/Users/张三X/app";
        assert_eq!(redact_home_path(mid), mid);
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn redact_text_combines_all_rules() {
        let home = dirs::home_dir().expect("home dir in test env");
        let input = format!(
            "Authorization: Bearer sk-ant-abcdefgh12345678\ncwd={home_str}/app?token=12345678&x=1",
            home_str = home.to_string_lossy()
        );
        let out = redact_text(&input);
        assert!(!out.contains("sk-ant"));
        assert!(!out.contains("12345678"));
        assert!(out.contains("Authorization: [redacted]"));
        assert!(out.contains("token=[redacted]"));
        assert!(out.contains("~/app"));
        // 不泄露原始 home 用户名
        assert!(!out.contains(home.to_string_lossy().as_ref()));
    }
}
