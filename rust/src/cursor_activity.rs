//! Local Cursor activity, read from Cursor's on-disk AI code-tracking database.
//!
//! Cursor records which model produced each accepted AI code block in
//! `~/.cursor/ai-tracking/ai-code-tracking.db` (table `ai_code_hashes`). This is
//! *activity*, not tokens or dollars — Cursor does not log token usage locally —
//! so callers must present it as "code contributions by model", never as spend.
//! A missing database is unavailable data. An unreadable or locked database is
//! unreadable. Neither is zero activity.

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

/// Whether the local tracking database produced a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorActivityStatus {
    /// Rows exist in the requested window.
    Available,
    /// The database opened and queried, but no attributed rows were in range.
    Empty,
    /// The tracking database is missing on this machine.
    Unavailable,
    /// The database exists but could not be opened or queried.
    Unreadable,
}

/// Local Composer activity, or an honest missing-data signal.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorActivitySnapshot {
    pub status: CursorActivityStatus,
    pub rows: Vec<CursorModelActivity>,
}

impl CursorActivitySnapshot {
    fn available(rows: Vec<CursorModelActivity>) -> Self {
        if rows.is_empty() {
            Self {
                status: CursorActivityStatus::Empty,
                rows,
            }
        } else {
            Self {
                status: CursorActivityStatus::Available,
                rows,
            }
        }
    }

    fn unavailable() -> Self {
        Self {
            status: CursorActivityStatus::Unavailable,
            rows: Vec::new(),
        }
    }

    fn unreadable() -> Self {
        Self {
            status: CursorActivityStatus::Unreadable,
            rows: Vec::new(),
        }
    }
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
/// A missing file returns [`CursorActivityStatus::Unavailable`]. An open or
/// query failure returns [`CursorActivityStatus::Unreadable`]. Callers must
/// not treat either as zero usage.
pub fn cursor_model_activity(now_ms: i64, window_days: i64) -> CursorActivitySnapshot {
    let Some(db) = cursor_tracking_db_path() else {
        return CursorActivitySnapshot::unavailable();
    };
    let since_ms = now_ms - window_days.max(0) * DAY_MS;
    snapshot_from_db(&db, since_ms)
}

fn snapshot_from_db(db: &Path, since_ms: i64) -> CursorActivitySnapshot {
    match read_cursor_model_activity(db, since_ms) {
        Ok(rows) => CursorActivitySnapshot::available(rows),
        Err(_) => CursorActivitySnapshot::unreadable(),
    }
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

    fn load_fixture(sql: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "providers/fixtures/cursor/activity-schema.sql"
        ))
        .unwrap();
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn query(conn: &Connection, since_ms: i64) -> Vec<CursorModelActivity> {
        aggregate_model_activity(conn, since_ms).unwrap()
    }

    #[test]
    fn fixture_normal_aggregates_by_model_and_dedupes_requests() {
        let conn = load_fixture(include_str!(
            "providers/fixtures/cursor/activity-normal.sql"
        ));
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
    fn fixture_partial_drops_rows_outside_the_window_and_blank_models() {
        let conn = load_fixture(include_str!(
            "providers/fixtures/cursor/activity-partial.sql"
        ));
        let rows = query(&conn, 1000);
        assert_eq!(
            rows,
            vec![CursorModelActivity {
                model: "grok-4.5".to_string(),
                contributions: 1,
                requests: 1,
            }]
        );
    }

    #[test]
    fn fixture_duplicate_counts_one_request_id_once() {
        let conn = load_fixture(include_str!(
            "providers/fixtures/cursor/activity-duplicate.sql"
        ));
        let rows = query(&conn, 0);
        assert_eq!(
            rows,
            vec![CursorModelActivity {
                model: "grok-4.5".to_string(),
                contributions: 4,
                requests: 2,
            }]
        );
    }

    #[test]
    fn fixture_malformed_schema_fails_closed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!(
            "providers/fixtures/cursor/activity-malformed.sql"
        ))
        .unwrap();
        assert!(aggregate_model_activity(&conn, 0).is_err());
    }

    #[test]
    fn snapshot_treats_empty_rows_as_empty_not_unavailable() {
        let snapshot = CursorActivitySnapshot::available(Vec::new());
        assert_eq!(snapshot.status, CursorActivityStatus::Empty);
        assert!(snapshot.rows.is_empty());
    }

    #[test]
    fn snapshot_treats_missing_source_as_unavailable() {
        let snapshot = CursorActivitySnapshot::unavailable();
        assert_eq!(snapshot.status, CursorActivityStatus::Unavailable);
        assert!(snapshot.rows.is_empty());
    }

    #[test]
    fn existing_unreadable_db_is_not_missing_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("ai-code-tracking.db");
        std::fs::write(&db, b"not a sqlite database").unwrap();
        let snapshot = snapshot_from_db(&db, 0);
        assert_eq!(snapshot.status, CursorActivityStatus::Unreadable);
        assert!(snapshot.rows.is_empty());
    }
}
