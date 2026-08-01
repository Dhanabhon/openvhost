// SPDX-License-Identifier: GPL-3.0-or-later
//! Pipeline orchestration: sweep → stage → download+verify → extract →
//! atomic install → current link. Single in-process install at a time (S25).

use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openvhost_proc::{SpawnSpec, TaskEvent, default_driver, run_task};
use tokio::sync::Semaphore;

use crate::download::download_and_verify;
use crate::error::PkgError;
use crate::extract;
use crate::layout::{self, Staging};
use crate::request::{
    ArchiveFormat, InstallRequest, InstalledPackage, PackagesRoot, Progress, validate_component,
};

/// The one and only argument the warm-up exec is ever given — a fixed
/// `&'static str`, never anything derived from a manifest, a request, or a
/// user. The exit status is irrelevant: macOS validates a binary's signature
/// at exec/mmap time, before `main` runs, so a program that rejects
/// `--version` and exits non-zero has still been warmed. The flag exists
/// only to make a program that DOES understand it exit immediately instead
/// of starting up for real.
const WARMUP_ARG: &str = "--version";

/// Wall-clock ceiling on the whole warm-up (spawn, exec, wait). Measured
/// worst case is ~1.9 s — a one-time ~1.1 s network notarization lookup plus
/// ~750 ms of signature validation — so this is >15x headroom for a slow
/// notarization round trip, while still guaranteeing a hung probe cannot
/// hold an install open. On expiry the child's whole process GROUP is
/// killed and the install carries on.
const WARMUP_BUDGET: Duration = Duration::from_secs(30);

/// Outcome of a warm-up attempt. Every variant is a non-event for the
/// install — this type exists so the decision is explicit and testable
/// rather than buried in a log line, and so a future variant cannot be
/// silently ignored by the exhaustive match in [`warm_up`].
#[derive(Debug, PartialEq, Eq)]
enum Warmup {
    /// A precondition failed; nothing was executed.
    Skipped(&'static str),
    /// The program ran to completion. `code` is `None` if a signal ended it.
    Ran { code: Option<i32> },
    /// Killed for outliving [`WARMUP_BUDGET`].
    TimedOut,
    /// The program could not be started at all (missing, not executable,
    /// wrong architecture, …).
    SpawnFailed,
}

/// Exec `rel` inside the staged tree once, discarding its output, so macOS
/// pays the Gatekeeper/XProtect first-execution signature check HERE —
/// behind the install progress the user is already watching — instead of on
/// their first "Start". Measured: 1.877 s cold vs 13.7 ms warm on a fresh
/// 67 MB `mysqld`, and because the validation is keyed to the inode rather
/// than the path it survives the atomic rename (749 ms paid in staging →
/// 13.6 ms on the first exec after the rename).
///
/// **Never fails the install.** Every failure mode — absent, unreadable,
/// resolving outside the tree, unspawnable, non-zero exit, hang — returns a
/// [`Warmup`] variant and is logged. That is deliberate: this is an
/// optimization, and a package whose `--version` misbehaves is still
/// correctly installed.
///
/// Containment, in order: `rel` was charset/shape-validated at request
/// ingress; it is joined onto the staged root and then CANONICALIZED, so a
/// symlink inside the archive cannot redirect the exec outside the tree we
/// just extracted; argv is one fixed literal; the child gets its own process
/// group and an allow-list environment (via `openvhost-proc`'s driver — the
/// single spawn path in this codebase); its cwd is the staging directory,
/// which is `0o700` and deleted wholesale when staging drops, so anything it
/// writes relative to cwd cannot land in the installed package or the user's
/// working directory; and the whole thing is bounded by `budget`, after
/// which the process GROUP (not just the direct child) is killed.
///
/// The canonicalize-then-contain check is a defence in depth, not the
/// primary guarantee: the tree being exec'd came from a SHA-256-verified
/// archive through the hardened extractor, and the staging directory is
/// private to this process for its whole lifetime.
async fn warm_up(staged_root: &Path, rel: &Path, cwd: &Path, budget: Duration) -> Warmup {
    let started = Instant::now();
    let outcome = warm_up_inner(staged_root, rel, cwd, budget).await;
    let took_ms = started.elapsed().as_millis();
    // Exhaustive by design: a new `Warmup` variant must not be able to slip
    // through an unlogged wildcard arm.
    match &outcome {
        Warmup::Skipped(why) => {
            tracing::debug!(rel = %rel.display(), why, "warm-up skipped")
        }
        Warmup::Ran { code } => {
            tracing::info!(rel = %rel.display(), ?code, took_ms, "warmed up binary")
        }
        Warmup::TimedOut => {
            tracing::warn!(rel = %rel.display(), took_ms, "warm-up exceeded its budget; killed")
        }
        Warmup::SpawnFailed => {
            tracing::warn!(rel = %rel.display(), took_ms, "warm-up could not be started")
        }
    }
    outcome
}

async fn warm_up_inner(staged_root: &Path, rel: &Path, cwd: &Path, budget: Duration) -> Warmup {
    let (Ok(program), Ok(root)) = (
        staged_root.join(rel).canonicalize(),
        staged_root.canonicalize(),
    ) else {
        return Warmup::Skipped("warm-up target does not resolve");
    };
    if !program.starts_with(&root) {
        return Warmup::Skipped("warm-up target resolves outside the staged tree");
    }
    if !program.is_file() {
        return Warmup::Skipped("warm-up target is not a regular file");
    }

    let spec = SpawnSpec {
        program,
        args: vec![OsString::from(WARMUP_ARG)],
        cwd: Some(cwd.to_path_buf()),
        env: Vec::new(),
    };

    // The output is thrown away, but it must still be READ: the driver pipes
    // stdout/stderr, and a chatty program whose pipe nobody drains blocks on
    // write and would burn the whole budget for no reason.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<TaskEvent>(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let outcome = match tokio::time::timeout(budget, run_task(default_driver(), spec, tx)).await {
        // Dropping `run_task`'s future is its documented cancellation, and
        // its `KillOnDrop` guard SIGKILLs the child's whole process group.
        Err(_elapsed) => Warmup::TimedOut,
        Ok(Err(_e)) => Warmup::SpawnFailed,
        Ok(Ok(code)) => Warmup::Ran { code },
    };
    // The child is gone either way by now (exited, or group-killed above), so
    // the detached pump tasks inside `run_task` are at EOF and this drain has
    // nothing left to receive. Abort rather than leave a task whose lifetime
    // depends on that reasoning staying true.
    drain.abort();
    outcome
}

/// Process-wide install gate (S25): a single permit, so at most one
/// [`install_package`] call is ever mid-flight in this process at a time —
/// concurrent callers queue here rather than racing the shared staging root
/// and sweeper. Nothing in this crate ever calls `close()` on it, so
/// `acquire` never actually fails in practice; the call site still maps a
/// hypothetical closed-gate error onto [`PkgError::Internal`] instead of
/// `expect()`, per repo rule (no `unwrap()`/`expect()` outside
/// `#[cfg(test)]` — also enforced here by the workspace's
/// `clippy::unwrap_used`/`expect_used` lints under `-D warnings`).
fn install_gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(1))
}

/// Install `req` under `root`.
///
/// Pipeline: pre-check the destination version directory doesn't already
/// exist, sweep abandoned staging directories older than 24h (S20), stage a
/// fresh private working directory, download the archive and verify its
/// SHA-256 (S8 — the SAME open, verified `File` handle returned by the
/// download stage is threaded straight into extraction on the blocking
/// pool, and handed back out again, never re-opened by path), extract
/// through the hardened per-format walk, optionally warm the binary named by
/// [`InstallRequest::with_warmup_binary`] while the tree is still in staging
/// (S26 — an optimization that can never fail the install), atomically
/// rename the extracted tree into its final
/// `packages/<name>/<major>/<version>/` location (S21), then swing the
/// per-major `current` link onto it (S22).
///
/// `progress` receives [`Progress`] events as the pipeline advances
/// (`Started`/`Downloaded` during the fetch, then `Verified`, `Extracted`,
/// `Linked`).
///
/// At most one install runs at a time process-wide (S25); concurrent
/// callers queue on an internal semaphore rather than racing the
/// filesystem. Returns [`PkgError::AlreadyInstalled`] immediately — before
/// any network or filesystem staging work happens for this call — if the
/// destination version directory already exists.
pub async fn install_package(
    req: &InstallRequest,
    root: &PackagesRoot,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<InstalledPackage, PkgError> {
    // Re-validate the path components even though `InstallRequest::new`
    // already checked them (security audit A2): this defends the trust
    // boundary at the point it actually matters — right before
    // `req.name`/`req.major`/`req.version` get used to build filesystem
    // paths — so it holds even if a caller ever obtained an `InstallRequest`
    // by some means other than `::new`. Cheap; fails closed on any bad
    // component before any network or filesystem work happens.
    validate_component(&req.name)?;
    validate_component(&req.major)?;
    validate_component(&req.version)?;

    let _permit = install_gate()
        .acquire()
        .await
        .map_err(|_| PkgError::Internal("install semaphore closed unexpectedly".to_string()))?;

    let final_dir = root.package_dir(&req.name, &req.major, &req.version);
    if final_dir.exists() {
        return Err(PkgError::AlreadyInstalled {
            name: req.name.clone(),
            version: req.version.clone(),
        });
    }
    layout::sweep_stale(root);

    let staging = Staging::create(root)?;
    let staging_path = staging.path().to_path_buf();
    let extract_root = staging_path.join("root");
    std::fs::create_dir_all(&extract_root)
        .map_err(|e| PkgError::io("create_dir", &extract_root, e))?;

    // Download + verify onto the same handle we extract from (S8).
    let mut file = download_and_verify(&req.url, &req.sha256, &staging_path, &mut progress).await?;

    // Extraction is blocking CPU work; run it on the blocking pool and hand
    // the SAME file handle back out — nothing is ever re-opened by path.
    let fmt = req.format;
    let er = extract_root.clone();
    let file = tokio::task::spawn_blocking(move || -> Result<std::fs::File, PkgError> {
        match fmt {
            ArchiveFormat::TarGz => extract::targz::extract_targz(&mut file, &er)?,
            ArchiveFormat::Zip => extract::zip::extract_zip(&mut file, &er)?,
        }
        Ok(file)
    })
    .await
    .map_err(|e| PkgError::Internal(format!("extract task panicked: {e}")))??;
    drop(file);
    progress(Progress::Extracted);

    // Pre-pay macOS's first-execution signature check while the tree is
    // still in staging (S26). This deliberately sits BEFORE the rename: the
    // check is keyed to the inode, so paying it here covers the first exec
    // after the rename, and the user's first "Start" is warm. Cannot fail
    // the install — see [`warm_up`].
    if let Some(rel) = req.warmup_bin.as_deref() {
        warm_up(&extract_root, rel, &staging_path, WARMUP_BUDGET).await;
    }

    layout::install_dir(&extract_root, &final_dir, &req.name, &req.version)?;

    let link = root.current_link(&req.name, &req.major);
    layout::update_current(&link, &req.version)?;
    progress(Progress::Linked);

    tracing::info!(
        name = %req.name,
        version = %req.version,
        dest = %final_dir.display(),
        "install complete"
    );

    Ok(InstalledPackage {
        dir: final_dir,
        current_link: link,
        name: req.name.clone(),
        major: req.major.clone(),
        version: req.version.clone(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;

    // ------------------------------------------------------------------
    // Fixtures. Deliberately local rather than shared with `testkit`: these
    // need per-test absolute paths baked into a shell script, which the
    // `&'static str` archive builders there cannot express.
    // ------------------------------------------------------------------

    /// A 0755 `#!/bin/sh` script. Absolute program paths only, because the
    /// driver hands the child an ALLOW-LIST environment, not the ambient one.
    fn script(body: &str) -> String {
        format!("#!/bin/sh\n{body}\n")
    }

    /// tar.gz with a 0755 `bin/probe` and a plain `README`. Two distinct
    /// top-level components on purpose: no single shared root, so the
    /// single-root strip rule is not part of what these tests depend on.
    fn targz_with_probe(probe: &str) -> Vec<u8> {
        use flate2::{Compression, write::GzEncoder};
        let gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut ar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(probe.len() as u64);
        h.set_mode(0o755);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        ar.append_data(&mut h, "bin/probe", probe.as_bytes())
            .unwrap();
        let mut r = tar::Header::new_gnu();
        r.set_size(2);
        r.set_mode(0o644);
        r.set_entry_type(tar::EntryType::Regular);
        r.set_cksum();
        ar.append_data(&mut r, "README", &b"hi"[..]).unwrap();
        ar.into_inner().unwrap().finish().unwrap()
    }

    fn sha_hex(b: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b))
    }

    fn serve_once(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b);
                let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = s.write_all(hdr.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}/pkg.tar.gz")
    }

    struct Fixture {
        _home: tempfile::TempDir,
        root: PackagesRoot,
        /// Scratch directory OUTSIDE the package tree that probe scripts
        /// write their evidence into.
        evidence: PathBuf,
    }

    fn fixture() -> Fixture {
        let home = tempfile::Builder::new()
            .prefix("ovh-warmup")
            .tempdir_in("/tmp")
            .unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(root.as_path()).unwrap();
        let evidence = home.path().join("evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        Fixture {
            _home: home,
            root,
            evidence,
        }
    }

    async fn install(
        fx: &Fixture,
        probe: &str,
        version: &str,
        warmup: Option<&str>,
    ) -> Result<InstalledPackage, PkgError> {
        let archive = targz_with_probe(probe);
        let sha = sha_hex(&archive);
        let url = serve_once(archive);
        let mut req =
            InstallRequest::new("mysql", "8.4", version, &url, &sha, ArchiveFormat::TarGz).unwrap();
        if let Some(rel) = warmup {
            req = req.with_warmup_binary(rel).unwrap();
        }
        install_package(&req, &fx.root, |_| {}).await
    }

    // ------------------------------------------------------------------
    // Group 1 — the warm-up actually runs, through the real pipeline.
    // ------------------------------------------------------------------

    /// A test that only asserted "the install still succeeded" would pass
    /// against a warm-up that does nothing at all. This asserts on a side
    /// effect only the child process can produce, AND on the argv it was
    /// given — so it also pins that the exec is `--version` and nothing else.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_named_binary_is_actually_executed_before_the_rename() {
        let fx = fixture();
        let marker = fx.evidence.join("argv");
        let probe = script(&format!(
            "/usr/bin/printf '%s' \"$*\" > {}",
            marker.display()
        ));

        let installed = install(&fx, &probe, "8.4.1", Some("bin/probe"))
            .await
            .unwrap();

        assert!(
            marker.is_file(),
            "the warm-up binary was never executed: no marker at {}",
            marker.display()
        );
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "--version",
            "the warm-up must pass exactly one fixed argument"
        );
        assert!(installed.dir.join("bin/probe").is_file());
    }

    /// The warm-up must happen in STAGING, before the atomic rename — that
    /// is the whole reason it works (the signature check is keyed to the
    /// inode and survives `rename(2)`). The probe records its own `$0`,
    /// which is therefore a staging path, not the final package path.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_warm_up_runs_in_staging_not_in_the_installed_location() {
        let fx = fixture();
        let marker = fx.evidence.join("argv0");
        let probe = script(&format!(
            "/usr/bin/printf '%s' \"$0\" > {}",
            marker.display()
        ));

        let installed = install(&fx, &probe, "8.4.2", Some("bin/probe"))
            .await
            .unwrap();

        let seen = std::fs::read_to_string(&marker).unwrap();
        let staging = fx.root.staging_root().canonicalize().unwrap();
        assert!(
            seen.starts_with(staging.to_str().unwrap()),
            "warm-up ran at {seen}, which is not under staging {}",
            staging.display()
        );
        assert!(
            !seen.starts_with(installed.dir.to_str().unwrap()),
            "warm-up ran at the FINAL location {seen} — it must precede the rename"
        );
    }

    /// A request that names no warm-up binary must execute nothing at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_install_that_names_no_warm_up_binary_executes_nothing() {
        let fx = fixture();
        let marker = fx.evidence.join("should-not-exist");
        let probe = script(&format!("/usr/bin/touch {}", marker.display()));

        install(&fx, &probe, "8.4.3", None).await.unwrap();

        assert!(
            !marker.exists(),
            "an install with no warm-up binary must not exec anything"
        );
    }

    // ------------------------------------------------------------------
    // Group 2 — a warm-up failure is a non-event for the install.
    // ------------------------------------------------------------------

    /// The binding constraint: a package whose probe exits non-zero is still
    /// correctly installed. The marker proves the failing program really ran
    /// (otherwise this would pass against a warm-up that was never wired up).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_warm_up_that_exits_non_zero_does_not_fail_the_install() {
        let fx = fixture();
        let marker = fx.evidence.join("ran-then-failed");
        let probe = script(&format!("/usr/bin/touch {}\nexit 7", marker.display()));

        let installed = install(&fx, &probe, "8.4.4", Some("bin/probe"))
            .await
            .expect("a non-zero warm-up must not fail the install");

        assert!(marker.is_file(), "the failing probe must actually have run");
        assert!(installed.dir.join("README").is_file());
        assert_eq!(
            std::fs::read_link(&installed.current_link).unwrap(),
            PathBuf::from("8.4.4")
        );
    }

    /// Same guarantee for a warm-up that cannot run at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_warm_up_naming_a_missing_binary_does_not_fail_the_install() {
        let fx = fixture();
        let installed = install(&fx, &script("exit 0"), "8.4.5", Some("bin/not-here"))
            .await
            .expect("a missing warm-up target must not fail the install");
        assert!(installed.dir.join("README").is_file());
    }

    /// …and for one that is present but not executable.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_warm_up_naming_a_non_executable_file_does_not_fail_the_install() {
        let fx = fixture();
        let installed = install(&fx, &script("exit 0"), "8.4.6", Some("README"))
            .await
            .expect("a non-executable warm-up target must not fail the install");
        assert!(installed.dir.join("bin/probe").is_file());
    }

    // ------------------------------------------------------------------
    // Group 3 — the helper's own contract, exercised directly.
    // ------------------------------------------------------------------

    /// Lay out a staged tree containing one 0755 script at `bin/probe`.
    #[cfg(unix)]
    fn staged_tree(body: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .prefix("ovh-warmup-unit")
            .tempdir_in("/tmp")
            .unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let p = root.join("bin/probe");
        std::fs::write(&p, script(body)).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, root)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn warm_up_reports_the_exit_code_of_a_program_that_runs() {
        let (dir, root) = staged_tree("exit 3");
        let out = warm_up(
            &root,
            Path::new("bin/probe"),
            dir.path(),
            Duration::from_secs(20),
        )
        .await;
        assert_eq!(out, Warmup::Ran { code: Some(3) });
    }

    /// A probe that hangs must be killed, not waited out — and the kill must
    /// reach the whole process group, which is why the script's own child
    /// (`sleep`) is what writes the post-sleep marker.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hanging_warm_up_is_killed_at_its_budget_and_never_finishes() {
        let (dir, root) = staged_tree("/bin/sleep 1 && /usr/bin/touch FINISHED");
        let cwd = dir.path();

        let t0 = Instant::now();
        let out = warm_up(
            &root,
            Path::new("bin/probe"),
            cwd,
            Duration::from_millis(150),
        )
        .await;
        let elapsed = t0.elapsed();

        assert_eq!(out, Warmup::TimedOut);
        assert!(
            elapsed < Duration::from_millis(900),
            "warm_up waited {elapsed:?} — it must return at its budget, not wait the child out"
        );
        // Outlast the child's own sleep: if the group kill had not landed,
        // FINISHED would appear here.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert!(
            !cwd.join("FINISHED").exists(),
            "the timed-out warm-up survived its budget and completed"
        );
    }

    /// cwd containment: whatever the probe writes relative to its cwd lands
    /// in staging (which is deleted wholesale), never in the package tree.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_warm_up_child_runs_with_staging_as_its_working_directory() {
        let (dir, root) = staged_tree("/usr/bin/touch ./STRAY");
        let out = warm_up(
            &root,
            Path::new("bin/probe"),
            dir.path(),
            Duration::from_secs(20),
        )
        .await;
        assert_eq!(out, Warmup::Ran { code: Some(0) });
        assert!(dir.path().join("STRAY").is_file(), "cwd was not staging");
        assert!(
            !root.join("STRAY").exists(),
            "a stray relative write must not land in the package tree"
        );
    }

    /// Containment: a symlink inside the tree that resolves OUTSIDE it must
    /// not be executed, even though its relative path is perfectly innocent.
    /// This is the check that has to hold no matter what the extractor's
    /// symlink rule decides to admit.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_warm_up_target_that_resolves_outside_the_tree_is_refused() {
        let (dir, root) = staged_tree("exit 0");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let escape = outside.join("evil");
        std::fs::write(&escape, script("/usr/bin/touch OWNED")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&escape, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::os::unix::fs::symlink(&escape, root.join("bin/escape")).unwrap();

        let out = warm_up(
            &root,
            Path::new("bin/escape"),
            dir.path(),
            Duration::from_secs(20),
        )
        .await;

        assert_eq!(
            out,
            Warmup::Skipped("warm-up target resolves outside the staged tree")
        );
        assert!(
            !dir.path().join("OWNED").exists(),
            "the out-of-tree program was executed"
        );
    }

    /// Non-vacuity twin of the test above: the SAME symlink shape, pointing
    /// back INSIDE the tree, is executed. Without this, a containment check
    /// that refused every symlink — or every warm-up — would look correct.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_that_stays_inside_the_tree_is_still_warmed() {
        let (dir, root) = staged_tree("/usr/bin/touch INSIDE");
        std::os::unix::fs::symlink("probe", root.join("bin/alias")).unwrap();

        let out = warm_up(
            &root,
            Path::new("bin/alias"),
            dir.path(),
            Duration::from_secs(20),
        )
        .await;

        assert_eq!(out, Warmup::Ran { code: Some(0) });
        assert!(dir.path().join("INSIDE").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn warm_up_skips_a_target_that_is_not_a_regular_file() {
        let (dir, root) = staged_tree("exit 0");
        let out = warm_up(&root, Path::new("bin"), dir.path(), Duration::from_secs(20)).await;
        assert_eq!(out, Warmup::Skipped("warm-up target is not a regular file"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn warm_up_reports_a_spawn_failure_for_a_non_executable_target() {
        let (dir, root) = staged_tree("exit 0");
        std::fs::write(root.join("bin/data"), b"not a program").unwrap();
        let out = warm_up(
            &root,
            Path::new("bin/data"),
            dir.path(),
            Duration::from_secs(20),
        )
        .await;
        assert_eq!(out, Warmup::SpawnFailed);
    }
}
