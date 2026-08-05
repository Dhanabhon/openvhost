// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in, real-binary proof for MariaDB initialization (spec D3/D6:
//! docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md; plan
//! Task 2). Drives `initialize_mariadb` against the REAL 11.4.9 artifact,
//! then starts a real server from the generated `my.cnf` and proves the
//! credential actually took — over BOTH transports.
//!
//! **That second transport is the whole point.** Verifying the password over
//! the unix socket alone shows a clean pass while `root@127.0.0.1` still has
//! an empty one, which is the hole measured on 2026-08-04: with only
//! `root@localhost` altered, `mariadb --protocol=TCP --user=root` with no
//! password connected as `root@127.0.0.1` with full privileges. This test is
//! that hole's regression.
//!
//! Two skip gates, in order:
//! 1. `OPENVHOST_MARIADB_LIVE_TESTS=1` — the opt-in itself, mirroring
//!    `mysql_live.rs`'s `OPENVHOST_MYSQL_LIVE_TESTS` convention. A real
//!    `mariadb-install-db` plus two server starts is far too heavy for every
//!    `cargo test --workspace`.
//! 2. `build/out/mariadb-11.4.9-macos-arm64.tar.gz` must be present. This test
//!    NEVER downloads: publishing the release is owner-gated (spec §10) and a
//!    test must not perform an install as a side effect.
//!
//! Hermetic: everything lives under a fresh `/tmp` tempdir — **`/tmp`, never
//! `$TMPDIR`**, because the 103-byte `sun_path` ceiling has bitten this
//! project twice, most recently at 159 bytes, and macOS's `$TMPDIR` is ~50
//! bytes before anything is joined onto it.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openvhost_core::mariadb::{
    MARIADB_PACKAGE_NAME, MARIADB_SERIES, MariadbDatadirState, MariadbInitCtx, MariadbInitOutcome,
    classify_mariadb_datadir, discover_mariadb, initialize_mariadb, mariadb_runtime_dirs,
};
use openvhost_core::{Db, PackagesRoot};
use openvhost_proc::{SpawnSpec, default_driver};
use std::sync::{Arc, Mutex};

const LIVE_ENV_VAR: &str = "OPENVHOST_MARIADB_LIVE_TESTS";
const ARTIFACT: &str = "build/out/mariadb-11.4.9-macos-arm64.tar.gz";
const VERSION: &str = "11.4.9";

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/crates/openvhost-core`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate must sit two levels under the repo root")
        .to_path_buf()
}

/// Extract the real tarball into `packages/mariadb/11.4/11.4.9/` and swing
/// `current` at it — the layout `discover_mariadb` reads, built through
/// [`PackagesRoot`] rather than by hand so the writer and the reader cannot
/// name different files.
fn install_artifact(home: &Path, tarball: &Path) -> PackagesRoot {
    let root = PackagesRoot::from_home(home);
    let dir = root.package_dir(MARIADB_PACKAGE_NAME, MARIADB_SERIES, VERSION);
    std::fs::create_dir_all(&dir).unwrap();
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&dir)
        .arg("--strip-components=1")
        .status()
        .expect("tar must run");
    assert!(status.success(), "extracting the artifact failed");

    let link = root.current_link(MARIADB_PACKAGE_NAME, MARIADB_SERIES);
    std::os::unix::fs::symlink(VERSION, &link).unwrap();
    root
}

fn arg(key: &str, value: &Path) -> OsString {
    let mut a = OsString::from(key);
    a.push(value.as_os_str());
    a
}

/// Run a client and return `(success, stdout)`. Never given the password on
/// argv — callers pass it through an on-disk 0600 defaults file, exactly as
/// production does.
fn client(program: &Path, args: &[OsString]) -> (bool, String) {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .expect("the client must be spawnable");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[tokio::test]
async fn mariadb_init_end_to_end_against_the_real_artifact() {
    if std::env::var(LIVE_ENV_VAR).as_deref() != Ok("1") {
        eprintln!("skipping: set {LIVE_ENV_VAR}=1 to run");
        return;
    }
    let tarball = repo_root().join(ARTIFACT);
    if !tarball.is_file() {
        eprintln!("skipping: {} is not present", tarball.display());
        return;
    }

    let home = tempfile::Builder::new()
        .prefix("ovh")
        .tempdir_in("/tmp")
        .unwrap();
    let root = install_artifact(home.path(), &tarball);

    let discovered = discover_mariadb(&root);
    assert_eq!(discovered.runtimes.len(), 1, "the artifact must be found");
    let runtime = discovered.runtimes[0].clone();
    // Spec D5: a CONCRETE version directory, never `current`.
    assert!(
        runtime.mariadbd.starts_with(root.package_dir(
            MARIADB_PACKAGE_NAME,
            MARIADB_SERIES,
            VERSION
        )),
        "spawned path must be concrete, got {}",
        runtime.mariadbd.display()
    );

    // ---- Initialize ----
    let ctx = MariadbInitCtx::new(home.path(), runtime.clone());
    assert!(!ctx.paths.datadir.exists());
    let (outcome, password) = initialize_mariadb(&ctx, default_driver()).await;
    assert_eq!(outcome, MariadbInitOutcome::Initialized, "init failed");
    let password = password.expect("an Initialized run must yield the credential");

    match classify_mariadb_datadir(&ctx.paths.datadir).unwrap() {
        MariadbDatadirState::Initialized { version } => assert_eq!(version, VERSION),
        other => panic!("the finalized datadir must classify Initialized, got {other:?}"),
    }
    // No staging litter left behind.
    let leftovers: Vec<_> = std::fs::read_dir(&ctx.paths.staging_parent)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != MARIADB_SERIES)
        .collect();
    assert!(leftovers.is_empty(), "staging litter: {leftovers:?}");

    // The credential round-trips through state.db.
    let db = Db::open(home.path()).await.unwrap();
    let repo = openvhost_core::mariadb::MariadbInstanceRepo::new(&db);
    repo.upsert(&password).await.unwrap();
    assert_eq!(
        repo.get().await.unwrap().unwrap().root_password.expose(),
        password.expose()
    );

    // ---- Start the real server from the GENERATED my.cnf ----
    //
    // argv is exactly `--defaults-file=<my.cnf>` — the shape Task 3's
    // `mariadb_spec` will hand the Supervisor.
    let driver = default_driver();
    let mut server = driver
        .spawn(&SpawnSpec {
            program: runtime.mariadbd.clone(),
            args: vec![arg("--defaults-file=", &ctx.paths.my_cnf)],
            cwd: None,
            env: vec![],
        })
        .expect("the real server must spawn");

    // Drain the server's own output. Without this a startup abort shows up
    // only as "never bound its socket", and a server that outlives the pipe
    // buffer blocks — the same two reasons `initialize_mariadb` drains its
    // temp server.
    let server_log = Arc::new(Mutex::new(String::new()));
    for stream in [server.take_stdout(), server.take_stderr()] {
        let Some(stream) = stream else { continue };
        let sink = Arc::clone(&server_log);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut b) = sink.lock() {
                    b.push_str(&line);
                    b.push('\n');
                }
            }
        });
    }
    let log = || server_log.lock().map(|b| b.clone()).unwrap_or_default();

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    // A passwordless ping is REFUSED once the credential is set, so readiness
    // is "the socket answers at all", not "the ping succeeded".
    while !ctx.paths.socket.exists() && std::time::Instant::now() < deadline {
        assert!(
            matches!(server.try_wait(), Ok(None)),
            "the server exited before binding its socket:\n{}",
            log()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        ctx.paths.socket.exists(),
        "the server never bound its socket:\n{}",
        log()
    );

    // MEASURED, and it matters for Task 3's readiness probe: `mariadb-admin
    // ping` exits 0 even when the connection is REFUSED for access — a
    // refused connection still proves a server is answering. So liveness is
    // asserted with a real query, never with a ping.
    let (open, out) = client(
        &runtime.mariadb,
        &[
            OsString::from("--no-defaults"),
            OsString::from("--protocol=SOCKET"),
            arg("--socket=", &ctx.paths.socket),
            OsString::from("--user=root"),
            OsString::from("-N"),
            OsString::from("-B"),
            OsString::from("-e"),
            OsString::from("SELECT CURRENT_USER()"),
        ],
    );
    assert!(
        !open,
        "a passwordless root query over the socket must be REFUSED after init, got {out:?}"
    );

    // A 0600 defaults file — the password reaches a client only this way or
    // on stdin, never on argv and never through the environment.
    let defaults = home.path().join("df.cnf");
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&defaults)
            .unwrap();
        write!(f, "[client]\nuser=root\npassword={}\n", password.expose()).unwrap();
    }

    // ---- The credential took, over BOTH transports ----
    let socket_query = vec![
        arg("--defaults-file=", &defaults),
        OsString::from("--protocol=SOCKET"),
        arg("--socket=", &ctx.paths.socket),
        OsString::from("-N"),
        OsString::from("-B"),
        OsString::from("-e"),
        OsString::from("SELECT CURRENT_USER()"),
    ];
    let (ok, out) = client(&runtime.mariadb, &socket_query);
    assert!(ok, "the stored password must authenticate over the socket");
    assert!(out.contains("root@localhost"), "got {out:?}");

    let tcp = |extra: Vec<OsString>| {
        let mut v = vec![
            OsString::from("--protocol=TCP"),
            OsString::from("--host=127.0.0.1"),
            OsString::from("--port=3307"),
            OsString::from("-N"),
            OsString::from("-B"),
            OsString::from("-e"),
            OsString::from("SELECT CURRENT_USER()"),
        ];
        v.splice(0..0, extra);
        v
    };
    let (ok, out) = client(
        &runtime.mariadb,
        &tcp(vec![arg("--defaults-file=", &defaults)]),
    );
    assert!(ok, "the stored password must authenticate over TCP too");
    assert!(out.contains("root@127.0.0.1"), "got {out:?}");

    // THE REGRESSION: `mariadb-install-db` creates root at four hosts, all
    // passwordless. Altering only `root@localhost` leaves this open.
    let (open, out) = client(
        &runtime.mariadb,
        &tcp(vec![
            OsString::from("--no-defaults"),
            OsString::from("--user=root"),
        ]),
    );
    assert!(
        !open,
        "root is reachable over TCP with NO PASSWORD — every reachable root \
         account must be closed, not just root@localhost. Got {out:?}"
    );

    // ---- The four runtime directories come from the package tree ----
    let dirs = mariadb_runtime_dirs(&runtime.mariadbd).expect("the real tree must resolve");
    let mut show = vec![arg("--defaults-file=", &defaults)];
    show.extend([
        OsString::from("--protocol=SOCKET"),
        arg("--socket=", &ctx.paths.socket),
        OsString::from("-N"),
        OsString::from("-B"),
        OsString::from("-e"),
        OsString::from(
            "SHOW VARIABLES WHERE Variable_name IN \
             ('basedir','plugin_dir','character_sets_dir','lc_messages_dir')",
        ),
    ]);
    let (ok, vars) = client(&runtime.mariadb, &show);
    assert!(ok, "SHOW VARIABLES must run");
    let pkg = dirs.basedir.to_string_lossy().into_owned();
    assert_eq!(
        vars.lines().count(),
        4,
        "all four must be reported, got {vars:?}"
    );
    for line in vars.lines() {
        let value = line.split('\t').nth(1).unwrap_or_default();
        assert!(
            value.starts_with(&pkg),
            "{line:?} does not point inside the package tree {pkg} — the server \
             is resolving it out of its compiled-in prefix"
        );
    }
    assert!(
        !vars.contains("/opt/openvhost-build/"),
        "the compiled-in build prefix must never appear: {vars:?}"
    );

    // ---- Stop cleanly ----
    let mut shutdown = vec![arg("--defaults-file=", &defaults)];
    shutdown.extend([
        OsString::from("--protocol=SOCKET"),
        arg("--socket=", &ctx.paths.socket),
        OsString::from("shutdown"),
    ]);
    let (ok, _) = client(&runtime.mariadb_admin, &shutdown);
    assert!(ok, "mariadb-admin shutdown must succeed");
    let status = tokio::time::timeout(Duration::from_secs(30), server.wait())
        .await
        .expect("the server must exit within its grace")
        .expect("waiting on the server must not error");
    assert!(status.success(), "the server exited {status:?}");
    assert!(
        !ctx.paths.socket.exists(),
        "a clean shutdown must remove its socket"
    );
}
