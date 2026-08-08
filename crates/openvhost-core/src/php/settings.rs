// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistence for the PHP-wide settings — today exactly one: which major the
//! catch-all serves by default (`state.db`'s `php_settings` table, migration
//! `0006_php_settings.sql`; default-PHP design D1).
//!
//! A singleton row mirroring [`crate::settings_repo`], and deliberately its own
//! table rather than a column on `web_server_settings`: that struct's own doc
//! calls it "editable nginx settings", every field of it is an nginx directive,
//! and the apply pipeline records that those values reach exactly one generated
//! file. "Which PHP is default" is none of those things.
//!
//! Queries are compile-time-checked (`query!`/`query_as!`) and the row
//! re-validates through [`PhpVersion`] on read, the same shape
//! `crate::site::repo` and `crate::settings_repo` use: a hand-edited `state.db`
//! must never feed an unvalidated value toward a generated config or a socket
//! filename.

use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;
use crate::site::model::PhpVersion;

/// The stored PHP-wide settings.
///
/// [`Default`] is "nobody has chosen a default", which is what a machine with
/// no row looks like and what every machine looks like today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhpSettings {
    /// The major the catch-all should serve, when someone has chosen one.
    ///
    /// `None` is a real value here, not a missing one: it means the historical
    /// first-installed rule still applies. Resolving this against what is
    /// actually installed is a separate step — see [`crate::php::DefaultPhp`],
    /// which is where "the stored major is no longer installed" becomes a state
    /// rather than an absence.
    pub default_major: Option<PhpVersion>,
}

/// The persistence seam for [`PhpSettings`]. Consumers depend on this, not the
/// concrete type.
///
/// Methods return `impl Future + Send` (RPITIT) rather than using
/// `async_trait` — see `crate::site::repo::SiteRepository` for the same choice
/// and its rationale.
pub trait PhpSettingsRepository: Send + Sync {
    /// The stored settings, or [`PhpSettings::default`] when no row exists yet.
    ///
    /// Does NOT write a row: a fresh install has no PHP settings row, and that
    /// is not an error to fix on read — the same call
    /// [`crate::settings_repo::WebServerSettingsRepository::get`] makes, for
    /// the same reason (seeding on read would mean every launch writes to
    /// state.db before the user has touched anything).
    fn get(&self) -> impl std::future::Future<Output = Result<PhpSettings, CoreError>> + Send;
    /// Insert or replace the singleton row (`id = 1`).
    ///
    /// Saving `default_major: None` **clears** the preference rather than
    /// leaving the previous one in place: `None` is a value the caller can
    /// mean, so writing it has to be expressible.
    fn save(
        &self,
        s: &PhpSettings,
    ) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
}

/// Re-validate a stored major, reporting the COLUMN rather than the newtype's
/// own generic field name.
///
/// [`PhpVersion::parse`] reports `field: "php_version"`, which names no column
/// in this table; a corrupt row would then tell the reader nothing about where
/// to look. Same relabel `crate::settings_repo::seconds_err` performs for its
/// four `Seconds` columns, and the same reason.
fn parse_default_major(s: &str) -> Result<PhpVersion, CoreError> {
    PhpVersion::parse(s).map_err(|e| match e {
        CoreError::Validation { reason, .. } => CoreError::Validation {
            field: "default_major",
            reason,
        },
        other => other,
    })
}

/// SQLite-backed [`PhpSettingsRepository`].
pub struct SqlitePhpSettings<'a>(&'a SqlitePool);

impl<'a> SqlitePhpSettings<'a> {
    /// Build a repository over the given database's connection pool.
    pub fn new(db: &'a Db) -> Self {
        Self(db.pool())
    }
}

impl PhpSettingsRepository for SqlitePhpSettings<'_> {
    async fn get(&self) -> Result<PhpSettings, CoreError> {
        let row: Option<Option<String>> =
            sqlx::query_scalar!("SELECT default_major FROM php_settings WHERE id = 1")
                .fetch_optional(self.0)
                .await?;
        // Two nestings of `Option`, and they mean different things: the outer
        // is "is there a row at all", the inner is "does that row name a
        // major". Both flatten to "no preference" here, and that is the ONLY
        // place they are allowed to: past this point the absence is a single
        // fact, and "the preference names something not installed" is a
        // different fact entirely (see `DefaultPhp`).
        match row.flatten() {
            Some(major) => Ok(PhpSettings {
                default_major: Some(parse_default_major(&major)?),
            }),
            None => Ok(PhpSettings::default()),
        }
    }

    async fn save(&self, s: &PhpSettings) -> Result<(), CoreError> {
        let ts = now_ms();
        let default_major = s.default_major.as_ref().map(|v| v.as_str());
        sqlx::query!(
            "INSERT INTO php_settings (id, default_major, updated_at) \
             VALUES (1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
               default_major = excluded.default_major, \
               updated_at = excluded.updated_at",
            default_major,
            ts,
        )
        .execute(self.0)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn version(v: &str) -> PhpVersion {
        PhpVersion::parse(v).unwrap()
    }

    #[tokio::test]
    async fn a_fresh_database_reads_no_preference_without_writing_a_row() {
        // The claim the whole slice rests on, at the storage layer: every
        // machine today has no row, and reading must not create one.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqlitePhpSettings::new(&db);
        assert_eq!(repo.get().await.unwrap(), PhpSettings::default());
        assert_eq!(repo.get().await.unwrap().default_major, None);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM php_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "reading must not insert");
    }

    #[tokio::test]
    async fn a_saved_preference_survives_a_round_trip() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqlitePhpSettings::new(&db);
        repo.save(&PhpSettings {
            default_major: Some(version("8.3")),
        })
        .await
        .unwrap();

        assert_eq!(
            repo.get().await.unwrap().default_major.unwrap().as_str(),
            "8.3"
        );
    }

    #[tokio::test]
    async fn saving_twice_replaces_rather_than_accumulating() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqlitePhpSettings::new(&db);
        repo.save(&PhpSettings {
            default_major: Some(version("8.1")),
        })
        .await
        .unwrap();
        repo.save(&PhpSettings {
            default_major: Some(version("8.4")),
        })
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM php_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            repo.get().await.unwrap().default_major.unwrap().as_str(),
            "8.4"
        );
    }

    #[tokio::test]
    async fn a_preference_can_be_cleared_back_to_none() {
        // Clearing has to be expressible, or "I want the old behaviour back"
        // becomes unreachable once a default has ever been set.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqlitePhpSettings::new(&db);
        repo.save(&PhpSettings {
            default_major: Some(version("8.3")),
        })
        .await
        .unwrap();
        repo.save(&PhpSettings::default()).await.unwrap();

        assert_eq!(repo.get().await.unwrap().default_major, None);
        // A row still exists — it is the VALUE that is NULL, not the row that
        // is gone. Both read as "no preference"; this pins that the row-present
        // path really does reach the same answer, so the flatten above is
        // exercised rather than assumed.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM php_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a_corrupt_default_major_is_rejected_on_read_and_names_its_column() {
        // state.db is a file on the user's disk, and this value selects a
        // php-fpm socket filename. `../../etc` is shaped like a traversal and
        // is exactly what re-validation on read exists to refuse — whatever
        // wrote the row.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqlitePhpSettings::new(&db);
        repo.save(&PhpSettings {
            default_major: Some(version("8.3")),
        })
        .await
        .unwrap();
        sqlx::query("UPDATE php_settings SET default_major = '../../etc' WHERE id = 1")
            .execute(db.pool())
            .await
            .unwrap();

        match repo.get().await {
            Err(CoreError::Validation { field, .. }) => {
                assert_eq!(
                    field, "default_major",
                    "the error must name the column, not PhpVersion's generic field"
                );
            }
            other => panic!("expected CoreError::Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_table_is_a_singleton_at_the_schema_level() {
        // The `CHECK (id = 1)` is what makes "which row is the real one" a
        // question no query has to answer. Asserted against the database
        // rather than trusted from the migration text.
        let db = Db::open_in_memory().await.unwrap();
        let err = sqlx::query(
            "INSERT INTO php_settings (id, default_major, updated_at) VALUES (2, '8.3', 0)",
        )
        .execute(db.pool())
        .await
        .unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("check"),
            "expected the CHECK (id = 1) constraint to reject it, got {err}"
        );
    }
}
