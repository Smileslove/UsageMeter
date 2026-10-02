use crate::session::opencode_reader::{OpenCodeSchemaMode, REQUIRED_V2_MESSAGE_COLUMNS};
use rusqlite::Connection;

pub(in crate::session) fn detect_schema_mode(
    conn: &Connection,
    required_session_columns: &[&str],
    required_message_columns: &[&str],
) -> (OpenCodeSchemaMode, Option<String>) {
    let legacy_missing = missing_required_columns(conn, "message", required_message_columns);
    let v2_missing = missing_required_columns(conn, "session_message", REQUIRED_V2_MESSAGE_COLUMNS);
    let has_legacy = legacy_missing.is_empty();
    let has_v2 = v2_missing.is_empty();

    if !has_legacy && !has_v2 {
        let col = legacy_missing
            .first()
            .cloned()
            .unwrap_or_else(|| v2_missing.first().cloned().unwrap_or_default());
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(format!(
                "OpenCode 消息表缺少字段 `{}`，可能是较旧或较新版本的 OpenCode",
                col
            )),
        );
    }

    if !verify_json_structure(conn, has_legacy, has_v2) {
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(
                "OpenCode 消息 data JSON 结构与预期不匹配，可能是版本升级后更改了内部格式"
                    .to_string(),
            ),
        );
    }

    let metadata_table = if table_exists(conn, "session_v2")
        && missing_required_columns(conn, "session_v2", required_session_columns).is_empty()
    {
        "session_v2"
    } else {
        "session"
    };
    if let Some(col) = missing_required_columns(conn, metadata_table, required_session_columns)
        .into_iter()
        .next()
    {
        return (
            OpenCodeSchemaMode::MessageOnly,
            Some(format!(
                "{} 表缺少字段 `{}`，将退化为仅消息模式",
                metadata_table, col
            )),
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

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .unwrap_or(false)
}

fn missing_required_columns(conn: &Connection, table: &str, required: &[&str]) -> Vec<String> {
    let columns = get_table_columns(conn, table);
    required
        .iter()
        .filter(|col| !columns.iter().any(|existing| existing == **col))
        .map(|col| (*col).to_string())
        .collect()
}

fn verify_json_structure(conn: &Connection, has_legacy: bool, has_v2: bool) -> bool {
    // 空库（尚无 assistant 消息）无法判断 JSON 结构，视为兼容；
    // 只有在存在 assistant 消息但均缺少 tokens 字段时才判定为结构不匹配。
    let mut found_messages = false;
    if has_legacy {
        let result: rusqlite::Result<i64> = conn.query_row(
            "SELECT COUNT(*) FROM message
             WHERE json_extract(data, '$.role') = 'assistant'
               AND (json_extract(data, '$.tokens.input') IS NOT NULL
                 OR json_extract(data, '$.tokens.output') IS NOT NULL
                 OR json_extract(data, '$.tokens.reasoning') IS NOT NULL
                 OR json_extract(data, '$.cost') > 0)",
            [],
            |row| row.get(0),
        );
        found_messages |= result.unwrap_or(0) > 0;
    }
    if has_v2 {
        let result: rusqlite::Result<i64> = conn.query_row(
            "SELECT COUNT(*) FROM session_message
             WHERE type IN ('assistant', 'compaction')
               AND (json_extract(data, '$.tokens.input') IS NOT NULL
                 OR json_extract(data, '$.tokens.output') IS NOT NULL
                 OR json_extract(data, '$.tokens.reasoning') IS NOT NULL
                 OR json_extract(data, '$.cost') > 0)",
            [],
            |row| row.get(0),
        );
        found_messages |= result.unwrap_or(0) > 0;
    }
    let total_messages: i64 = if has_legacy && has_v2 {
        conn.query_row(
            "SELECT (SELECT COUNT(*) FROM message WHERE json_extract(data, '$.role') = 'assistant')
                    + (SELECT COUNT(*) FROM session_message WHERE type IN ('assistant', 'compaction'))",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
    } else if has_legacy {
        conn.query_row(
            "SELECT COUNT(*) FROM message WHERE json_extract(data, '$.role') = 'assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM session_message WHERE type IN ('assistant', 'compaction')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
    };
    total_messages == 0 || found_messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn create_message_table(conn: &Connection) {
        conn.execute(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                data TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
    }

    #[test]
    fn verify_json_structure_requires_matching_assistant_tokens() {
        let conn = Connection::open_in_memory().unwrap();
        create_message_table(&conn);

        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            (
                "m1",
                "s1",
                r#"{"role":"assistant","content":"hello without token payload"}"#,
            ),
        )
        .unwrap();

        assert!(!verify_json_structure(&conn, true, false));
    }

    #[test]
    fn verify_json_structure_accepts_matching_assistant_tokens() {
        let conn = Connection::open_in_memory().unwrap();
        create_message_table(&conn);

        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            (
                "m1",
                "s1",
                r#"{"role":"assistant","tokens":{"input":12,"output":34}}"#,
            ),
        )
        .unwrap();

        assert!(verify_json_structure(&conn, true, false));
    }

    #[test]
    fn detect_schema_mode_accepts_v2_message_table_without_full_session_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL);
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                type TEXT NOT NULL,
                seq INTEGER NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO session_message
                (id, session_id, type, seq, time_created, time_updated, data)
            VALUES ('m1', 's1', 'assistant', 1, 1000, 1000,
                    '{"model":{"id":"gpt-5"},"tokens":{"input":1,"output":2}}');
            "#,
        )
        .unwrap();

        let (mode, reason) = detect_schema_mode(
            &conn,
            crate::session::opencode_reader::REQUIRED_SESSION_COLUMNS,
            crate::session::opencode_reader::REQUIRED_MESSAGE_COLUMNS,
        );
        assert_eq!(mode, OpenCodeSchemaMode::MessageOnly);
        assert!(reason.unwrap_or_default().contains("session 表缺少字段"));
    }
}
