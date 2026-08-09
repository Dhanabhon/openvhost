// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing our own PHP build into OpenVHost's package tree — the fourth
//! consumer of `openvhost-pkg`, and (like nginx and MariaDB) for an artifact
//! we produced ourselves rather than downloaded from upstream (php-recipe
//! design, and build-pipeline design D5).
//!
//! This module is wiring, and deliberately thin: it resolves a pinned
//! catalogue entry for the requested major, hands it to the existing
//! download → SHA-256 verify → extract → atomic-install pipeline, and
//! records the version it asked for. It adds no install machinery of its
//! own — no second downloader, no second extractor, no second notion of
//! where a package lives. It is a near-exact transcription of
//! [`crate::nginx::install_nginx_package`]'s wiring, with a `major` argument
//! threaded through the way [`crate::mysql::install_mysql_package`] already
//! threads one — consistency beats novelty, and a second shape here would be
//! a finding rather than a design.
//!
//! SECURITY: the public entry point takes a [`PhpMajor`] and nothing else
//! that reaches the downloader. The URL and hash come only from
//! [`crate::php::PHP_PACKAGES`], so there is no argument any caller — IPC,
//! CLI or otherwise — can supply that changes which bytes are fetched or what
//! they must hash to. Verification happens before extraction, on the same file
//! handle, and the extracted tree is renamed into place atomically, so a
//! failure at any stage leaves the package tree exactly as it was.
//!
//! **The pinned release is not published yet** (see the catalogue's header), so
//! the public entry point refuses with [`CoreError::PackageNotPublished`]
//! before any network or filesystem work. That refusal is the reason a 404 can
//! never reach a user through this path, and it is enforced by an exhaustive
//! match on [`Availability`] rather than by nothing being wired to it.
//!
//! What this module never does, on any path including error paths: write to
//! `<home>/data/`, write to `<home>/logs/`, or touch a stored credential —
//! PHP has no datadir or credential of its own, and this slice does not give
//! it one; installing a copy of the binaries is not running them.
//!
//! **Scope.** Installing a runtime is not running one. Discovering an
//! existing package install, choosing it over Homebrew's, generating a pool
//! config or starting php-fpm and any Languages-page row are later slices
//! (5B/5C) and are deliberately absent here (php-recipe design §10 — this
//! slice ends at a verified, installable artifact).

use openvhost_pkg::{InstallRequest, InstalledPackage, PackagesRoot, Progress};

use crate::error::CoreError;
use crate::mysql::{InstallLedger, LedgerWrite, NO_LEDGER_REASON};
use crate::php::PhpMajor;
use crate::php::package::catalogue::{
    Availability, PHP_PACKAGE_NAME, PHP_WARMUP_BINARY, PhpPackage, php_package_for_host,
};

/// The result of installing one catalogued PHP build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpPackageInstall {
    /// Where it landed, and the `current` link that now points at it.
    pub package: InstalledPackage,
    /// Whether the install ledger recorded it. [`LedgerWrite`] is shared with
    /// the MySQL, MariaDB and nginx install paths rather than re-declared: it
    /// describes the ledger, which is package-agnostic, not the package.
    pub ledger: LedgerWrite,
}

/// Install the catalogued PHP build for `major` on this host.
///
/// Resolves the pinned entry, streams it to a private staging directory,
/// verifies its SHA-256 **before** anything parses the bytes, extracts through
/// the hardened walk, pre-pays macOS's first-execution signature check on
/// `bin/php-fpm` while the tree is still in staging, renames the tree
/// atomically into `packages/php/<major>/<version>/`, swings the per-major
/// `current` link onto it, and records the exact version in `state.db`.
///
/// `progress` receives [`Progress`] events as the pipeline advances. The events
/// a user should be able to distinguish — a download that was verified from one
/// that merely arrived — are distinct variants, not log text.
///
/// **`ledger` is optional** (5C audit LOW-4). `state.db` is opened best-effort
/// at startup, so a machine whose store is missing or unreadable has no
/// [`InstallLedger`] to hand over — and refusing the install there would make a
/// degraded `state.db` into "PHP cannot be installed", which is exactly what
/// [`LedgerWrite`] exists to avoid saying. Pass `None` only for that: the
/// package tree IS the inventory, so a skipped row costs provenance and never
/// correctness, and it is reported as [`LedgerWrite::Failed`] rather than
/// silently omitted. This was PHP's alone until the optional-state.db design's
/// D4 gave [`crate::mysql::install_mysql_package`] and
/// [`crate::mariadb::install_mariadb_package`] the same signature for the same
/// reason — their commands used to take a `State<Db>`, so Tauri refused the
/// whole call before their installers were reached and the question never arose
/// for them.
///
/// **Cancellation.** As with the MySQL, MariaDB and nginx paths: dropping the
/// returned future is the cancel. The staging directory is an RAII temporary
/// removed as the future unwinds, and the process-wide install permit is
/// released with it.
///
/// # Errors
///
/// - [`CoreError::NoPackageForTarget`] — before any network or filesystem
///   work, if this build publishes no artifact for `major` at all (only 8.4
///   is pinned today — see the catalogue's header) or none for this host's
///   architecture.
/// - [`CoreError::PackageNotPublished`] — before any network or filesystem
///   work, while the release hosting the pinned artifact does not exist yet.
///   This is the state today for 8.4.
///
/// A ledger that cannot be written is **not** an error: see `ledger` above.
pub async fn install_php_package(
    major: &PhpMajor,
    root: &PackagesRoot,
    ledger: Option<&InstallLedger>,
    progress: impl FnMut(Progress) + Send,
) -> Result<PhpPackageInstall, CoreError> {
    let entry = php_package_for_host(major)?;
    // Exhaustive on purpose, with no wildcard arm: a third availability state
    // must fail to compile here rather than be silently treated as fetchable.
    match entry.availability {
        Availability::AwaitingRelease { tag } => Err(CoreError::PackageNotPublished {
            name: PHP_PACKAGE_NAME,
            version: entry.version,
            tag,
            url: entry.url,
        }),
        Availability::Published => install_entry(entry, root, ledger, progress).await,
    }
}

/// The pipeline itself, split out from the catalogue lookup so tests can drive
/// it against a loopback fixture — and so the live proof can install the real
/// tarball from a local source while the release remains unpublished.
///
/// Private on purpose: taking a [`PhpPackage`] means taking a URL and a
/// hash, and the whole point of the public signature above is that no caller
/// can choose those. Nothing outside this module may name this function.
async fn install_entry(
    entry: &PhpPackage,
    root: &PackagesRoot,
    ledger: Option<&InstallLedger>,
    progress: impl FnMut(Progress) + Send,
) -> Result<PhpPackageInstall, CoreError> {
    // Every component below is a compiled-in `&'static str` from the catalogue,
    // so the path this install writes to is fixed at compile time even though
    // the lookup key (`major`) was not.
    let request = InstallRequest::new(
        PHP_PACKAGE_NAME,
        entry.major,
        entry.version,
        entry.url,
        entry.sha256,
        entry.format,
    )?
    .with_warmup_binary(PHP_WARMUP_BINARY)?;

    let package = openvhost_pkg::install_package(&request, root, progress).await?;

    // MySQL-from-tarball design D4, reused unchanged: we asked for this
    // version, so we know it. Recorded only after the tree is on disk — a
    // failed install must leave no phantom row.
    //
    // The `None` arm is 5C audit LOW-4: no store to write to is the same
    // OUTCOME as a write that failed — the tree is installed, the row is not
    // there — so it reports as one rather than as a second, quieter kind of
    // nothing. See this module's public entry point for who passes `None`.
    let ledger_write = match ledger {
        Some(ledger) => match ledger
            .record(&package.name, &package.major, &package.version)
            .await
        {
            Ok(installed_at) => LedgerWrite::Recorded { installed_at },
            Err(e) => {
                tracing::error!(
                    name = %package.name,
                    version = %package.version,
                    dir = %package.dir.display(),
                    error = %e,
                    "PHP is installed but its ledger row could not be written"
                );
                LedgerWrite::Failed {
                    reason: e.to_string(),
                }
            }
        },
        None => {
            tracing::warn!(
                name = %package.name,
                version = %package.version,
                dir = %package.dir.display(),
                "PHP is installed but state.db was unavailable, so nothing recorded it"
            );
            LedgerWrite::Failed {
                reason: NO_LEDGER_REASON.to_string(),
            }
        }
    };

    tracing::info!(
        version = %package.version,
        dir = %package.dir.display(),
        "installed PHP from OpenVHost's own build"
    );

    Ok(PhpPackageInstall {
        package,
        ledger: ledger_write,
    })
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use openvhost_pkg::{ArchiveFormat, PkgError};

    use crate::PackageTarget;
    use crate::db::Db;
    use crate::mysql::{MysqlInstanceRepo, MysqlMajor, generate_root_password};

    // ------------------------------------------------------------------
    // Fixtures.
    // ------------------------------------------------------------------

    /// A `.tar.gz` shaped like our own PHP tarball (measured against
    /// `build/out/php-8.4.24-macos-arm64.tar.gz`, which has exactly this
    /// layout): one implicit top-level directory the extractor strips,
    /// `bin/php-fpm` AND `bin/php` — the tarball ships both, only the
    /// former is warmed — plus one shared-extension file under `modules/`.
    fn php_shaped_targz(php_fpm: &str, php_cli: &str) -> Vec<u8> {
        use flate2::{Compression, write::GzEncoder};
        let gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut ar = tar::Builder::new(gz);
        let entries: [(&str, &str, u32); 3] = [
            ("php-8.4.24/bin/php-fpm", php_fpm, 0o755),
            ("php-8.4.24/bin/php", php_cli, 0o755),
            ("php-8.4.24/modules/opcache.so", "not a real dylib\n", 0o644),
        ];
        for (path, body, mode) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(mode);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            ar.append_data(&mut h, path, body.as_bytes()).unwrap();
        }
        ar.into_inner().unwrap().finish().unwrap()
    }

    /// A 0755 `#!/bin/sh` body. Absolute program paths only: the warm-up child
    /// is handed an allow-list environment, not the ambient one.
    fn script(body: &str) -> String {
        format!("#!/bin/sh\n{body}\n")
    }

    fn sha_hex(b: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b))
    }

    /// Serve `body` once over loopback HTTP. Debug builds accept plain http to
    /// a loopback host (`openvhost-pkg`'s `validate_https_url`), so these tests
    /// need no TLS.
    fn serve_once(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = s.write_all(hdr.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/php.tar.gz")
    }

    /// Build a catalogue-entry-shaped value pointing at a local fixture.
    /// Test-only: production entries are `&'static str` literals compiled into
    /// the binary, which is exactly why `install_entry` is private.
    ///
    /// Marked [`Availability::Published`] because for a fixture it is simply
    /// true — the loopback server really is serving those bytes. The pinned
    /// entry's own availability is asserted in the catalogue's tests.
    fn entry_for(url: String, sha256: String) -> PhpPackage {
        PhpPackage {
            major: "8.4",
            version: "8.4.24",
            target: PackageTarget::MacosArm64,
            url: Box::leak(url.into_boxed_str()),
            sha256: Box::leak(sha256.into_boxed_str()),
            format: ArchiveFormat::TarGz,
            availability: Availability::Published,
            upstream_released_on: "2026-07-30",
            last_checked_on: "2026-08-07",
        }
    }

    struct Fixture {
        _home: tempfile::TempDir,
        home: PathBuf,
        root: PackagesRoot,
        db: Db,
        ledger: InstallLedger,
        /// Scratch directory OUTSIDE the package tree that fixture scripts
        /// write their evidence into.
        evidence: PathBuf,
    }

    impl Fixture {
        /// A home under `/tmp` (never `$TMPDIR` — the 103-byte `sun_path`
        /// ceiling has bitten this project twice) with plausible MariaDB *and*
        /// MySQL datadirs, a log file, and a stored MySQL root credential
        /// already in place: the things no install path may touch, PHP's
        /// least of all — this slice gives it no data or credentials of its
        /// own to disturb anything else with.
        async fn new() -> Fixture {
            let home = tempfile::Builder::new()
                .prefix("ovh-php-pkg")
                .tempdir_in("/tmp")
                .unwrap();
            let h = home.path().to_path_buf();
            let root = PackagesRoot::from_home(&h);
            std::fs::create_dir_all(root.as_path()).unwrap();

            std::fs::create_dir_all(h.join("data/mariadb/11.4")).unwrap();
            std::fs::write(h.join("data/mariadb/11.4/ibdata1"), b"PRECIOUS USER DATA").unwrap();
            std::fs::create_dir_all(h.join("data/mysql/8.4")).unwrap();
            std::fs::write(h.join("data/mysql/8.4/ibdata1"), b"SOMEONE ELSE'S DATA").unwrap();
            std::fs::create_dir_all(h.join("logs")).unwrap();
            std::fs::write(h.join("logs/mariadb-11.4.err"), b"an existing error log").unwrap();
            let evidence = h.join("evidence");
            std::fs::create_dir_all(&evidence).unwrap();

            let db = Db::open_in_memory().await.unwrap();
            MysqlInstanceRepo::new(&db)
                .upsert(
                    &MysqlMajor::parse("8.4").unwrap(),
                    &generate_root_password(),
                )
                .await
                .unwrap();
            let ledger = InstallLedger::new(&db);

            Fixture {
                _home: home,
                home: h,
                root,
                db,
                ledger,
                evidence,
            }
        }

        fn version_dir(&self) -> PathBuf {
            self.root.package_dir("php", "8.4", "8.4.24")
        }

        fn current_link(&self) -> PathBuf {
            self.root.current_link("php", "8.4")
        }

        /// Every live staging directory under the package root.
        fn staging_dirs(&self) -> Vec<PathBuf> {
            match std::fs::read_dir(self.root.staging_root()) {
                Err(_) => Vec::new(),
                Ok(rd) => rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect(),
            }
        }

        fn assert_no_package_tree(&self, why: &str) {
            assert!(
                !self.version_dir().exists(),
                "{why}: a partial tree was left at {}",
                self.version_dir().display()
            );
            assert!(
                !self.current_link().exists() && self.current_link().symlink_metadata().is_err(),
                "{why}: a `current` link was left at {}",
                self.current_link().display()
            );
            assert!(
                self.staging_dirs().is_empty(),
                "{why}: staging directories were left behind: {:?}",
                self.staging_dirs()
            );
        }

        async fn ledger_rows(&self) -> usize {
            self.ledger.list("php").await.unwrap().len()
        }
    }

    /// Identity AND content. The inode is the load-bearing half: a
    /// delete-and-recreate with byte-identical content passes a content-only
    /// check, and this project has shipped exactly that mistake before.
    #[derive(Debug, PartialEq, Eq)]
    struct Fingerprint {
        dev: u64,
        ino: u64,
        mode: u32,
        bytes: Option<Vec<u8>>,
    }

    fn fingerprint(path: &Path) -> Fingerprint {
        let md = std::fs::symlink_metadata(path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()));
        let bytes = md.is_file().then(|| std::fs::read(path).unwrap());
        Fingerprint {
            dev: md.dev(),
            ino: md.ino(),
            mode: md.mode(),
            bytes,
        }
    }

    /// The paths no install path may write to, plus the stored credential.
    struct Sanctuary {
        paths: Vec<PathBuf>,
        before: Vec<Fingerprint>,
        credential: Option<String>,
    }

    impl Sanctuary {
        async fn snapshot(fx: &Fixture) -> Sanctuary {
            let paths = vec![
                fx.home.join("data"),
                fx.home.join("data/mariadb"),
                fx.home.join("data/mariadb/11.4"),
                fx.home.join("data/mariadb/11.4/ibdata1"),
                fx.home.join("data/mysql"),
                fx.home.join("data/mysql/8.4"),
                fx.home.join("data/mysql/8.4/ibdata1"),
                fx.home.join("logs"),
                fx.home.join("logs/mariadb-11.4.err"),
            ];
            let before = paths.iter().map(|p| fingerprint(p)).collect();
            let credential = MysqlInstanceRepo::new(&fx.db)
                .get(&MysqlMajor::parse("8.4").unwrap())
                .await
                .unwrap()
                .map(|i| i.root_password.expose().to_string());
            Sanctuary {
                paths,
                before,
                credential,
            }
        }

        async fn assert_untouched(&self, fx: &Fixture, why: &str) {
            for (path, before) in self.paths.iter().zip(&self.before) {
                let after = fingerprint(path);
                assert_eq!(
                    *before,
                    after,
                    "{why}: {} changed identity or content",
                    path.display()
                );
            }
            let after = MysqlInstanceRepo::new(&fx.db)
                .get(&MysqlMajor::parse("8.4").unwrap())
                .await
                .unwrap()
                .map(|i| i.root_password.expose().to_string());
            assert_eq!(self.credential, after, "{why}: the root credential changed");
            assert!(after.is_some(), "{why}: the credential row vanished");
        }
    }

    /// Collect progress events from a run.
    fn recorder() -> (Arc<Mutex<Vec<Progress>>>, impl FnMut(Progress) + Send) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        (seen, move |p| {
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(p)
        })
    }

    // ------------------------------------------------------------------
    // Group 1 — a successful install lands where the catalogue says, warms the
    // right binary, and records the version — or says why it could not.
    //
    // Vacuity: every assertion is against a path or value the fixture does not
    // pre-create, and each test asserts a positive fact about the finished tree
    // before drawing any conclusion. Proven by mutation — pointing
    // `PHP_PACKAGE_NAME` at "mysql" moved the install to the wrong tree and
    // failed this group. `an_install_with_no_ledger_still_lands_…` is the one
    // test here that group-wide mutation does not cover, because it guards an
    // arm nothing else reaches; it carries its own mutation record.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_successful_install_lands_at_the_catalogue_version_and_links_current() {
        let fx = Fixture::new().await;
        let archive = php_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        let out = install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap();

        assert_eq!(out.package.dir, fx.version_dir());
        assert_eq!(out.package.name, "php");
        assert_eq!(out.package.major, "8.4");
        assert_eq!(out.package.version, "8.4.24");
        assert!(
            fx.version_dir().join("bin/php-fpm").is_file(),
            "the implicit archive root must be stripped so bin/php-fpm sits at the top"
        );
        assert_eq!(
            std::fs::read_link(fx.current_link()).unwrap(),
            PathBuf::from("8.4.24"),
            "`current` must point at the version we just installed"
        );
    }

    /// The trap this rules out, matching MySQL's and MariaDB's `-safe`
    /// wrapper tests: the tarball ships TWO binaries under `bin/`, and only
    /// `bin/php-fpm` — the one the app supervises — may be warmed.
    #[tokio::test]
    async fn the_install_warms_php_fpm_and_never_the_cli_binary() {
        let fx = Fixture::new().await;
        let warmed = fx.evidence.join("php-fpm-ran");
        let forbidden = fx.evidence.join("php-cli-ran");
        let archive = php_shaped_targz(
            &script(&format!("/usr/bin/touch {}", warmed.display())),
            &script(&format!("/usr/bin/touch {}", forbidden.display())),
        );
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap();

        assert!(
            warmed.is_file(),
            "bin/php-fpm was never warmed: the Gatekeeper cost lands on the user's \
             first Start"
        );
        assert!(
            !forbidden.exists(),
            "bin/php was executed — that is not the binary this app supervises"
        );
    }

    /// MySQL-from-tarball design D4's whole point, exercised through the real
    /// install rather than against the ledger in isolation: the version we
    /// asked the catalogue for is the version that round-trips out of
    /// `state.db`.
    #[tokio::test]
    async fn the_install_records_the_exact_version_it_asked_the_catalogue_for() {
        let fx = Fixture::new().await;
        let archive = php_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        let out = install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap();

        let installed_at = match out.ledger {
            LedgerWrite::Recorded { installed_at } => installed_at,
            LedgerWrite::Failed { reason } => panic!("ledger write failed: {reason}"),
        };
        let row = fx
            .ledger
            .get("php", "8.4", "8.4.24")
            .await
            .unwrap()
            .expect("the install must record the version it fetched");
        assert_eq!(row.name, "php");
        assert_eq!(row.major, "8.4");
        assert_eq!(row.version, "8.4.24");
        assert_eq!(row.installed_at, installed_at);
        assert_ne!(
            row.version, row.major,
            "the ledger must hold the exact version, not the major"
        );
        // And it must not have been filed under any other package's name.
        assert!(fx.ledger.list("mysql").await.unwrap().is_empty());
        assert!(fx.ledger.list("mariadb").await.unwrap().is_empty());
        assert!(fx.ledger.list("nginx").await.unwrap().is_empty());
    }

    /// The counterpart to the test above, and the reason `ledger` is an
    /// `Option` at all (5C audit LOW-4): **a degraded `state.db` costs
    /// provenance, never correctness.** With no ledger to write to, the install
    /// must still land in full — tree, warm-up and `current` link — and the
    /// missing row must surface as [`LedgerWrite::Failed`] carrying
    /// [`NO_LEDGER_REASON`], not as an error and not as a silent success.
    ///
    /// The row count is the half that makes this discriminating. Without it the
    /// test could not tell "reported failed" from "reported failed and wrote a
    /// row anyway", which is exactly what a `None` arm bolted onto a working
    /// writer would do. The fixture's own [`InstallLedger`] is a live writer
    /// over the same `state.db`, so a row appearing there would be visible.
    ///
    /// Vacuity: proven by mutation, twice, because the `None` arm has two ways
    /// to be wrong. Returning `Err(CoreError::Internal(…))` from it failed at
    /// the `unwrap()`; returning `LedgerWrite::Recorded { installed_at: … }`
    /// failed the `Failed` match below. Neither mutation disturbed any other
    /// test in this module, which is what makes this test the only thing
    /// holding that arm.
    #[tokio::test]
    async fn an_install_with_no_ledger_still_lands_and_reports_the_missing_row() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let warmed = fx.evidence.join("php-fpm-ran");
        let archive = php_shaped_targz(
            &script(&format!("/usr/bin/touch {}", warmed.display())),
            &script("exit 0"),
        );
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        // `None` is precisely what the desktop app passes when `state.db` never
        // opened: `DbHandle::optional()` is `None` and there is no
        // `InstallLedger` to construct.
        let out = install_entry(&entry, &fx.root, None, |_| {}).await.unwrap();

        // The install landed, in full. Asserted first, because every conclusion
        // below is worthless if nothing ran.
        assert_eq!(out.package.dir, fx.version_dir());
        assert_eq!(out.package.version, "8.4.24");
        assert!(
            fx.version_dir().join("bin/php-fpm").is_file(),
            "a missing ledger cost the package tree, which is the one thing it must never cost"
        );
        assert!(
            warmed.is_file(),
            "the warm-up was skipped, so macOS's first-execution check lands on the user's Start"
        );
        assert_eq!(
            std::fs::read_link(fx.current_link()).unwrap(),
            PathBuf::from("8.4.24"),
            "`current` must point at the version we just installed"
        );
        assert!(
            fx.staging_dirs().is_empty(),
            "staging survived a ledger-less success"
        );

        // …and it said so, in the state that exists to say it.
        match &out.ledger {
            LedgerWrite::Failed { reason } => assert_eq!(
                reason.as_str(),
                NO_LEDGER_REASON,
                "the missing row must carry the no-database reason, not a database error"
            ),
            LedgerWrite::Recorded { .. } => panic!(
                "no ledger was handed over, so nothing can have recorded this: {:?}",
                out.ledger
            ),
        }

        // "Reported failed" and "recorded it anyway" are different facts, and
        // only this assertion separates them.
        assert_eq!(
            fx.ledger_rows().await,
            0,
            "a row appeared for an install that was handed no ledger"
        );
        sanctuary
            .assert_untouched(&fx, "after a ledger-less install")
            .await;
    }

    /// Golden rule 6, made observable: the bytes are verified BEFORE anything
    /// unpacks them, and a user watching progress can tell a verified download
    /// from one that merely arrived.
    #[tokio::test]
    async fn progress_reports_verification_before_extraction_and_linking() {
        let fx = Fixture::new().await;
        let archive = php_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));
        let (seen, sink) = recorder();

        install_entry(&entry, &fx.root, Some(&fx.ledger), sink)
            .await
            .unwrap();

        let events = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let index_of = |want: &Progress| {
            events
                .iter()
                .position(|e| e == want)
                .unwrap_or_else(|| panic!("{want:?} never reported; got {events:?}"))
        };
        let verified = index_of(&Progress::Verified);
        let extracted = index_of(&Progress::Extracted);
        let linked = index_of(&Progress::Linked);
        assert!(
            verified < extracted && extracted < linked,
            "expected Verified < Extracted < Linked, got {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, Progress::Started { .. })),
            "no Started event; got {events:?}"
        );
    }

    // ------------------------------------------------------------------
    // Group 2 — a failed install leaves no partial tree, and nothing under
    // `<home>/data`, `<home>/logs` or the credential store is touched on ANY
    // path.
    //
    // Vacuity: the "no partial tree" assertions are on the FILESYSTEM, not on a
    // `Result`, and `Sanctuary` compares (dev, ino, mode, bytes) rather than
    // content alone — the same shape proven by deliberate mutation against
    // MariaDB's and nginx's identical fixture (see those modules' reports):
    // truncating a guarded file, and separately rewriting one with IDENTICAL
    // bytes via delete-and-recreate, each turned every test in this group
    // red; the second is the one a content-only check would have missed.
    // `a_successful_install_writes_only_under_packages` is the twin that
    // stops the whole group passing because nothing ever ran.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_hash_mismatch_is_refused_and_leaves_no_partial_tree() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let archive = php_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        // The pin says something else entirely — the tampered-payload case, and
        // for an artifact we built ourselves it is also the "someone replaced
        // the release asset" case.
        let entry = entry_for(url, sha_hex(b"not the bytes we pinned"));

        let err = install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap_err();

        assert!(
            matches!(err, CoreError::Package(PkgError::HashMismatch { .. })),
            "a tampered payload must be reported as a hash mismatch, got {err:?}"
        );
        fx.assert_no_package_tree("after a hash mismatch");
        assert_eq!(fx.ledger_rows().await, 0, "a failed install recorded a row");
        sanctuary
            .assert_untouched(&fx, "after a hash mismatch")
            .await;
    }

    /// Non-vacuity twin for the sanctuary assertions: they must also hold on
    /// the SUCCESS path, and the package tree must genuinely appear — so the
    /// "untouched" checks above cannot be passing merely because nothing ever
    /// ran.
    #[tokio::test]
    async fn a_successful_install_writes_only_under_packages() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let archive = php_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap();

        assert!(
            fx.version_dir().join("bin/php-fpm").is_file(),
            "the install did not actually happen, so nothing below is evidence"
        );
        assert!(fx.staging_dirs().is_empty(), "staging survived a success");
        assert_eq!(fx.ledger_rows().await, 1);
        sanctuary
            .assert_untouched(&fx, "after a successful install")
            .await;
    }

    // ------------------------------------------------------------------
    // Group 3 — the unpublished release, and an unpinned major, are each
    // refused before any work.
    //
    // Vacuity: the refusal tests assert the ERROR VARIANT and its fields, plus
    // that no staging root was ever created — not merely `is_err()`. Proven by
    // mutation: replacing the `AwaitingRelease` arm's body with a call to
    // `install_entry` made the unpublished-release test fail with a network
    // error instead of the refusal, which is precisely the confusion the
    // variant exists to prevent (the same mutation nginx's and MariaDB's
    // identical test was proven against).
    // ------------------------------------------------------------------

    /// Publishing is owner-gated and has not happened, so the pinned URL 404s.
    /// The public entry point must therefore refuse *before* touching the
    /// network, and the refusal must name the release a human has to create —
    /// otherwise the gap surfaces to a user as an unexplained download failure.
    #[tokio::test]
    async fn the_unpublished_pin_is_refused_before_any_network_or_filesystem_work() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let major = PhpMajor::parse("8.4").unwrap();

        let err = install_php_package(&major, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap_err();

        match err {
            CoreError::PackageNotPublished {
                name,
                version,
                tag,
                url,
            } => {
                assert_eq!(name, "php");
                assert_eq!(version, "8.4.24");
                assert_eq!(tag, "php-8.4.24");
                assert_eq!(url, crate::php::PHP_PACKAGES[0].url);
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(
            err.to_string().contains("php-8.4.24"),
            "the refusal must name the release to publish; got {err}"
        );
        assert!(
            !fx.root.staging_root().exists(),
            "the refusal happened after staging was created"
        );
        fx.assert_no_package_tree("after refusing an unpublished pin");
        assert_eq!(fx.ledger_rows().await, 0);
        sanctuary
            .assert_untouched(&fx, "after refusing an unpublished pin")
            .await;
    }

    /// The public entry point threads `major` all the way to the catalogue
    /// lookup: a major with no pinned package build is refused before any
    /// work too, and BEFORE the availability check (there is no availability
    /// to check — there is no entry).
    #[tokio::test]
    async fn a_major_with_no_pinned_package_build_is_refused_before_any_work() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let major = PhpMajor::parse("8.5").unwrap();

        let err = install_php_package(&major, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap_err();

        assert!(
            matches!(err, CoreError::NoPackageForTarget { .. }),
            "got {err:?}"
        );
        assert!(
            !fx.root.staging_root().exists(),
            "the refusal happened after staging was created"
        );
        fx.assert_no_package_tree("after refusing an unpinned major");
        assert_eq!(fx.ledger_rows().await, 0);
        sanctuary
            .assert_untouched(&fx, "after refusing an unpinned major")
            .await;
    }

    // ------------------------------------------------------------------
    // Group 4 — the live proof that the loop closes.
    //
    // Ignored by default because it needs the real build artifact, which is
    // gitignored build output rather than a committed fixture. It is checked in
    // rather than run by hand so the proof is repeatable:
    //
    //   OPENVHOST_PHP_TARBALL=$PWD/build/out/php-8.4.24-macos-arm64.tar.gz \
    //     cargo test -p openvhost-core --lib -- --ignored --nocapture \
    //     php::package::install::tests::the_real_artifact_installs_and_runs_from_the_package_tree
    //
    // Module-qualified because a cargo test filter is a SUBSTRING match and
    // MariaDB and nginx carry twins of this test under the identical name: the
    // bare name selects all three, and the other two panic on their own unset
    // tarball variable, so the command exits 101 with this test itself passing.
    //
    // Vacuity: it asserts the version string out of the binary's own output,
    // AND (unlike nginx's twin, which cannot) that `php-fpm --version` exits
    // 0 — see `PHP_WARMUP_BINARY`'s doc for why php-fpm's flag parser accepts
    // the same argument the warm-up step already sent it. Proven by mutation:
    // expecting "8.4.23" failed it against the real 8.4.24 output.
    // ------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "needs the real build artifact; set OPENVHOST_PHP_TARBALL"]
    async fn the_real_artifact_installs_and_runs_from_the_package_tree() {
        let path = std::env::var("OPENVHOST_PHP_TARBALL")
            .expect("set OPENVHOST_PHP_TARBALL to build/out/php-8.4.24-macos-arm64.tar.gz");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let sha = sha_hex(&bytes);
        assert_eq!(
            sha,
            crate::php::PHP_PACKAGES[0].sha256,
            "the artifact at {path} is not the one the catalogue pins"
        );

        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        // A LOCAL source, deliberately: the pinned release does not exist yet,
        // so proving the loop closes must not depend on it. The (url, sha256)
        // pair is otherwise handled exactly as production would.
        let entry = entry_for(serve_once(bytes), sha);

        let out = install_entry(&entry, &fx.root, Some(&fx.ledger), |_| {})
            .await
            .unwrap();

        assert_eq!(out.package.dir, fx.version_dir());
        let php_fpm_bin = fx.version_dir().join("bin/php-fpm");
        assert!(
            php_fpm_bin.is_file(),
            "{} is missing",
            php_fpm_bin.display()
        );

        let output = std::process::Command::new(&php_fpm_bin)
            .env_clear()
            .arg("--version")
            .output()
            .unwrap_or_else(|e| panic!("exec {}: {e}", php_fpm_bin.display()));
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!(
            "ran {}\n  status: {}\n  output: {}",
            php_fpm_bin.display(),
            output.status,
            combined.trim()
        );
        assert!(
            output.status.success(),
            "php-fpm --version exited {}: {combined}",
            output.status,
        );
        assert!(
            combined.contains("8.4.24"),
            "php-fpm --version printed {combined:?}, which is not the version we installed"
        );

        assert_eq!(fx.ledger_rows().await, 1);
        sanctuary
            .assert_untouched(&fx, "after installing the real artifact")
            .await;
    }
}
