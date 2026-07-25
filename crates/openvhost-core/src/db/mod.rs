// SPDX-License-Identifier: GPL-3.0-or-later
//! SQLite state store (`state.db`) — one file under the OpenVHost home.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::error::CoreError;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/db/migrations");

/// Milliseconds since the Unix epoch (no date-lib dependency).
///
/// Used by `site::repo` to stamp `created_at`/`updated_at` on write. Kept
/// here since it is `db`-scoped rather than `site`-scoped.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Handle to the SQLite state store.
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if absent) `<home>/state.db` and run migrations.
    pub async fn open(home: &Path) -> Result<Db, CoreError> {
        let path = home.join("state.db");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        Self::from_options(opts).await
    }

    /// In-memory store for tests — same migrations, real SQL.
    pub async fn open_in_memory() -> Result<Db, CoreError> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        Self::from_options(opts).await
    }

    async fn from_options(opts: SqliteConnectOptions) -> Result<Db, CoreError> {
        // A single connection keeps `:memory:` coherent and is plenty for a
        // desktop app; WAL handles concurrency on the file path.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        MIGRATOR.run(&pool).await.map_err(sqlx::Error::from)?;
        Ok(Db { pool })
    }

    /// The underlying connection pool, for callers that run their own
    /// queries (e.g. `sqlx::query!` call sites in higher-level repositories).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_positive_and_non_decreasing() {
        let a = now_ms();
        let b = now_ms();
        assert!(a > 0);
        assert!(b >= a);
    }

    #[tokio::test]
    async fn in_memory_db_runs_migrations() {
        let db = Db::open_in_memory().await.unwrap();
        // The migration created `sites`; querying it must succeed (empty).
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn open_is_idempotent_on_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let a = Db::open(home).await.unwrap();
        drop(a);
        // Re-open the same file: migrations already applied, no error.
        let b = Db::open(home).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sites")
            .fetch_one(b.pool())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
