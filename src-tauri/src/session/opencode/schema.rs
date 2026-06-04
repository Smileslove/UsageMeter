use crate::session::opencode_reader::OpenCodeSchemaMode;
use rusqlite::Connection;

pub(in crate::session) fn detect_schema_mode(
    conn: &Connection,
    required_session_columns: &[&str],
    required_message_columns: &[&str],
) -> (OpenCodeSchemaMode, Option<String>) {
    if let Some(col) = missing_required_columns(conn, "message", required_message_columns)
        .into_iter()
        .next()
    {
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(format!(
                "message 表缺少字段 `{}`，可能是较旧或较新版本的 OpenCode",
                col
            )),
        );
    }

    if !verify_json_structure(conn) {
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(
                "message.data JSON 结构与预期不匹配（tokens.input / tokens.output 字段不存在），可能是 OpenCode 版本升级后更改了内部格式"
                    .to_string(),
            ),
        );
    }

    if let Some(col) = missing_required_columns(conn, "session", required_session_columns)
        .into_iter()
        .next()
    {
        return (
            OpenCodeSchemaMode::MessageOnly,
            Some(format!("session 表缺少字段 `{}`，将退化为仅消息模式", col)),
        );
    }

    (OpenCodeSchemaMode::Full, None)
}

fn get_table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn missing_required_columns(conn: &Connection, table: &str, required: &[&str]) -> Vec<String> {
    let columns = get_table_columns(conn, table);
    required
        .iter()
        .filter(|col| !columns.iter().any(|existing| existing == **col))
        .map(|col| (*col).to_string())
        .collect()
}

fn verify_json_structure(conn: &Connection) -> bool {
    let result: rusqlite::Result<Option<i64>> = conn.query_row(
        "SELECT COUNT(*) FROM message
         WHERE json_extract(data, '$.role') = 'assistant'
           AND (json_extract(data, '$.tokens.input') IS NOT NULL
             OR json_extract(data, '$.tokens.output') IS NOT NULL
             OR json_extract(data, '$.tokens.reasoning') IS NOT NULL)
         LIMIT 1",
        [],
        |row| row.get(0),
    );
    result.is_ok()
}
