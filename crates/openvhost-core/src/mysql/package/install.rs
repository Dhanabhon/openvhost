// SPDX-License-Identifier: GPL-3.0-or-later
//! Installing a catalogued MySQL build into OpenVHost's own package tree —
//! the first production consumer of `openvhost-pkg` (MySQL-from-tarball design
//! D1, D2, D4).
//!
//! This module is wiring, and deliberately thin: it resolves a pinned
//! catalogue entry, hands it to the existing download → SHA-256 verify →
//! extract → atomic-install pipeline, and records the version it asked for.
//! It adds no install machinery of its own — no second downloader, no second
//! extractor, no second notion of where a package lives.
//!
//! SECURITY: the public entry point takes a [`MysqlMajor`] and nothing else.
//! The URL and hash reaching the downloader come only from
//! [`crate::mysql::MYSQL_PACKAGES`], so there is no argument any caller — IPC,
//! CLI or otherwise — can supply that changes which bytes are fetched or what
//! they must hash to. Verification happens before extraction, on the same file
//! handle, and the extracted tree is renamed into place atomically, so a
//! failure at any stage leaves the package tree exactly as it was.
//!
//! What this module never does, on any path including error paths: write to
//! `<home>/data/` (the datadir is shared across install sources by design D6 —
//! a user who initialized 8.4 under Homebrew keeps their databases), write to
//! `<home>/logs/`, or touch a stored root credential. It writes under
//! `<home>/packages/` and appends one row to `state.db`'s ledger.

use openvhost_pkg::{InstallRequest, InstalledPackage, PackagesRoot, Progress};

use crate::error::CoreError;
use crate::mysql::MysqlMajor;
use crate::mysql::package::catalogue::{
    MYSQL_PACKAGE_NAME, MYSQL_WARMUP_BINARY, MysqlPackage, mysql_package_for_host,
};
use crate::mysql::package::ledger::InstallLedger;

/// Whether the install was also written to the ledger.
///
/// Modelled as a value rather than buried in a log line for the same reason
/// `openvhost-pkg`'s own warm-up outcome is: the decision is then testable,
/// and a future variant cannot be silently ignored by a wildcard arm.
///
/// The package is installed either way. The tree is the inventory (see
/// [`crate::mysql::InstallLedger`]), so a ledger write that fails costs
/// provenance, never correctness — and reporting the install as failed when
/// the binaries are demonstrably on disk would be a worse lie than the missing
/// row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerWrite {
    /// The row was written; `installed_at` is milliseconds since the epoch.
    Recorded { installed_at: i64 },
    /// The row could not be written. Already logged at error level.
    Failed { reason: String },
}

/// The result of installing one catalogued MySQL build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlPackageInstall {
    /// Where it landed, and the `current` link that now points at it.
    pub package: InstalledPackage,
    /// Whether the install ledger recorded it.
    pub ledger: LedgerWrite,
}

/// Install the catalogued MySQL build for `major` on this host.
///
/// Resolves the pinned entry, streams it to a private staging directory,
/// verifies its SHA-256 **before** anything parses the bytes, extracts through
/// the hardened walk, pre-pays macOS's first-execution signature check on
/// `bin/mysqld` while the tree is still in staging, renames the tree
/// atomically into `packages/mysql/<major>/<version>/`, swings the per-major
/// `current` link onto it, and records the exact version in `state.db`.
///
/// `progress` receives [`Progress`] events as the pipeline advances. The
/// events a user should be able to distinguish — a download that was verified
/// from one that merely arrived — are distinct variants, not log text.
///
/// **Cancellation.** There is no wall-clock ceiling on the transfer, only a
/// 30-second idle window and a 1 GiB size cap, so a server that dribbles bytes
/// slowly enough could otherwise hold this call open indefinitely. Dropping
/// the returned future is the cancel: the staging directory is an RAII
/// temporary and is removed as the future unwinds, and the process-wide
/// install permit is released with it. Callers exposing this to a user should
/// hold an abort handle (or a `tokio::select!` on a cancel signal) and offer
/// it — see the test
/// `cancelling_a_stalled_download_leaves_no_partial_tree_and_touches_nothing`.
/// The permit matters: it is process-wide and taken *before* staging, so an
/// install nobody can cancel blocks every later install too.
///
/// Returns [`CoreError::NoPackageForTarget`] before any network or filesystem
/// work if this build publishes no verified artifact for `major` on this host.
pub async fn install_mysql_package(
    major: &MysqlMajor,
    root: &PackagesRoot,
    ledger: &InstallLedger,
    progress: impl FnMut(Progress) + Send,
) -> Result<MysqlPackageInstall, CoreError> {
    let entry = mysql_package_for_host(major)?;
    install_entry(entry, root, ledger, progress).await
}

/// The pipeline itself, split out from the catalogue lookup so tests can drive
/// it against a loopback fixture.
///
/// Private on purpose: taking a [`MysqlPackage`] means taking a URL and a
/// hash, and the whole point of the public signature above is that no caller
/// can choose those. Nothing outside this module may name this function.
async fn install_entry(
    entry: &MysqlPackage,
    root: &PackagesRoot,
    ledger: &InstallLedger,
    progress: impl FnMut(Progress) + Send,
) -> Result<MysqlPackageInstall, CoreError> {
    // Every component below is a compiled-in `&'static str` from the
    // catalogue, not a value derived from `major` — so the path this install
    // writes to is fixed at compile time even though the lookup key was not.
    let request = InstallRequest::new(
        MYSQL_PACKAGE_NAME,
        entry.major,
        entry.version,
        entry.url,
        entry.sha256,
        entry.format,
    )?
    .with_warmup_binary(MYSQL_WARMUP_BINARY)?;

    let package = openvhost_pkg::install_package(&request, root, progress).await?;

    // Design D4: we asked for this version, so we know it. Recorded only after
    // the tree is on disk — a failed install must leave no phantom row.
    let ledger_write = match ledger
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
                "MySQL is installed but its ledger row could not be written"
            );
            LedgerWrite::Failed {
                reason: e.to_string(),
            }
        }
    };

    tracing::info!(
        version = %package.version,
        dir = %package.dir.display(),
        "installed MySQL from the upstream tarball"
    );

    Ok(MysqlPackageInstall {
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

    use crate::db::Db;
    use crate::mysql::package::catalogue::PackageTarget;
    use crate::mysql::{MysqlInstanceRepo, generate_root_password};

    // ------------------------------------------------------------------
    // Fixtures.
    // ------------------------------------------------------------------

    /// A `.tar.gz` shaped like Oracle's: one implicit top-level directory that
    /// the extractor strips, `bin/mysqld` and `bin/mysqld_safe` both present
    /// and both executable.
    fn mysql_shaped_targz(mysqld: &str, mysqld_safe: &str) -> Vec<u8> {
        use flate2::{Compression, write::GzEncoder};
        let gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut ar = tar::Builder::new(gz);
        let entries: [(&str, &str, u32); 3] = [
            ("mysql-8.4.11-macos15-arm64/bin/mysqld", mysqld, 0o755),
            (
                "mysql-8.4.11-macos15-arm64/bin/mysqld_safe",
                mysqld_safe,
                0o755,
            ),
            ("mysql-8.4.11-macos15-arm64/LICENSE", "GPL-2.0\n", 0o644),
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
    /// need no TLS; the carve-out is compiled out of release entirely.
    fn serve_once(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = s.write_all(hdr.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/mysql.tar.gz")
    }

    /// Announce a large body, send a few bytes, then go quiet — the shape that
    /// would hold an install open with no wall clock to stop it.
    fn serve_stalling() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000000\r\n\r\n");
                let _ = s.write_all(&[0u8; 64]);
                let _ = s.flush();
                std::thread::sleep(Duration::from_secs(10));
            }
        });
        format!("http://127.0.0.1:{port}/mysql.tar.gz")
    }

    /// Build a catalogue-entry-shaped value pointing at a loopback fixture.
    /// Test-only: production entries are `&'static str` literals compiled into
    /// the binary, which is exactly why `install_entry` is private.
    fn entry_for(url: String, sha256: String) -> MysqlPackage {
        MysqlPackage {
            major: "8.4",
            version: "8.4.11",
            target: PackageTarget::MacosArm64,
            url: Box::leak(url.into_boxed_str()),
            sha256: Box::leak(sha256.into_boxed_str()),
            format: ArchiveFormat::TarGz,
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
        /// A home with a plausible datadir, a log file and a stored root
        /// credential already in place — the three things no install path may
        /// touch.
        async fn new() -> Fixture {
            let home = tempfile::Builder::new()
                .prefix("ovh-mysql-pkg")
                .tempdir_in("/tmp")
                .unwrap();
            let h = home.path().to_path_buf();
            let root = PackagesRoot::from_home(&h);
            std::fs::create_dir_all(root.as_path()).unwrap();

            std::fs::create_dir_all(h.join("data/mysql/8.4")).unwrap();
            std::fs::write(h.join("data/mysql/8.4/ibdata1"), b"PRECIOUS USER DATA").unwrap();
            std::fs::create_dir_all(h.join("logs")).unwrap();
            std::fs::write(h.join("logs/mysql-8.4.err"), b"an existing error log").unwrap();
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
            self.root.package_dir("mysql", "8.4", "8.4.11")
        }

        fn current_link(&self) -> PathBuf {
            self.root.current_link("mysql", "8.4")
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
            self.ledger.list("mysql").await.unwrap().len()
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
                fx.home.join("data/mysql"),
                fx.home.join("data/mysql/8.4"),
                fx.home.join("data/mysql/8.4/ibdata1"),
                fx.home.join("logs"),
                fx.home.join("logs/mysql-8.4.err"),
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
    // Group 1 — a successful install lands where the catalogue says, warms
    // the right binary, and records the version.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_successful_install_lands_at_the_catalogue_version_and_links_current() {
        let fx = Fixture::new().await;
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        let out = install_entry(&entry, &fx.root, &fx.ledger, |_| {})
            .await
            .unwrap();

        assert_eq!(out.package.dir, fx.version_dir());
        assert_eq!(out.package.name, "mysql");
        assert_eq!(out.package.major, "8.4");
        assert_eq!(out.package.version, "8.4.11");
        assert!(
            fx.version_dir().join("bin/mysqld").is_file(),
            "the implicit archive root must be stripped so bin/mysqld sits at the top"
        );
        assert_eq!(
            std::fs::read_link(fx.current_link()).unwrap(),
            PathBuf::from("8.4.11"),
            "`current` must point at the version we just installed"
        );
    }

    /// The trap recorded from the proof work: warming `mysqld_safe` would run
    /// a shell wrapper that hardcodes /usr/local/mysql/data and really does
    /// start a server. Both binaries are present and both leave evidence, so
    /// this fails loudly if the wrong one is warmed — and the positive marker
    /// stops it passing against an install that warms nothing at all.
    #[tokio::test]
    async fn the_install_warms_mysqld_and_never_the_mysqld_safe_wrapper() {
        let fx = Fixture::new().await;
        let warmed = fx.evidence.join("mysqld-ran");
        let forbidden = fx.evidence.join("mysqld_safe-ran");
        let archive = mysql_shaped_targz(
            &script(&format!("/usr/bin/touch {}", warmed.display())),
            &script(&format!("/usr/bin/touch {}", forbidden.display())),
        );
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        install_entry(&entry, &fx.root, &fx.ledger, |_| {})
            .await
            .unwrap();

        assert!(
            warmed.is_file(),
            "bin/mysqld was never warmed: the Gatekeeper cost lands on the user's first Start"
        );
        assert!(
            !forbidden.exists(),
            "bin/mysqld_safe was executed — that wrapper starts a real server against \
             a hardcoded /usr/local/mysql/data"
        );
    }

    /// Design D4's whole point, exercised through the real install rather than
    /// against the ledger in isolation.
    #[tokio::test]
    async fn the_install_records_the_exact_version_it_asked_the_catalogue_for() {
        let fx = Fixture::new().await;
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        let out = install_entry(&entry, &fx.root, &fx.ledger, |_| {})
            .await
            .unwrap();

        let installed_at = match out.ledger {
            LedgerWrite::Recorded { installed_at } => installed_at,
            LedgerWrite::Failed { reason } => panic!("ledger write failed: {reason}"),
        };
        let row = fx
            .ledger
            .get("mysql", "8.4", "8.4.11")
            .await
            .unwrap()
            .expect("the install must record the version it fetched");
        assert_eq!(row.version, "8.4.11");
        assert_eq!(row.major, "8.4");
        assert_eq!(row.installed_at, installed_at);
        assert_ne!(
            row.version, row.major,
            "the ledger must hold the exact version, not the series"
        );
    }

    /// Golden rule 6, made observable: the bytes are verified BEFORE anything
    /// unpacks them, and a user watching progress can tell a verified download
    /// from one that merely arrived.
    #[tokio::test]
    async fn progress_reports_verification_before_extraction_and_linking() {
        let fx = Fixture::new().await;
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));
        let (seen, sink) = recorder();

        install_entry(&entry, &fx.root, &fx.ledger, sink)
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
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_hash_mismatch_is_refused_and_leaves_no_partial_tree() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        // The pin says something else entirely — the tampered-payload case.
        let entry = entry_for(url, sha_hex(b"not the bytes we pinned"));

        let err = install_entry(&entry, &fx.root, &fx.ledger, |_| {})
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

    /// Dropping the future is the cancel seam, and this is the evidence it
    /// leaves nothing behind — the only bound on a server that dribbles bytes
    /// slowly enough to stay under the 30-second idle window forever.
    ///
    /// The mid-flight assertion is not decoration. Written without it, this
    /// test passed while proving nothing: `install_package` takes a
    /// process-wide permit BEFORE it stages anything, so under the normal
    /// concurrent test run this future spent its whole window queued behind a
    /// sibling install and cancelled a job that had not started. Waiting for
    /// staging to actually exist is what makes the cleanup assertions mean
    /// something.
    #[tokio::test]
    async fn cancelling_a_stalled_download_leaves_no_partial_tree_and_touches_nothing() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        let entry = entry_for(serve_stalling(), sha_hex(b"never served"));

        {
            let mut fut = Box::pin(install_entry(&entry, &fx.root, &fx.ledger, |_| {}));
            // Interleave polling with observation: the future only makes
            // progress while it is being polled, and it may be queued behind
            // another test's install for an unbounded slice of that time.
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while fx.staging_dirs().is_empty() {
                let polled = tokio::time::timeout(Duration::from_millis(25), &mut fut).await;
                assert!(polled.is_err(), "the stalling server somehow completed");
                assert!(
                    std::time::Instant::now() < deadline,
                    "the install never created a staging directory, so cancelling it \
                     would prove nothing"
                );
            }
            assert_eq!(
                fx.staging_dirs().len(),
                1,
                "expected exactly one staging dir"
            );
            drop(fut);
        }

        assert!(
            fx.staging_dirs().is_empty(),
            "a cancelled install left staging behind: {:?}",
            fx.staging_dirs()
        );
        fx.assert_no_package_tree("after cancelling a stalled download");
        assert_eq!(fx.ledger_rows().await, 0);
        sanctuary
            .assert_untouched(&fx, "after cancelling a stalled download")
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
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let url = serve_once(archive.clone());
        let entry = entry_for(url, sha_hex(&archive));

        install_entry(&entry, &fx.root, &fx.ledger, |_| {})
            .await
            .unwrap();

        assert!(
            fx.version_dir().join("bin/mysqld").is_file(),
            "the install did not actually happen, so nothing below is evidence"
        );
        assert!(fx.staging_dirs().is_empty(), "staging survived a success");
        assert_eq!(fx.ledger_rows().await, 1);
        sanctuary
            .assert_untouched(&fx, "after a successful install")
            .await;
    }

    // ------------------------------------------------------------------
    // Group 3 — the catalogue is the only source of a URL, and a refusal
    // happens before any work.
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn a_major_with_no_pinned_build_is_refused_before_any_staging_happens() {
        let fx = Fixture::new().await;
        let sanctuary = Sanctuary::snapshot(&fx).await;
        // Shape-valid, discoverable, and not something this build publishes.
        let discovered_only = MysqlMajor::from_probe("9.7".to_string()).unwrap();

        let err = install_mysql_package(&discovered_only, &fx.root, &fx.ledger, |_| {})
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
        assert_eq!(fx.ledger_rows().await, 0);
        sanctuary.assert_untouched(&fx, "after a refusal").await;
    }

    #[tokio::test]
    async fn installing_the_same_version_twice_is_refused_rather_than_re_downloaded() {
        let fx = Fixture::new().await;
        let archive = mysql_shaped_targz(&script("exit 0"), &script("exit 0"));
        let sha = sha_hex(&archive);
        let first = entry_for(serve_once(archive.clone()), sha.clone());
        install_entry(&first, &fx.root, &fx.ledger, |_| {})
            .await
            .unwrap();

        // A second server that would serve the same bytes — never contacted,
        // because the destination directory already exists.
        let second = entry_for(serve_once(archive.clone()), sha);
        let (seen, sink) = recorder();
        let err = install_entry(&second, &fx.root, &fx.ledger, sink)
            .await
            .unwrap_err();

        assert!(
            matches!(err, CoreError::Package(PkgError::AlreadyInstalled { .. })),
            "got {err:?}"
        );
        assert!(
            seen.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "the second install started transferring bytes"
        );
        assert_eq!(fx.ledger_rows().await, 1, "the row must not be duplicated");
    }
}
