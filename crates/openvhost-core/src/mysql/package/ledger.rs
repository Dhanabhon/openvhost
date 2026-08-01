// SPDX-License-Identifier: GPL-3.0-or-later
//! The install ledger: what THIS app fetched into its own `packages/` tree,
//! and when (MySQL-from-tarball design D4 — "the version is recorded at
//! install and never probed again").
//!
//! We asked the compiled-in catalogue for a specific version, so we know
//! exactly what we installed. Recording it closes the defect the previous
//! slice's live proof found, where a *successful* install reported "not
//! detected" because probing the freshly extracted binary for its version
//! could not outlast macOS's first-execution scan. Probing survives only for
//! discovering Homebrew runtimes we did not install.
//!
//! **This table is a ledger, not an inventory.** The package tree is the
//! inventory: `packages/<name>/<major>/<version>/` exists if and only if that
//! version is installed, and the per-major `current` symlink says which one is
//! selected. Nothing may read this table to decide what is installed — a row
//! survives a tree deleted out from under us. It records the two facts the
//! tree cannot: that we installed it, and when.
//!
//! Rows re-validate on read, the same "never trust a hand-edited state.db"
//! discipline [`crate::mysql::MysqlInstanceRepo`] and [`crate::site::repo`]
//! apply.

use sqlx::SqlitePool;

use crate::db::{Db, now_ms};
use crate::error::CoreError;

/// Longest accepted ledger component. Matches `openvhost-pkg`'s own path
/// component limit in value but is deliberately an independent constant in an
/// independent crate: what a *database* will store and what a *filesystem*
/// path may contain are different questions, and coupling them would mean
/// relaxing one silently relaxes the other.
const MAX_COMPONENT_BYTES: usize = 64;

/// Reserved Windows device basenames, checked without a trailing extension.
///
/// Mirrors `openvhost-pkg`'s own list in value. An independent constant in an
/// independent crate for the same reason [`MAX_COMPONENT_BYTES`] is one — that
/// crate's copy is `pub(crate)` and unimportable, and coupling the two would
/// mean relaxing one silently relaxes the other. What must NOT diverge is the
/// SET. `the_write_guard_refuses_everything_the_installer_refuses` pins that
/// every name in THIS list is refused — it does not keep the two lists in
/// step, because it spells the names out a third time. A 25th entry added to
/// `openvhost_pkg`'s `RESERVED` would leave both this list and that test stale
/// and green. Closing that properly means asserting set equality against
/// pkg's list, which needs it made public.
const RESERVED_STEMS: [&str; 24] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Reject anything that is not a safe single path component.
///
/// Applied on write AND on read. On write because these values name the
/// directory the package lives in, so a ledger row can never disagree with a
/// path the installer would accept; on read because a row is only as
/// trustworthy as the file it came from, and a hand-edited `state.db` is
/// precisely what re-validation exists to catch.
///
/// AUDIT F3: the reserved-device-name rule below was missing, so this guard
/// diverged from `openvhost_pkg::request::validate_component` in exactly the
/// direction the paragraph above says it must not — `record("nul", …)`
/// succeeded where the installer refuses `nul`. Inert on macOS, live on
/// Windows, and the divergence itself is the defect: the write-side check earns
/// its keep only by accepting precisely what the installer accepts.
fn check_component(field: &'static str, s: &str) -> Result<(), CoreError> {
    let bad = |reason: String| CoreError::Validation { field, reason };
    if s.is_empty() || s.len() > MAX_COMPONENT_BYTES {
        return Err(bad(format!(
            "{s:?} must be 1..={MAX_COMPONENT_BYTES} bytes"
        )));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(bad(format!("{s:?} may only contain [a-z0-9._-]")));
    }
    if s.starts_with('.') || s.starts_with('-') || s.ends_with('.') {
        return Err(bad(format!(
            "{s:?} must not start with . or - or end with ."
        )));
    }
    // The stem is everything before the first dot: `nul.tar` names the same
    // device as `nul` on Windows. The charset rule above already forced lower
    // case, so no case folding is needed here.
    let stem = s.split('.').next().unwrap_or(s);
    if RESERVED_STEMS.contains(&stem) {
        return Err(bad(format!("{s:?} is a reserved device name")));
    }
    Ok(())
}

/// One recorded install: which package version, and when it landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// The package tree's top-level name, e.g. `"mysql"`.
    pub name: String,
    /// The `major.minor` series, e.g. `"8.4"`.
    pub major: String,
    /// The exact version, e.g. `"8.4.11"` — the value design D4 exists to
    /// preserve.
    pub version: String,
    /// Milliseconds since the Unix epoch at the moment the install completed.
    pub installed_at: i64,
}

/// Raw DB row (primitive columns) — decoded by sqlx, then re-validated by the
/// `TryFrom` below.
struct LedgerRow {
    name: String,
    major: String,
    version: String,
    installed_at: i64,
}

impl TryFrom<LedgerRow> for LedgerEntry {
    type Error = CoreError;

    fn try_from(r: LedgerRow) -> Result<Self, CoreError> {
        check_component("package_name", &r.name)?;
        check_component("package_major", &r.major)?;
        check_component("package_version", &r.version)?;
        Ok(LedgerEntry {
            name: r.name,
            major: r.major,
            version: r.version,
            installed_at: r.installed_at,
        })
    }
}

/// SQLite-backed [`LedgerEntry`] storage over `state.db`'s
/// `installed_packages` table.
pub struct InstallLedger {
    pool: SqlitePool,
}

impl InstallLedger {
    /// Build a ledger over the given database's connection pool.
    pub fn new(db: &Db) -> Self {
        Self {
            pool: db.pool().clone(),
        }
    }

    /// Record that `name` `version` (of series `major`) was installed, now.
    /// Returns the timestamp written.
    ///
    /// Re-recording the same triple replaces the timestamp rather than
    /// duplicating the row: a reinstall of the same version is one fact about
    /// when it most recently landed, not two.
    ///
    /// `pub(crate)`, not `pub` (audit F3). The only writer is
    /// `crate::mysql::package::install::install_entry`, whose three components
    /// are `&'static str` from the compiled-in catalogue — so "is
    /// [`check_component`] alone enough to sit between a caller's string and a
    /// derived path?" stops being a question anything outside this crate can
    /// ask. The guard stays and is now complete; this narrows who can lean on
    /// it. `get`/`list` remain public: reading is where a hand-edited
    /// `state.db` has to be re-validated, and that is a service to callers.
    pub(crate) async fn record(
        &self,
        name: &str,
        major: &str,
        version: &str,
    ) -> Result<i64, CoreError> {
        check_component("package_name", name)?;
        check_component("package_major", major)?;
        check_component("package_version", version)?;
        let ts = now_ms();
        sqlx::query!(
            "INSERT INTO installed_packages (name, major, version, installed_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(name, major, version) DO UPDATE SET \
               installed_at = excluded.installed_at",
            name,
            major,
            version,
            ts,
        )
        .execute(&self.pool)
        .await?;
        Ok(ts)
    }

    /// The recorded install of one exact version, or `Ok(None)` if this app
    /// never installed it.
    pub async fn get(
        &self,
        name: &str,
        major: &str,
        version: &str,
    ) -> Result<Option<LedgerEntry>, CoreError> {
        let row = sqlx::query_as!(
            LedgerRow,
            "SELECT name, major, version, installed_at FROM installed_packages \
             WHERE name = ? AND major = ? AND version = ?",
            name,
            major,
            version
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(LedgerEntry::try_from).transpose()
    }

    /// Every version of `name` this app installed, newest install first.
    ///
    /// Callers must still confirm each version's directory is really on disk
    /// before treating it as installed — see this module's header: the tree is
    /// the inventory, this is the ledger.
    pub async fn list(&self, name: &str) -> Result<Vec<LedgerEntry>, CoreError> {
        let rows = sqlx::query_as!(
            LedgerRow,
            "SELECT name, major, version, installed_at FROM installed_packages \
             WHERE name = ? ORDER BY installed_at DESC, version DESC",
            name
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(LedgerEntry::try_from).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    async fn ledger() -> (Db, InstallLedger) {
        let db = Db::open_in_memory().await.unwrap();
        let ledger = InstallLedger::new(&db);
        (db, ledger)
    }

    // ------------------------------------------------------------------
    // Group 1 — the recorded-version round trip.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_version_this_app_never_installed_is_absent() {
        let (_db, ledger) = ledger().await;
        assert!(
            ledger
                .get("mysql", "8.4", "8.4.11")
                .await
                .unwrap()
                .is_none()
        );
        assert!(ledger.list("mysql").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recording_a_version_round_trips_it_exactly() {
        let (_db, ledger) = ledger().await;
        let at = ledger.record("mysql", "8.4", "8.4.11").await.unwrap();

        let got = ledger
            .get("mysql", "8.4", "8.4.11")
            .await
            .unwrap()
            .expect("the version just recorded must be readable");
        assert_eq!(got.name, "mysql");
        assert_eq!(got.major, "8.4");
        assert_eq!(got.version, "8.4.11");
        assert_eq!(got.installed_at, at);
        assert!(at > 0, "timestamp must be a real epoch value, got {at}");
    }

    /// The exact confusion design D4 exists to prevent: the ledger must return
    /// the FULL version, never the major it is filed under.
    #[tokio::test]
    async fn the_recorded_version_is_the_full_version_not_the_major() {
        let (_db, ledger) = ledger().await;
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();
        let got = ledger.get("mysql", "8.4", "8.4.11").await.unwrap().unwrap();
        assert_ne!(got.version, got.major);
        assert_eq!(got.version, "8.4.11");
    }

    #[tokio::test]
    async fn a_lookup_for_a_different_version_of_the_same_major_misses() {
        let (_db, ledger) = ledger().await;
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();
        assert!(
            ledger
                .get("mysql", "8.4", "8.4.10")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ledger
                .get("mysql", "8.0", "8.4.11")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            ledger
                .get("nginx", "8.4", "8.4.11")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn two_versions_of_one_major_coexist_because_the_tree_allows_it() {
        let (_db, ledger) = ledger().await;
        ledger.record("mysql", "8.4", "8.4.10").await.unwrap();
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();

        let all = ledger.list("mysql").await.unwrap();
        assert_eq!(all.len(), 2, "got {all:?}");
        let versions: Vec<&str> = all.iter().map(|e| e.version.as_str()).collect();
        assert!(versions.contains(&"8.4.10"));
        assert!(versions.contains(&"8.4.11"));
    }

    #[tokio::test]
    async fn recording_the_same_version_twice_replaces_rather_than_accumulates() {
        let (db, ledger) = ledger().await;
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM installed_packages")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "a reinstall is one fact, not two rows");
    }

    #[tokio::test]
    async fn packages_are_listed_independently_by_name() {
        let (_db, ledger) = ledger().await;
        ledger.record("mysql", "8.4", "8.4.11").await.unwrap();
        ledger.record("nginx", "1.28", "1.28.0").await.unwrap();

        assert_eq!(ledger.list("mysql").await.unwrap().len(), 1);
        assert_eq!(ledger.list("nginx").await.unwrap().len(), 1);
        assert_eq!(ledger.list("php").await.unwrap().len(), 0);
    }

    // ------------------------------------------------------------------
    // Group 2 — a ledger row can never disagree with a path the installer
    // would accept, in either direction.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_component_that_could_steer_a_path_is_refused_on_write() {
        let (db, ledger) = ledger().await;
        for bad in [
            "",
            "..",
            ".",
            "../etc",
            "my/sql",
            "MySQL",
            "a b",
            ".hidden",
            "-rf",
            "trailing.",
        ] {
            assert!(
                ledger.record(bad, "8.4", "8.4.11").await.is_err(),
                "accepted name {bad:?}"
            );
            assert!(
                ledger.record("mysql", bad, "8.4.11").await.is_err(),
                "accepted major {bad:?}"
            );
            assert!(
                ledger.record("mysql", "8.4", bad).await.is_err(),
                "accepted version {bad:?}"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM installed_packages")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "a refused write must not reach the table");
    }

    /// AUDIT F3: this guard's own header says a ledger row can never disagree
    /// with a path the installer would accept — and it did, for every reserved
    /// Windows device name. `record("nul", …)` succeeded where
    /// `openvhost_pkg::request::validate_component("nul")` refuses.
    ///
    /// The sweep is over the whole list rather than one example, and covers
    /// `nul.tar` too: the rule is about the STEM before the first dot, so a
    /// check that compared the whole string would pass on `nul` and let
    /// `nul.tar` through.
    #[tokio::test]
    async fn the_write_guard_refuses_everything_the_installer_refuses() {
        let (db, ledger) = ledger().await;
        let reserved = [
            "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6",
            "com7", "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7",
            "lpt8", "lpt9",
        ];
        for name in reserved {
            for spelling in [
                name.to_string(),
                format!("{name}.tar"),
                format!("{name}.8.4"),
            ] {
                assert!(
                    ledger.record(&spelling, "8.4", "8.4.11").await.is_err(),
                    "accepted reserved name {spelling:?}"
                );
                assert!(
                    ledger.record("mysql", &spelling, "8.4.11").await.is_err(),
                    "accepted reserved major {spelling:?}"
                );
                assert!(
                    ledger.record("mysql", "8.4", &spelling).await.is_err(),
                    "accepted reserved version {spelling:?}"
                );
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM installed_packages")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "a refused write must not reach the table");
    }

    /// Non-vacuity twin: the rule bites on the reserved STEM and nothing else.
    /// A guard that rejected every name containing those letters would pass the
    /// sweep above and break every real package.
    #[tokio::test]
    async fn a_name_that_merely_contains_a_reserved_word_is_still_accepted() {
        let (_db, ledger) = ledger().await;
        for ok in ["console", "aux-tools", "nullify", "com10", "lpt", "mysql"] {
            assert!(
                ledger.record(ok, "8.4", "8.4.11").await.is_ok(),
                "refused legitimate name {ok:?}"
            );
        }
    }

    /// And a reserved name that reached the table anyway — a hand-edited
    /// `state.db`, the case the read-side re-validation exists for — is refused
    /// on the way out rather than handed back as a path component.
    #[tokio::test]
    async fn a_reserved_name_written_by_hand_is_refused_on_read() {
        let (db, ledger) = ledger().await;
        sqlx::query(
            "INSERT INTO installed_packages (name, major, version, installed_at) \
             VALUES ('mysql', '8.4', 'nul', 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let err = ledger.get("mysql", "8.4", "nul").await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Validation {
                    field: "package_version",
                    ..
                }
            ),
            "got {err:?}"
        );
        assert!(ledger.list("mysql").await.is_err());
    }

    #[tokio::test]
    async fn a_hand_edited_row_is_refused_on_read_rather_than_returned() {
        let (db, ledger) = ledger().await;
        // Bypass `record` entirely — this is the hand-edited-database case.
        sqlx::query(
            "INSERT INTO installed_packages (name, major, version, installed_at) \
             VALUES ('mysql', '8.4', '../../../etc', 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let err = ledger
            .get("mysql", "8.4", "../../../etc")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Validation {
                    field: "package_version",
                    ..
                }
            ),
            "got {err:?}"
        );
        assert!(ledger.list("mysql").await.is_err());
    }

    #[tokio::test]
    async fn a_clean_row_written_by_hand_still_reads_back() {
        // Non-vacuity twin of the test above: re-validation must reject the
        // dangerous row specifically, not every row that skipped `record`.
        let (db, ledger) = ledger().await;
        sqlx::query(
            "INSERT INTO installed_packages (name, major, version, installed_at) \
             VALUES ('mysql', '8.4', '8.4.11', 7)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let got = ledger.get("mysql", "8.4", "8.4.11").await.unwrap().unwrap();
        assert_eq!(got.version, "8.4.11");
        assert_eq!(got.installed_at, 7);
    }
}
