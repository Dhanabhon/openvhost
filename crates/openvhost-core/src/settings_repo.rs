// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistence for the editable nginx settings (`openvhost_conf::WebServerSettings`)
//! as a singleton row in `state.db`. Queries are compile-time-checked
//! (`query!`/`query_as!`); rows re-validate through the newtypes on read, the
//! same shape `crate::site::repo` uses for `Site` — a hand-edited `state.db`
//! must never feed an unvalidated value into a generated nginx config.

use openvhost_conf::{
    BodySize, ConfError, GzipLevel, GzipTypes, OnOff, Seconds, WebServerSettings, WorkerConnections,
};
use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;

/// The persistence seam for [`WebServerSettings`]. Consumers depend on this,
/// not the concrete type.
///
/// Methods return `impl Future + Send` (RPITIT) rather than using
/// `async_trait` — see `crate::site::repo::SiteRepository` for the same
/// choice and its rationale.
pub trait WebServerSettingsRepository: Send + Sync {
    /// The stored settings, or [`WebServerSettings::default`] when no row
    /// exists yet. Does NOT write a row: a fresh install has no settings row,
    /// and that is not an error to fix on read — see `save` for where the
    /// first row actually gets created.
    fn get(&self)
    -> impl std::future::Future<Output = Result<WebServerSettings, CoreError>> + Send;
    /// Insert or replace the singleton row (`id = 1`).
    fn save(
        &self,
        s: &WebServerSettings,
    ) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
}

/// Raw DB row (primitive columns) — decoded by sqlx, then re-validated.
struct WebServerSettingsRow {
    worker_connections: i64,
    client_max_body_size: String,
    keepalive_timeout: i64,
    tcp_nodelay: i64,
    fastcgi_connect_timeout: i64,
    fastcgi_send_timeout: i64,
    fastcgi_read_timeout: i64,
    gzip: i64,
    gzip_comp_level: i64,
    gzip_types: String,
}

/// Maps a validation failure from `openvhost_conf` to `CoreError::Validation`,
/// naming the column the same way `openvhost_conf::ConfError::InvalidField`
/// already names the field, so a corrupt row surfaces which column broke.
fn from_conf_err(e: ConfError) -> CoreError {
    match e {
        ConfError::InvalidField {
            field,
            value,
            reason,
        } => CoreError::Validation {
            field,
            reason: format!("{value:?}: {reason}"),
        },
        other => CoreError::Validation {
            field: "web_server_settings",
            reason: other.to_string(),
        },
    }
}

/// A SQLite `INTEGER` column decodes to `i64`; the newtypes take `u32`. A
/// value that does not fit (negative, or past `u32::MAX`) is exactly the
/// kind of hand-edited corruption re-validation exists to catch, so it maps
/// to the same `CoreError::Validation` a failed `parse` would produce rather
/// than panicking or silently truncating.
fn to_u32(field: &'static str, v: i64) -> Result<u32, CoreError> {
    u32::try_from(v).map_err(|_| CoreError::Validation {
        field,
        reason: format!("{v} does not fit in a u32"),
    })
}

impl TryFrom<WebServerSettingsRow> for WebServerSettings {
    type Error = CoreError;

    fn try_from(r: WebServerSettingsRow) -> Result<WebServerSettings, CoreError> {
        Ok(WebServerSettings {
            worker_connections: WorkerConnections::parse(to_u32(
                "worker_connections",
                r.worker_connections,
            )?)
            .map_err(from_conf_err)?,
            client_max_body_size: BodySize::parse(&r.client_max_body_size)
                .map_err(from_conf_err)?,
            keepalive_timeout: Seconds::parse(to_u32("keepalive_timeout", r.keepalive_timeout)?)
                .map_err(from_conf_err)?,
            tcp_nodelay: OnOff::new(r.tcp_nodelay != 0),
            fastcgi_connect_timeout: Seconds::parse(to_u32(
                "fastcgi_connect_timeout",
                r.fastcgi_connect_timeout,
            )?)
            .map_err(from_conf_err)?,
            fastcgi_send_timeout: Seconds::parse(to_u32(
                "fastcgi_send_timeout",
                r.fastcgi_send_timeout,
            )?)
            .map_err(from_conf_err)?,
            fastcgi_read_timeout: Seconds::parse(to_u32(
                "fastcgi_read_timeout",
                r.fastcgi_read_timeout,
            )?)
            .map_err(from_conf_err)?,
            gzip: OnOff::new(r.gzip != 0),
            gzip_comp_level: GzipLevel::parse(to_u32("gzip_comp_level", r.gzip_comp_level)?)
                .map_err(from_conf_err)?,
            gzip_types: GzipTypes::parse(&r.gzip_types).map_err(from_conf_err)?,
        })
    }
}

/// SQLite-backed [`WebServerSettingsRepository`].
pub struct SqliteWebServerSettings<'a>(&'a SqlitePool);

impl<'a> SqliteWebServerSettings<'a> {
    /// Build a repository over the given database's connection pool.
    pub fn new(db: &'a Db) -> Self {
        Self(db.pool())
    }
}

impl WebServerSettingsRepository for SqliteWebServerSettings<'_> {
    async fn get(&self) -> Result<WebServerSettings, CoreError> {
        let row = sqlx::query_as!(
            WebServerSettingsRow,
            "SELECT worker_connections, client_max_body_size, keepalive_timeout, tcp_nodelay, \
             fastcgi_connect_timeout, fastcgi_send_timeout, fastcgi_read_timeout, gzip, \
             gzip_comp_level, gzip_types \
             FROM web_server_settings WHERE id = 1"
        )
        .fetch_optional(self.0)
        .await?;

        match row {
            Some(r) => WebServerSettings::try_from(r),
            // No row yet: the defaults, and we do not write them (see the
            // trait doc comment for why seeding on read is the wrong call).
            None => Ok(WebServerSettings::default()),
        }
    }

    async fn save(&self, s: &WebServerSettings) -> Result<(), CoreError> {
        let ts = now_ms();
        let worker_connections = i64::from(s.worker_connections.get());
        let client_max_body_size = s.client_max_body_size.as_str();
        let keepalive_timeout = i64::from(s.keepalive_timeout.get());
        let tcp_nodelay = i64::from(s.tcp_nodelay.is_on());
        let fastcgi_connect_timeout = i64::from(s.fastcgi_connect_timeout.get());
        let fastcgi_send_timeout = i64::from(s.fastcgi_send_timeout.get());
        let fastcgi_read_timeout = i64::from(s.fastcgi_read_timeout.get());
        let gzip = i64::from(s.gzip.is_on());
        let gzip_comp_level = i64::from(s.gzip_comp_level.get());
        let gzip_types = s.gzip_types.as_directive();

        sqlx::query!(
            "INSERT INTO web_server_settings \
             (id, worker_connections, client_max_body_size, keepalive_timeout, tcp_nodelay, \
              fastcgi_connect_timeout, fastcgi_send_timeout, fastcgi_read_timeout, gzip, \
              gzip_comp_level, gzip_types, updated_at) \
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
               worker_connections = excluded.worker_connections, \
               client_max_body_size = excluded.client_max_body_size, \
               keepalive_timeout = excluded.keepalive_timeout, \
               tcp_nodelay = excluded.tcp_nodelay, \
               fastcgi_connect_timeout = excluded.fastcgi_connect_timeout, \
               fastcgi_send_timeout = excluded.fastcgi_send_timeout, \
               fastcgi_read_timeout = excluded.fastcgi_read_timeout, \
               gzip = excluded.gzip, \
               gzip_comp_level = excluded.gzip_comp_level, \
               gzip_types = excluded.gzip_types, \
               updated_at = excluded.updated_at",
            worker_connections,
            client_max_body_size,
            keepalive_timeout,
            tcp_nodelay,
            fastcgi_connect_timeout,
            fastcgi_send_timeout,
            fastcgi_read_timeout,
            gzip,
            gzip_comp_level,
            gzip_types,
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

    #[tokio::test]
    async fn a_fresh_database_reads_the_defaults_without_writing_a_row() {
        // Seeding on read would mean every launch writes to state.db before the
        // user has touched anything, and a failure there would surface as a
        // startup error for a value nobody changed.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        assert_eq!(repo.get().await.unwrap(), WebServerSettings::default());

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM web_server_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "reading must not insert");
    }

    #[tokio::test]
    async fn a_saved_value_survives_a_round_trip() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        let s = WebServerSettings {
            fastcgi_read_timeout: Seconds::parse(900).unwrap(),
            gzip: OnOff::new(true),
            ..WebServerSettings::default()
        };
        repo.save(&s).await.unwrap();

        let back = repo.get().await.unwrap();
        assert_eq!(back.fastcgi_read_timeout.get(), 900);
        assert!(back.gzip.is_on());
    }

    #[tokio::test]
    async fn saving_twice_replaces_rather_than_accumulating() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        let s = WebServerSettings {
            keepalive_timeout: Seconds::parse(30).unwrap(),
            ..WebServerSettings::default()
        };
        repo.save(&s).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM web_server_settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(repo.get().await.unwrap().keepalive_timeout.get(), 30);
    }

    /// Asserts `result` failed with `CoreError::Validation` naming exactly
    /// `expected_field`, panicking with the actual value otherwise. Shared by
    /// the corrupt-row tests below so a failure reports both which field was
    /// expected and what actually came back, instead of a bare `is_err()`
    /// that cannot tell "rejected the right field" from "errored for some
    /// unrelated reason".
    fn assert_names_field<T: std::fmt::Debug>(result: Result<T, CoreError>, expected_field: &str) {
        match result {
            Err(CoreError::Validation { field, .. }) => {
                assert_eq!(field, expected_field, "wrong field named in error");
            }
            other => {
                panic!("expected CoreError::Validation naming {expected_field:?}, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn a_corrupt_gzip_comp_level_is_rejected_on_read_and_names_its_field() {
        // state.db is a file on the user's disk. The repo re-validates on read
        // for the same reason SiteRepository does: nothing unparsed may reach a
        // template, whatever wrote the row. `99` breaks `GzipLevel`'s `1..=9`
        // bound while still fitting a `u32`, so this exercises the newtype's
        // own check, not `to_u32` (see the dedicated downcast test below).
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        sqlx::query("UPDATE web_server_settings SET gzip_comp_level = 99 WHERE id = 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_names_field(repo.get().await, "gzip_comp_level");
    }

    #[tokio::test]
    async fn a_corrupt_worker_connections_is_rejected_on_read_and_names_its_field() {
        // `99999` breaks `WorkerConnections`' `1..=65535` bound while still
        // fitting a `u32`, so this is the newtype's own check, not `to_u32`.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        sqlx::query("UPDATE web_server_settings SET worker_connections = 99999 WHERE id = 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_names_field(repo.get().await, "worker_connections");
    }

    #[tokio::test]
    async fn a_corrupt_gzip_types_is_rejected_on_read_and_names_its_field() {
        // `not_mime` has no `/`, so `is_mime_shaped` rejects it outright —
        // this bypasses the repository entirely via raw SQL, the same way
        // the `client_max_body_size` test above does.
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        sqlx::query("UPDATE web_server_settings SET gzip_types = 'not_mime' WHERE id = 1")
            .execute(db.pool())
            .await
            .unwrap();
        assert_names_field(repo.get().await, "gzip_types");
    }

    /// `Seconds::parse` takes only the out-of-range value (see its signature
    /// in `openvhost_conf::settings::value`), so `ConfError::InvalidField`
    /// always reports `field: "seconds"` regardless of which of the four
    /// `Seconds`-typed columns produced it — a limitation of the current API,
    /// not of this test. **Task 5 of the current plan is to have every
    /// `Seconds` call site pass its own field name through**, so a future
    /// version of this test can assert per-column names the way
    /// `worker_connections`/`gzip_comp_level`/`gzip_types`/
    /// `client_max_body_size` do above. Until then, this is one test over all
    /// four columns (not four tests pretending to be independent) that
    /// proves each column is still independently re-validated: a dropped
    /// `Seconds::parse` call on any one of them fails only that iteration,
    /// naming the column in the panic message, even though the error itself
    /// cannot distinguish them yet.
    #[tokio::test]
    async fn a_corrupt_seconds_field_is_rejected_on_read_for_every_seconds_column() {
        let seconds_columns = [
            "keepalive_timeout",
            "fastcgi_connect_timeout",
            "fastcgi_send_timeout",
            "fastcgi_read_timeout",
        ];
        for column in seconds_columns {
            let db = Db::open_in_memory().await.unwrap();
            let repo = SqliteWebServerSettings::new(&db);
            repo.save(&WebServerSettings::default()).await.unwrap();
            // `99999` breaks `Seconds`' `1..=86400` bound while still fitting
            // a `u32`. `column` comes from the fixed list above, never from
            // untrusted input, so asserting it's SQL-safe here is honest —
            // unlike every value column in this module, which stays
            // parameter-bound.
            let sql = format!("UPDATE web_server_settings SET {column} = 99999 WHERE id = 1");
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(db.pool())
                .await
                .unwrap();

            match repo.get().await {
                Err(CoreError::Validation { field, .. }) => {
                    assert_eq!(
                        field, "seconds",
                        "{column}: Seconds::parse always names its field \"seconds\" today"
                    );
                }
                other => panic!("{column}: expected CoreError::Validation, got {other:?}"),
            }
        }
    }

    /// `to_u32` exists specifically to catch an `i64` that cannot become a
    /// `u32` — negative, or past `u32::MAX` — before it ever reaches a
    /// newtype's own bound check (see `to_u32`'s doc comment). A value that
    /// merely breaks `WorkerConnections`' `1..=65535` bound (like the
    /// `99999` used above) proves the newtype's own check runs, but says
    /// nothing about `to_u32`: `u32::try_from(99999)` succeeds. `-1` and a
    /// value past `u32::MAX` force the downcast itself to fail, and the two
    /// failure modes are told apart by the error's `reason` text — `to_u32`
    /// always says "does not fit in a u32", never "must be between ..." —
    /// so a `-1` that got rejected by `WorkerConnections`' bound instead of
    /// by `to_u32` would fail this assertion rather than passing for the
    /// wrong reason.
    #[tokio::test]
    async fn a_negative_or_oversized_worker_connections_is_rejected_by_the_u32_downcast() {
        for corrupt_value in [-1_i64, i64::from(u32::MAX) + 1] {
            let db = Db::open_in_memory().await.unwrap();
            let repo = SqliteWebServerSettings::new(&db);
            repo.save(&WebServerSettings::default()).await.unwrap();
            sqlx::query("UPDATE web_server_settings SET worker_connections = ? WHERE id = 1")
                .bind(corrupt_value)
                .execute(db.pool())
                .await
                .unwrap();

            match repo.get().await {
                Err(CoreError::Validation { field, reason }) => {
                    assert_eq!(field, "worker_connections");
                    assert!(
                        reason.contains("does not fit in a u32"),
                        "{corrupt_value}: expected the to_u32 downcast's reason, got {reason:?} \
                         — a bound-check failure here would mean to_u32 was bypassed"
                    );
                }
                other => panic!("{corrupt_value}: expected CoreError::Validation, got {other:?}"),
            }
        }
    }

    /// Every field, not just one, must survive a round trip — the brief's
    /// `a_saved_value_survives_a_round_trip` only checks two fields
    /// (`fastcgi_read_timeout`, `gzip`). This pins the rest so a column that
    /// silently drops or mis-maps a value fails a test instead of shipping.
    #[tokio::test]
    async fn every_field_survives_a_round_trip() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        let s = WebServerSettings {
            worker_connections: WorkerConnections::parse(2048).unwrap(),
            client_max_body_size: BodySize::parse("64m").unwrap(),
            keepalive_timeout: Seconds::parse(45).unwrap(),
            tcp_nodelay: OnOff::new(false),
            fastcgi_connect_timeout: Seconds::parse(12).unwrap(),
            fastcgi_send_timeout: Seconds::parse(34).unwrap(),
            fastcgi_read_timeout: Seconds::parse(56).unwrap(),
            gzip: OnOff::new(true),
            gzip_comp_level: GzipLevel::parse(7).unwrap(),
            gzip_types: GzipTypes::parse("text/plain application/json").unwrap(),
        };
        repo.save(&s).await.unwrap();

        let back = repo.get().await.unwrap();
        assert_eq!(back, s);
    }

    /// The whole justification for re-validating on read is that a corrupt
    /// row is rejected, not merely detected on one hand-picked column. This
    /// hand-edits a column via raw SQL that bypasses the repository entirely
    /// (`BodySize` never accepts unit-less "not-a-size") and checks the
    /// error names the right field, so the previous test's bare `is_err()`
    /// cannot pass for the wrong reason.
    #[tokio::test]
    async fn a_corrupt_body_size_is_rejected_on_read_and_names_its_field() {
        let db = Db::open_in_memory().await.unwrap();
        let repo = SqliteWebServerSettings::new(&db);
        repo.save(&WebServerSettings::default()).await.unwrap();
        sqlx::query(
            "UPDATE web_server_settings SET client_max_body_size = 'not-a-size' WHERE id = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();

        match repo.get().await {
            Err(CoreError::Validation { field, .. }) => {
                assert_eq!(field, "client_max_body_size");
            }
            other => panic!("expected CoreError::Validation, got {other:?}"),
        }
    }
}
