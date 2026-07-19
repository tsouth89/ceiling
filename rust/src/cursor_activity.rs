//! Local Cursor activity, read from Cursor's on-disk AI code-tracking database.
//!
//! Cursor records which model produced each accepted AI code block in
//! `~/.cursor/ai-tracking/ai-code-tracking.db` (table `ai_code_hashes`). This is
//! *activity*, not tokens or dollars — Cursor does not log token usage locally —
//! so callers must present it as "code contributions by model", never as spend.

use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 86_400_000;

/// Per-model Cursor Composer activity over a window.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorModelActivity {
    /// Model id as Cursor recorded it (e.g. "grok-4.5", "claude-sonnet-5",
    /// "default" for Auto model selection).
    pub model: String,
    /// Tracked AI code blocks attributed to this model.
    pub contributions: u64,
    /// Distinct Cursor requests that produced them.
    pub requests: u64,
}

/// Default location of Cursor's AI code-tracking database, when present.
fn cursor_tracking_db_path() -> Option<PathBuf> {
    let path = dirs::home_dir()?
        .join(".cursor")
        .join("ai-tracking")
        .join("ai-code-tracking.db");
    path.exists().then_some(path)
}

/// Cursor Composer activity by model over the last `window_days` (relative to
/// `now_ms`), most-active model first.
///
/// Returns empty when the database is absent or unreadable (Cursor not
/// installed, an older schema, or the file briefly locked mid-write) rather
/// than erroring — this is a best-effort local read.
pub fn cursor_model_activity(now_ms: i64, window_days: i64) -> Vec<CursorModelActivity> {
    let Some(db) = cursor_tracking_db_path() else {
        return Vec::new();
    };
    let since_ms = now_ms - window_days.max(0) * DAY_MS;
    read_cursor_model_activity(&db, since_ms).unwrap_or_default()
}

fn read_cursor_model_activity(
    db: &Path,
    since_ms: i64,
) -> rusqlite::Result<Vec<CursorModelActivity>> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    aggregate_model_activity(&conn, since_ms)
}

/// Group `ai_code_hashes` by model within the window. Split out from the
/// file-open path so it can be tested against an in-memory database.
fn aggregate_model_activity(
    conn: &Connection,
    since_ms: i64,
) -> rusqlite::Result<Vec<CursorModelActivity>> {
    let mut stmt = conn.prepare(
        "SELECT model, COUNT(*) AS contributions, COUNT(DISTINCT requestId) AS requests
         FROM ai_code_hashes
         WHERE model IS NOT NULL AND model <> '' AND timestamp >= ?1
         GROUP BY model
         ORDER BY contributions DESC, model ASC",
    )?;
    let rows = stmt.query_map([since_ms], |row| {
        Ok(CursorModelActivity {
            model: row.get::<_, String>(0)?,
            contributions: row.get::<_, i64>(1)?.max(0) as u64,
            requests: row.get::<_, i64>(2)?.max(0) as u64,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed(rows: &[(&str, i64, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_code_hashes (
                hash TEXT, source TEXT, fileExtension TEXT, fileName TEXT,
                requestId TEXT, conversationId TEXT, timestamp INTEGER,
                model TEXT, createdAt INTEGER
            );",
        )
        .unwrap();
        for (model, ts, req) in rows {
            conn.execute(
                "INSERT INTO ai_code_hashes (model, timestamp, requestId) VALUES (?1, ?2, ?3)",
                rusqlite::params![model, ts, req],
            )
            .unwrap();
        }
        conn
    }

    fn query(conn: &Connection, since_ms: i64) -> Vec<CursorModelActivity> {
        aggregate_model_activity(conn, since_ms).unwrap()
    }

    #[test]
    fn aggregates_by_model_ranked_by_contributions() {
        // grok: 3 blocks over 2 requests; claude: 1 block, 1 request.
        let conn = seed(&[
            ("grok-4.5", 1_000, "r1"),
            ("grok-4.5", 1_000, "r1"),
            ("grok-4.5", 1_000, "r2"),
            ("claude-sonnet-5", 1_000, "r3"),
        ]);
        let rows = query(&conn, 0);
        assert_eq!(
            rows,
            vec![
                CursorModelActivity {
                    model: "grok-4.5".to_string(),
                    contributions: 3,
                    requests: 2,
                },
                CursorModelActivity {
                    model: "claude-sonnet-5".to_string(),
                    contributions: 1,
                    requests: 1,
                },
            ]
        );
    }

    #[test]
    fn excludes_rows_before_the_window_and_blank_models() {
        let conn = seed(&[
            ("grok-4.5", 5_000, "r1"), // in window
            ("grok-4.5", 100, "r0"),   // before window -> excluded
            ("", 5_000, "r2"),         // blank model -> excluded
        ]);
        let rows = query(&conn, 1_000);
        assert_eq!(
            rows,
            vec![CursorModelActivity {
                model: "grok-4.5".to_string(),
                contributions: 1,
                requests: 1,
            }]
        );
    }
}
