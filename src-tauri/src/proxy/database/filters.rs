use super::ProxyDatabase;
use crate::models::{SourceFilter, ToolFilter, UsageQueryFilter};

impl ProxyDatabase {
    pub(super) fn build_source_filter_sql(source_filter: &SourceFilter) -> (String, Vec<String>) {
        match source_filter {
            SourceFilter::All => (String::new(), vec![]),
            // OAuth-only attribution is inferred from local scanner facts, never proxy rows.
            SourceFilter::OfficialOpenAiOAuth => ("AND 1 = 0".to_string(), vec![]),
            SourceFilter::Source {
                source_id: _,
                api_key_prefixes,
                base_url,
            } => {
                if api_key_prefixes.is_empty() {
                    return ("AND 1 = 0".to_string(), vec![]);
                }
                let placeholders: Vec<String> =
                    api_key_prefixes.iter().map(|_| "?".to_string()).collect();
                let mut params: Vec<String> = api_key_prefixes.clone();
                params.push(base_url.clone().unwrap_or_default());
                (
                    format!(
                        "AND api_key_prefix IN ({}) AND COALESCE(request_base_url, '') = ?",
                        placeholders.join(",")
                    ),
                    params,
                )
            }
            SourceFilter::Unknown { known_pairs } => {
                if known_pairs.is_empty() {
                    (String::new(), vec![])
                } else {
                    let mut clauses = Vec::new();
                    let mut params = Vec::new();
                    for (prefix, base_url) in known_pairs {
                        clauses.push(
                            "(api_key_prefix = ? AND COALESCE(request_base_url, '') = ?)"
                                .to_string(),
                        );
                        params.push(prefix.clone());
                        params.push(base_url.clone().unwrap_or_default());
                    }
                    (
                        format!(
                            "AND (api_key_prefix IS NULL OR NOT ({}))",
                            clauses.join(" OR ")
                        ),
                        params,
                    )
                }
            }
        }
    }

    pub(super) fn build_tool_filter_sql(tool_filter: &ToolFilter) -> (String, Vec<String>) {
        match tool_filter {
            ToolFilter::All => (String::new(), vec![]),
            ToolFilter::Tool(tool) if tool.trim().is_empty() => (String::new(), vec![]),
            ToolFilter::Tool(tool) => ("AND client_tool = ?".to_string(), vec![tool.clone()]),
            ToolFilter::AnyOf(tools) if tools.is_empty() => ("AND 1 = 0".to_string(), vec![]),
            ToolFilter::AnyOf(tools) => {
                let placeholders = tools.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                (
                    format!("AND client_tool IN ({placeholders})"),
                    tools.clone(),
                )
            }
        }
    }

    pub(super) fn build_usage_filter_sql(usage_filter: &UsageQueryFilter) -> (String, Vec<String>) {
        let (source_where, mut params) = Self::build_source_filter_sql(&usage_filter.source);
        let (tool_where, tool_params) = Self::build_tool_filter_sql(&usage_filter.tool);
        params.extend(tool_params);
        let where_clause = match (source_where.is_empty(), tool_where.is_empty()) {
            (true, true) => String::new(),
            (false, true) => source_where,
            (true, false) => tool_where,
            (false, false) => format!("{source_where} {tool_where}"),
        };
        (where_clause, params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// 构造内存 SQLite 并初始化 proxy 表（不含迁移，SQL 生成测试只需基础表）。
    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        ProxyDatabase::create_tables(&conn).expect("create tables");
        conn
    }

    fn insert_record(
        conn: &Connection,
        message_id: &str,
        model: &str,
        api_key_prefix: Option<&str>,
        request_base_url: Option<&str>,
        client_tool: &str,
    ) {
        conn.execute(
            "INSERT INTO usage_records (
                timestamp, message_id, storage_dedupe_key, model,
                api_key_prefix, request_base_url, client_tool
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                1_700_000_000_000i64,
                message_id,
                format!("key-{message_id}"),
                model,
                api_key_prefix,
                request_base_url,
                client_tool,
            ],
        )
        .expect("insert record");
    }

    /// 用生成的 where 子句在内存库中统计命中行数，验证参数绑定与语义。
    fn run_count(conn: &Connection, where_clause: &str, params: &[String]) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM usage_records WHERE 1 = 1 {where_clause}");
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row| row.get::<_, i64>(0))
            .expect("run count query")
    }

    #[test]
    fn source_filter_all_yields_no_condition() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::All);
        assert_eq!(sql, "");
        assert!(params.is_empty());
    }

    #[test]
    fn source_filter_empty_prefixes_yields_never_match() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::Source {
            source_id: "test".to_string(),
            api_key_prefixes: vec![],
            base_url: None,
        });
        assert_eq!(sql, "AND 1 = 0");
        assert!(params.is_empty());
    }

    #[test]
    fn source_filter_binds_prefixes_then_base_url_in_order() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::Source {
            source_id: "test".to_string(),
            api_key_prefixes: vec!["sk-alpha".to_string(), "sk-beta".to_string()],
            base_url: Some("https://api.example.com".to_string()),
        });
        assert_eq!(
            sql,
            "AND api_key_prefix IN (?,?) AND COALESCE(request_base_url, '') = ?"
        );
        assert_eq!(
            params,
            vec!["sk-alpha", "sk-beta", "https://api.example.com"]
        );
    }

    #[test]
    fn source_filter_missing_base_url_binds_empty_string() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::Source {
            source_id: "test".to_string(),
            api_key_prefixes: vec!["sk-alpha".to_string()],
            base_url: None,
        });
        assert_eq!(
            sql,
            "AND api_key_prefix IN (?) AND COALESCE(request_base_url, '') = ?"
        );
        assert_eq!(params, vec!["sk-alpha", ""]);
    }

    #[test]
    fn source_filter_unknown_with_empty_pairs_yields_no_condition() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::Unknown {
            known_pairs: vec![],
        });
        assert_eq!(sql, "");
        assert!(params.is_empty());
    }

    #[test]
    fn source_filter_unknown_negates_known_pairs_with_param_order() {
        let (sql, params) = ProxyDatabase::build_source_filter_sql(&SourceFilter::Unknown {
            known_pairs: vec![
                (
                    "sk-a".to_string(),
                    Some("https://a.example.com".to_string()),
                ),
                ("sk-b".to_string(), None),
            ],
        });
        assert_eq!(
            sql,
            "AND (api_key_prefix IS NULL OR NOT ((api_key_prefix = ? AND COALESCE(request_base_url, '') = ?) OR (api_key_prefix = ? AND COALESCE(request_base_url, '') = ?)))"
        );
        assert_eq!(params, vec!["sk-a", "https://a.example.com", "sk-b", ""]);
    }

    #[test]
    fn tool_filter_all_and_blank_tool_yield_no_condition() {
        let (sql, params) = ProxyDatabase::build_tool_filter_sql(&ToolFilter::All);
        assert_eq!(sql, "");
        assert!(params.is_empty());

        let (sql, params) = ProxyDatabase::build_tool_filter_sql(&ToolFilter::Tool("   ".into()));
        assert_eq!(sql, "");
        assert!(params.is_empty());
    }

    #[test]
    fn tool_filter_single_tool_binds_exact_param() {
        let (sql, params) =
            ProxyDatabase::build_tool_filter_sql(&ToolFilter::Tool("claude_code".into()));
        assert_eq!(sql, "AND client_tool = ?");
        assert_eq!(params, vec!["claude_code"]);
    }

    #[test]
    fn tool_filter_any_of_binds_all_params() {
        let (sql, params) = ProxyDatabase::build_tool_filter_sql(&ToolFilter::AnyOf(vec![
            "codex".to_string(),
            "opencode".to_string(),
            "qoder".to_string(),
        ]));
        assert_eq!(sql, "AND client_tool IN (?, ?, ?)");
        assert_eq!(params, vec!["codex", "opencode", "qoder"]);
    }

    #[test]
    fn tool_filter_empty_any_of_yields_never_match() {
        let (sql, params) = ProxyDatabase::build_tool_filter_sql(&ToolFilter::AnyOf(vec![]));
        assert_eq!(sql, "AND 1 = 0");
        assert!(params.is_empty());
    }

    #[test]
    fn usage_filter_combines_source_and_tool_and_orders_params() {
        let filter = UsageQueryFilter {
            source: SourceFilter::Source {
                source_id: "test".to_string(),
                api_key_prefixes: vec!["sk-a".to_string()],
                base_url: Some("https://api.example.com".to_string()),
            },
            tool: ToolFilter::Tool("codex".to_string()),
        };
        let (sql, params) = ProxyDatabase::build_usage_filter_sql(&filter);
        assert_eq!(
            sql,
            "AND api_key_prefix IN (?) AND COALESCE(request_base_url, '') = ? AND client_tool = ?"
        );
        assert_eq!(params, vec!["sk-a", "https://api.example.com", "codex"]);
    }

    #[test]
    fn usage_filter_standalone_tool_or_source_keeps_single_clause() {
        let filter = UsageQueryFilter {
            source: SourceFilter::All,
            tool: ToolFilter::Tool("codex".to_string()),
        };
        let (sql, params) = ProxyDatabase::build_usage_filter_sql(&filter);
        assert_eq!(sql, "AND client_tool = ?");
        assert_eq!(params, vec!["codex"]);

        let filter = UsageQueryFilter {
            source: SourceFilter::Source {
                source_id: "test".to_string(),
                api_key_prefixes: vec!["sk-a".to_string()],
                base_url: None,
            },
            tool: ToolFilter::All,
        };
        let (sql, params) = ProxyDatabase::build_usage_filter_sql(&filter);
        assert_eq!(
            sql,
            "AND api_key_prefix IN (?) AND COALESCE(request_base_url, '') = ?"
        );
        assert_eq!(params, vec!["sk-a", ""]);
    }

    #[test]
    fn single_quote_injection_param_matches_nothing() {
        let conn = mem_db();
        insert_record(&conn, "m1", "gpt-4o", Some("sk-a"), None, "codex");
        insert_record(&conn, "m2", "gpt-4o", Some("sk-a"), None, "claude_code");

        let payload = "codex' OR '1'='1".to_string();
        let (sql, params) =
            ProxyDatabase::build_tool_filter_sql(&ToolFilter::Tool(payload.clone()));
        assert_eq!(sql, "AND client_tool = ?");
        assert_eq!(params, vec![payload]);
        // 注入串作为参数值整体参与比较，不会改变结果集。
        assert_eq!(run_count(&conn, &sql, &params), 0);
    }

    #[test]
    fn percent_and_underscore_in_params_are_exact_not_like_wildcards() {
        let conn = mem_db();
        insert_record(&conn, "m1", "gpt-4o", Some("sk-a"), None, "50%_off");

        // client_tool 含 % 与 _，必须作为精确值匹配，而不是被当作 LIKE 通配符。
        let (sql, params) =
            ProxyDatabase::build_tool_filter_sql(&ToolFilter::Tool("50%_off".into()));
        assert_eq!(run_count(&conn, &sql, &params), 1);

        let (sql, params) = ProxyDatabase::build_tool_filter_sql(&ToolFilter::Tool("50".into()));
        assert_eq!(run_count(&conn, &sql, &params), 0);
    }

    #[test]
    fn source_and_tool_filters_select_expected_rows_in_database() {
        let conn = mem_db();
        insert_record(
            &conn,
            "m1",
            "gpt-4o",
            Some("sk-a"),
            Some("https://api.a.com"),
            "codex",
        );
        insert_record(
            &conn,
            "m2",
            "gpt-4o",
            Some("sk-b"),
            Some("https://api.b.com"),
            "codex",
        );
        insert_record(
            &conn,
            "m3",
            "claude",
            Some("sk-a"),
            Some("https://api.a.com"),
            "claude_code",
        );

        let filter = UsageQueryFilter {
            source: SourceFilter::Source {
                source_id: "test".to_string(),
                api_key_prefixes: vec!["sk-a".to_string()],
                base_url: Some("https://api.a.com".to_string()),
            },
            tool: ToolFilter::Tool("codex".to_string()),
        };
        let (sql, params) = ProxyDatabase::build_usage_filter_sql(&filter);
        assert_eq!(run_count(&conn, &sql, &params), 1);

        // Unknown 过滤：排除已知来源组合；无 api_key_prefix 的记录（未归因）保留。
        insert_record(&conn, "m4", "gpt-4o", None, None, "claude_code");
        let unknown = UsageQueryFilter {
            source: SourceFilter::Unknown {
                known_pairs: vec![("sk-a".to_string(), Some("https://api.a.com".to_string()))],
            },
            tool: ToolFilter::All,
        };
        let (sql, params) = ProxyDatabase::build_usage_filter_sql(&unknown);
        // 保留：m2（不同前缀）与 m4（无前缀）。
        assert_eq!(run_count(&conn, &sql, &params), 2);
    }
}
