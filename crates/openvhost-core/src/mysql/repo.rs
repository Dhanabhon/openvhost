// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistence for one MySQL major's generated root credential
//! (`state.db`'s `mysql_instances` table; migration
//! `0003_mysql_instances.sql`; spec D3). Queries are compile-time-checked
//! (`query!`/`query_as!`); rows re-validate through [`MysqlMajor`] on read
//! (the same "never trust a hand-edited state.db" discipline
//! `crate::site::repo`/`crate::settings_repo` apply). NOT the source of
//! truth for "is this datadir initialized" — that is read from disk, never
//! a state.db boolean (see [`crate::mysql::classify_datadir`]); this table
//! exists purely to hold the credential the app itself generated, plus
//! when it was (most recently) set.

use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;
use crate::mysql::{MysqlMajor, RootPassword};

/// One persisted MySQL instance row: which major, its generated root
/// credential, and when it was (most recently) set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlInstance {
    pub major: MysqlMajor,
    pub root_password: RootPassword,
    pub initialized_at: i64,
}

/// Raw DB row (primitive columns) — decoded by sqlx, then re-validated via
/// `TryFrom` below.
struct MysqlInstanceRow {
    major: String,
    root_password: String,
    initialized_at: i64,
}

impl TryFrom<MysqlInstanceRow> for MysqlInstance {
    type Error = CoreError;

    fn try_from(r: MysqlInstanceRow) -> Result<Self, CoreError> {
        // Shape-only (`from_probe`), not the catalogue-gated `parse`: this
        // is a value THIS process itself wrote, and catalogue membership
        // ("can we install/initialize this major") is an orthogonal
        // policy question — see `MysqlMajor`'s own doc comment — not a
        // storage-layer one. A row that somehow isn't even major.minor
        // shaped is exactly the hand-edited-database corruption
        // re-validation exists to catch.
        let major =
            MysqlMajor::from_probe(r.major.clone()).ok_or_else(|| CoreError::Validation {
                field: "major",
                reason: format!("{:?} is not a major.minor version", r.major),
            })?;
        Ok(MysqlInstance {
            major,
            root_password: RootPassword::from_stored(r.root_password),
            initialized_at: r.initialized_at,
        })
    }
}

/// SQLite-backed persistence for [`MysqlInstance`] rows, keyed by
/// [`MysqlMajor`].
///
/// Deliberately a single concrete struct, not a trait behind a
/// `SqliteXxx` implementation (unlike `crate::site::repo::SiteRepository` /
/// `crate::settings_repo::WebServerSettingsRepository`) — this is the exact
/// shape the task interface specifies, and its only consumer (the command
/// layer, plan Task 5) can exercise it directly against
/// [`Db::open_in_memory`], exactly as this module's own tests do, with no
/// need for a swappable abstraction.
pub struct MysqlInstanceRepo {
    pool: SqlitePool,
}

impl MysqlInstanceRepo {
    /// Build a repository over the given database's connection pool.
    pub fn new(db: &Db) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }

    /// Insert or replace the row for `major`, stamping `initialized_at` to
    /// now. This is the ONE write path: both a fresh init and a later
    /// password reset call it, so the timestamp reflects "last set", not
    /// necessarily the datadir's original creation time — the datadir's
    /// own age, if ever needed, is independently recoverable from the
    /// filesystem, never from this column.
    pub async fn upsert(
        &self,
        major: &MysqlMajor,
        password: &RootPassword,
    ) -> Result<(), CoreError> {
        let ts = now_ms();
        sqlx::query!(
            "INSERT INTO mysql_instances (major, root_password, initialized_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT(major) DO UPDATE SET \
               root_password = excluded.root_password, \
               initialized_at = excluded.initialized_at",
            major.as_str(),
            password.expose(),
            ts,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up the stored instance for `major`; `Ok(None)` if none exists.
    pub async fn get(&self, major: &MysqlMajor) -> Result<Option<MysqlInstance>, CoreError> {
        let row = sqlx::query_as!(
            MysqlInstanceRow,
            "SELECT major, root_password, initialized_at FROM mysql_instances WHERE major = ?",
            major.as_str()
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(MysqlInstance::try_from).transpose()
    }

    /// Delete the row for `major`; returns whether a row was actually
    /// removed.
    pub async fn delete(&self, major: &MysqlMajor) -> Result<bool, CoreError> {
        let res = sqlx::query!(
            "DELETE FROM mysql_instances WHERE major = ?",
            major.as_str()
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
    use crate::db::Db;
    use crate::error::CoreError;
    use crate::mysql::{MysqlMajor, generate_root_password};

    async fn repo() -> (Db, MysqlInstanceRepo) {
        let db = Db::open_in_memory().await.unwrap();
        let repo = MysqlInstanceRepo::new(&db);
        (db, repo)
    }

    #[tokio::test]
    async fn get_on_an_empty_table_is_none() {
        let (_db, repo) = repo().await;
        let major = MysqlMajor::parse("8.4").unwrap();
        assert!(repo.get(&major).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_then_get_round_trips_major_password_and_a_positive_timestamp() {
        let (_db, repo) = repo().await;
        let major = MysqlMajor::parse("8.4").unwrap();
        let pw = generate_root_password();

        repo.upsert(&major, &pw).await.unwrap();

        let got = repo.get(&major).await.unwrap().unwrap();
        assert_eq!(got.major, major);
        assert_eq!(got.root_password.expose(), pw.expose());
        assert!(got.initialized_at > 0);
    }

    #[tokio::test]
    async fn upsert_twice_replaces_rather_than_duplicating() {
        let (db, repo) = repo().await;
        let major = MysqlMajor::parse("8.4").unwrap();
        let first = generate_root_password();
        let second = generate_root_password();

        repo.upsert(&major, &first).await.unwrap();
        repo.upsert(&major, &second).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mysql_instances")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "upsert must replace, not accumulate rows");

        let got = repo.get(&major).await.unwrap().unwrap();
        assert_eq!(got.root_password.expose(), second.expose());
    }

    #[tokio::test]
    async fn delete_removes_the_row_and_reports_whether_one_existed() {
        let (_db, repo) = repo().await;
        let major = MysqlMajor::parse("8.4").unwrap();

        assert!(!repo.delete(&major).await.unwrap(), "nothing to delete yet");

        repo.upsert(&major, &generate_root_password())
            .await
            .unwrap();
        assert!(repo.delete(&major).await.unwrap());
        assert!(repo.get(&major).await.unwrap().is_none());
        assert!(!repo.delete(&major).await.unwrap(), "already gone");
    }

    #[tokio::test]
    async fn two_majors_are_stored_independently() {
        let (_db, repo) = repo().await;
        let major_84 = MysqlMajor::parse("8.4").unwrap();
        let pw_84 = generate_root_password();
        repo.upsert(&major_84, &pw_84).await.unwrap();

        // Shape-valid but out of the install catalogue — proves this repo
        // gates on nothing but shape (`major` is just a TEXT primary key
        // here; catalogue policy lives in `crate::mysql::MysqlMajor`, not
        // in persistence).
        let major_97 = MysqlMajor::from_probe("9.7".to_string()).unwrap();
        let pw_97 = generate_root_password();
        repo.upsert(&major_97, &pw_97).await.unwrap();

        let got_84 = repo.get(&major_84).await.unwrap().unwrap();
        let got_97 = repo.get(&major_97).await.unwrap().unwrap();
        assert_eq!(got_84.root_password.expose(), pw_84.expose());
        assert_eq!(got_97.root_password.expose(), pw_97.expose());
        assert_ne!(got_84.root_password.expose(), got_97.root_password.expose());
    }

    #[test]
    fn try_from_row_rejects_a_malformed_major() {
        let row = MysqlInstanceRow {
            major: "not-a-version".to_string(),
            root_password: "deadbeef".to_string(),
            initialized_at: 1,
        };
        let err = MysqlInstance::try_from(row).unwrap_err();
        assert!(
            matches!(err, CoreError::Validation { field: "major", .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn try_from_row_accepts_a_well_shaped_major() {
        let row = MysqlInstanceRow {
            major: "8.4".to_string(),
            root_password: "deadbeef".to_string(),
            initialized_at: 42,
        };
        let instance = MysqlInstance::try_from(row).unwrap();
        assert_eq!(instance.major.as_str(), "8.4");
        assert_eq!(instance.root_password.expose(), "deadbeef");
        assert_eq!(instance.initialized_at, 42);
    }

    #[test]
    fn mysql_instance_debug_does_not_leak_the_password_even_though_the_struct_derives_debug() {
        let instance = MysqlInstance {
            major: MysqlMajor::parse("8.4").unwrap(),
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
}
