// SPDX-License-Identifier: GPL-3.0-or-later
//! SQLite state store (`state.db`) — one file under the OpenVHost home.

use std::path::{Path, PathBuf};
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
    ///
    /// SECURITY (audit H1, the merge blocker): `state.db` holds the MySQL
    /// root credential in plain text (spec D3's at-rest argument, which
    /// assumes a private home directory AND a private state.db — see
    /// `crate::platform::macos::demo_stack::provision_home` for the home
    /// half). Two layers guarantee the file is 0600:
    /// 1. [`precreate_private`] opens-or-creates the file at 0600 BEFORE
    ///    sqlite ever gets a chance to create it itself, closing the window
    ///    where a fresh file would otherwise briefly exist at the ambient
    ///    umask (commonly 0644).
    /// 2. [`harden_state_db_permissions`], run AFTER the connection is
    ///    established (migrations applied, WAL sidecars created), chmods
    ///    `state.db` and its `-wal`/`-shm` sidecars (when present)
    ///    UNCONDITIONALLY — the only way to repair an install that predates
    ///    this fix (an existing looser file is opened as-is by step 1,
    ///    never repaired by it, since a mode passed to `OpenOptions` only
    ///    applies when that call is the one that creates the file) and the
    ///    only way to reach the sidecars at all, since their creation is
    ///    entirely internal to sqlite.
    pub async fn open(home: &Path) -> Result<Db, CoreError> {
        let path = home.join("state.db");
        precreate_private(&path)?;
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let db = Self::from_options(opts).await?;
        harden_state_db_permissions(&path)?;
        Ok(db)
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

/// Open-or-create `path` at mode 0600, never truncating existing content —
/// see [`Db::open`]'s doc comment for the two-layer rationale. A no-op
/// (`Ok`) when `path` already exists: sqlite opens an existing file as-is
/// without touching its mode, so this only needs to win the race against a
/// FRESH file; an existing file predating this fix is repaired by
/// [`harden_state_db_permissions`] instead, since `OpenOptions::mode` is
/// only honored when the very call that opens it is also the one that
/// creates it.
#[cfg(unix)]
fn precreate_private(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false) // never wipe an existing state.db's content
        .mode(0o600)
        .open(path)
        .map(drop)
        .map_err(|source| CoreError::Io {
            op: "create",
            path: path.to_path_buf(),
            source,
        })
}
#[cfg(not(unix))]
fn precreate_private(_path: &Path) -> Result<(), CoreError> {
    Ok(())
}

/// The path SQLite's WAL mode names a sidecar at: the FULL db filename with
/// `-wal`/`-shm` appended directly (`state.db-wal`, not `state.db.wal`).
fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Unconditionally chmod `state.db` (and its `-wal`/`-shm` sidecars, if
/// present) to 0600 — see [`Db::open`]'s doc comment. Whether a given
/// sidecar exists yet by the time this runs is sqlite's own implementation
/// detail across versions; this crate's own test observes what actually
/// happens rather than asserting a sidecar must exist, and a missing one
/// here is likewise not an error — there is nothing to tighten.
#[cfg(unix)]
fn harden_state_db_permissions(path: &Path) -> Result<(), CoreError> {
    chmod_0600(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = sidecar_path(path, suffix);
        if sidecar.exists() {
            chmod_0600(&sidecar)?;
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn harden_state_db_permissions(_path: &Path) -> Result<(), CoreError> {
    Ok(())
}

#[cfg(unix)]
fn chmod_0600(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        CoreError::Io {
            op: "set_permissions",
            path: path.to_path_buf(),
            source,
        }
    })
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
    async fn in_memory_db_runs_the_mysql_instances_migration() {
        let db = Db::open_in_memory().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM mysql_instances")
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

    /// Security audit H1 (THE merge blocker): `state.db` holds the MySQL
    /// root credential in plain text (spec D3's at-rest argument), so the
    /// file itself must never be readable by another local account. Any
    /// `-wal`/`-shm` sidecar SQLite's WAL mode has created by the time
    /// `open` returns must be 0600 too — but WHETHER either sidecar exists
    /// yet is sqlite's own implementation detail, observed here rather than
    /// assumed, per the audit's own caution.
    #[cfg(unix)]
    #[tokio::test]
    async fn state_db_and_any_wal_sidecars_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        let _db = Db::open(home).await.unwrap();

        let path = home.join("state.db");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state.db must be 0600");

        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.clone().into_os_string();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            if sidecar.exists() {
                let sidecar_mode =
                    std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    sidecar_mode, 0o600,
                    "{suffix} sidecar must be 0600 when present"
                );
            }
        }
    }

    /// An install that predates this fix left `state.db` at a looser mode
    /// (e.g. the umask-derived 0644 sqlite would otherwise create it at) —
    /// `open` must tighten it on every call, not only when it creates the
    /// file fresh.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_preexisting_looser_state_db_is_tightened_to_0600_on_open() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let path = home.join("state.db");
        // A zero-byte file is a valid, empty SQLite database — indistinguishable
        // to sqlite from a brand-new file it would have created itself, so this
        // faithfully simulates "an existing install's state.db", not a corrupt one.
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let _db = Db::open(home).await.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a pre-existing looser state.db must be tightened, not just a freshly created one"
        );
    }
}
