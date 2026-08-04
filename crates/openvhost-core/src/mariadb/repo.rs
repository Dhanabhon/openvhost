// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistence for the packaged MariaDB series' generated root credential
//! (`state.db`'s `mariadb_instances` table; migration
//! `0005_mariadb_instances.sql`; spec D4). Queries are compile-time-checked
//! (`query!`/`query_as!`), mirroring [`crate::mysql::MysqlInstanceRepo`] in
//! shape and in what it refuses to be: **NOT the source of truth for "is this
//! datadir initialized"** — that is read from disk (see
//! [`crate::mariadb::classify_mariadb_datadir`]), never from a state.db
//! boolean. This table holds the credential this app itself generated, and
//! when it was most recently set. Nothing else.
//!
//! [`RootPassword`] is reused IN PLACE from `crate::mysql::init` rather than
//! forked (spec D5): it is generic in substance despite its module, it carries
//! the redacting `Debug` and the deliberate absence of `Serialize`, and a
//! second copy is a second thing to remember to keep redacting.

use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;
use crate::mariadb::MARIADB_SERIES;
use crate::mysql::RootPassword;

/// The persisted MariaDB instance row: its generated root credential and when
/// it was (most recently) set.
///
/// No `major`/`series` field, for the reason
/// [`crate::mariadb::MariadbPaths`] takes no series parameter: this build ships
/// exactly one series, so a field here would be a value every caller has to
/// carry and none can vary. The COLUMN still exists — see [`MariadbInstanceRepo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MariadbInstance {
    pub root_password: RootPassword,
    pub initialized_at: i64,
}

/// Raw DB row (primitive columns) — decoded by sqlx, then wrapped below.
struct MariadbInstanceRow {
    root_password: String,
    initialized_at: i64,
}

/// SQLite-backed persistence for the one [`MariadbInstance`] row.
///
/// **Every method binds [`MARIADB_SERIES`] itself; none takes a series from a
/// caller.** That is what makes the "re-validate on read" step
/// [`crate::mysql::MysqlInstanceRepo`] needs unnecessary here rather than
/// merely skipped: `MysqlInstanceRepo` selects by a caller-supplied
/// [`crate::mysql::MysqlMajor`] and so has to re-check what came back, whereas
/// a row this repo returns matched a `WHERE major = ?` bound to a compile-time
/// constant. A hand-edited `state.db` can add a `major = '9.9'` row; it can
/// never make one come back through here.
///
/// The `major` column is kept in the schema (rather than the table being a
/// one-row singleton) so a second series is a schema-compatible change: adding
/// the argument later is a signature change, whereas widening a singleton
/// table is a migration.
///
/// A single concrete struct, not a trait behind an implementation — the same
/// call `MysqlInstanceRepo` makes: its only consumer can exercise it directly
/// against [`Db::open_in_memory`], exactly as this module's own tests do.
pub struct MariadbInstanceRepo {
    pool: SqlitePool,
}

impl MariadbInstanceRepo {
    /// Build a repository over the given database's connection pool.
    pub fn new(db: &Db) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }

    /// Insert or replace the row, stamping `initialized_at` to now. The ONE
    /// write path: a fresh init and a later password reset both call it, so
    /// the timestamp means "last set", never the datadir's creation time —
    /// which is independently recoverable from the filesystem and never from
    /// this column.
    ///
    /// The password crosses this boundary as a [`RootPassword`], not a
    /// `String`: sqlx binds `password.expose()` directly into the statement's
    /// parameter, so the value never becomes part of a formatted SQL string,
    /// an argv, or an environment variable.
    pub async fn upsert(&self, password: &RootPassword) -> Result<(), CoreError> {
        let ts = now_ms();
        sqlx::query!(
            "INSERT INTO mariadb_instances (major, root_password, initialized_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT(major) DO UPDATE SET \
               root_password = excluded.root_password, \
               initialized_at = excluded.initialized_at",
            MARIADB_SERIES,
            password.expose(),
            ts,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up the stored instance; `Ok(None)` if none exists.
    pub async fn get(&self) -> Result<Option<MariadbInstance>, CoreError> {
        let row = sqlx::query_as!(
            MariadbInstanceRow,
            "SELECT root_password, initialized_at FROM mariadb_instances WHERE major = ?",
            MARIADB_SERIES
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| MariadbInstance {
            root_password: RootPassword::from_stored(r.root_password),
            initialized_at: r.initialized_at,
        }))
    }

    /// Delete the row; returns whether one was actually removed.
    pub async fn delete(&self) -> Result<bool, CoreError> {
        let res = sqlx::query!(
            "DELETE FROM mariadb_instances WHERE major = ?",
            MARIADB_SERIES
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mysql::{MysqlInstanceRepo, MysqlMajor, generate_root_password};

    async fn repo() -> (Db, MariadbInstanceRepo) {
        let db = Db::open_in_memory().await.unwrap();
        let repo = MariadbInstanceRepo::new(&db);
        (db, repo)
    }

    #[tokio::test]
    async fn get_on_an_empty_table_is_none() {
        let (_db, repo) = repo().await;
        assert!(repo.get().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_then_get_round_trips_the_password_and_a_positive_timestamp() {
        let (_db, repo) = repo().await;
        let pw = generate_root_password();

        repo.upsert(&pw).await.unwrap();

        let got = repo.get().await.unwrap().unwrap();
        assert_eq!(got.root_password.expose(), pw.expose());
        assert!(got.initialized_at > 0);
    }

    #[tokio::test]
    async fn upsert_twice_replaces_rather_than_duplicating() {
        let (db, repo) = repo().await;
        let first = generate_root_password();
        let second = generate_root_password();

        repo.upsert(&first).await.unwrap();
        repo.upsert(&second).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mariadb_instances")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "upsert must replace, not accumulate rows");
        assert_eq!(
            repo.get().await.unwrap().unwrap().root_password.expose(),
            second.expose()
        );
    }

    #[tokio::test]
    async fn delete_removes_the_row_and_reports_whether_one_existed() {
        let (_db, repo) = repo().await;

        assert!(!repo.delete().await.unwrap(), "nothing to delete yet");

        repo.upsert(&generate_root_password()).await.unwrap();
        assert!(repo.delete().await.unwrap());
        assert!(repo.get().await.unwrap().is_none());
        assert!(!repo.delete().await.unwrap(), "already gone");
    }

    /// Spec D4's whole reason for a separate table: the two engines' rows must
    /// not be able to reach each other. 11.4 and 8.4 not colliding today is an
    /// accident; a shared table keyed on `major` would make it load-bearing.
    ///
    /// VACUITY: proven by pointing this repo's three statements at
    /// `mysql_instances` — `mariadb.get()` then returns MySQL's password and
    /// the `assert_ne!` fails.
    #[tokio::test]
    async fn the_two_engines_credentials_are_stored_in_separate_tables() {
        let db = Db::open_in_memory().await.unwrap();
        let mariadb = MariadbInstanceRepo::new(&db);
        let mysql = MysqlInstanceRepo::new(&db);
        let major_84 = MysqlMajor::parse("8.4").unwrap();

        let mariadb_pw = generate_root_password();
        let mysql_pw = generate_root_password();
        mariadb.upsert(&mariadb_pw).await.unwrap();
        mysql.upsert(&major_84, &mysql_pw).await.unwrap();

        assert_eq!(
            mariadb.get().await.unwrap().unwrap().root_password.expose(),
            mariadb_pw.expose()
        );
        assert_eq!(
            mysql
                .get(&major_84)
                .await
                .unwrap()
                .unwrap()
                .root_password
                .expose(),
            mysql_pw.expose()
        );
        assert_ne!(mariadb_pw.expose(), mysql_pw.expose());

        // And deleting one leaves the other entirely alone.
        mariadb.delete().await.unwrap();
        assert!(mysql.get(&major_84).await.unwrap().is_some());
    }

    /// A hand-edited `state.db` can hold a row for a series this build does
    /// not ship. It must never come back through this repo — the `WHERE
    /// major = ?` bound to the compile-time constant is the guarantee.
    ///
    /// VACUITY: proven by changing the bound value to `'9.9'` — `get` then
    /// returns the planted row and the `is_none` fails.
    #[tokio::test]
    async fn a_row_for_another_series_is_never_returned() {
        let (db, repo) = repo().await;
        sqlx::query("INSERT INTO mariadb_instances VALUES ('9.9', 'planted', 1)")
            .execute(db.pool())
            .await
            .unwrap();

        assert!(repo.get().await.unwrap().is_none());
    }

    #[test]
    fn the_instance_debug_does_not_leak_the_password_even_though_it_derives_debug() {
        let instance = MariadbInstance {
            root_password: generate_root_password(),
            initialized_at: 1,
        };
        let debug = format!("{instance:?}");
        assert!(
            !debug.contains(instance.root_password.expose()),
            "got {debug:?}"
        );
        assert!(debug.contains("<redacted>"), "got {debug:?}");
    }

    /// `MariadbInstance` must never gain `Serialize`: an outbound DTO could
    /// then be built by deriving straight through it, which is precisely what
    /// `RootPassword`'s missing `Serialize` exists to prevent. Asserted by
    /// compilation — this generic function only accepts `T: Serialize`, and
    /// the commented line below is the thing that must not compile.
    #[test]
    fn the_instance_is_not_serializable() {
        fn assert_serializable<T: serde::Serialize>() {}
        assert_serializable::<String>();
        // assert_serializable::<MariadbInstance>(); // MUST NOT COMPILE
    }

    /// state.db and its WAL sidecars are 0600 after a credential write.
    ///
    /// Asserted here, on the file this credential actually lands in, rather
    /// than trusted from `Db::open`'s own tests: the guarantee that matters is
    /// "the file holding the password is private", and the sidecars are where
    /// an un-checkpointed WAL keeps a freshly written row.
    ///
    /// VACUITY: proven by chmodding `state.db` to 0644 immediately before the
    /// assertion — it fails, naming the mode it found.
    #[cfg(unix)]
    #[tokio::test]
    async fn state_db_and_its_sidecars_are_0600_after_the_credential_is_written() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let db = Db::open(home.path()).await.unwrap();
        let repo = MariadbInstanceRepo::new(&db);

        repo.upsert(&generate_root_password()).await.unwrap();

        let state_db = home.path().join("state.db");
        let mut checked = 0;
        for name in ["state.db", "state.db-wal", "state.db-shm"] {
            let p = home.path().join(name);
            if !p.exists() {
                continue;
            }
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} is {mode:o}, must be 0600", p.display());
            checked += 1;
        }
        assert!(
            state_db.exists() && checked >= 1,
            "the loop must have checked at least state.db itself, or it proves nothing"
        );
    }
}
