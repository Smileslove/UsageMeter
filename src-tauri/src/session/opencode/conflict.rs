use crate::session::opencode_reader::OpenCodeMessageIdConflictStatus;
use rusqlite::Connection;

pub(in crate::session) fn detect_message_id_conflicts(
    conn: &Connection,
    limit: usize,
) -> OpenCodeMessageIdConflictStatus {
    let sql = format!(
        "SELECT id, COUNT(DISTINCT session_id) AS session_count
         FROM message
         WHERE id IS NOT NULL AND TRIM(id) != ''
         GROUP BY id
         HAVING COUNT(DISTINCT session_id) > 1
         LIMIT {}",
        limit.max(1)
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => {
            return OpenCodeMessageIdConflictStatus {
                has_conflict: false,
                conflict_count: 0,
                sample_ids: Vec::new(),
            }
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => {
            return OpenCodeMessageIdConflictStatus {
                has_conflict: false,
                conflict_count: 0,
                sample_ids: Vec::new(),
            }
        }
    };
    let mut sample_ids = Vec::new();
    let mut count = 0_u64;
    for row in rows.flatten() {
        count += 1;
        sample_ids.push(row.0);
    }
    OpenCodeMessageIdConflictStatus {
        has_conflict: count > 0,
        conflict_count: count,
        sample_ids,
    }
}
