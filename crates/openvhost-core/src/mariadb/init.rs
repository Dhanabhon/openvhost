// SPDX-License-Identifier: GPL-3.0-or-later
//! Staged initialization of the packaged MariaDB series' datadir (spec D6:
//! docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md).
//!
//! The SHAPE is MySQL's (`crate::mysql::init`) — render, initialize into
//! staging, start a temp server, set the password, shut down, atomically
//! finalize — and the two hard-won containments carry unchanged:
//!
//! - **`--no-defaults` on every init-time child**, so a user's `!includedir`
//!   drop-ins cannot steer a server whose root is still open;
//! - **the temp server never goes through the Supervisor.** It is spawned
//!   directly through [`openvhost_proc::ProcessDriver`] behind a manual
//!   kill guard ([`TempServerGuard`]), because nothing else in this app would
//!   ever notice it — the orphaned-process class P0-8's work exists to prevent.
//!
//! Unlike MySQL's, this module DRIVES the sequence rather than only supplying
//! its pieces. MySQL's driver lives in the desktop crate's command layer
//! because that is where its IPC command lives; slice A has no MariaDB command
//! (spec §1), so the driver lives here, where `openvhost-core`'s existing
//! `openvhost-proc` dependency already reaches.
//!
//! # What was measured, not assumed (2026-08-04, real 11.4.9 artifact)
//!
//! **Root authentication.** A default `mariadb-install-db` leaves
//! `root@localhost` with `authentication_string: "invalid"` and
//! `auth_or: [{}, {"plugin":"unix_socket"}]` — password login is deliberately
//! impossible, and connecting as a non-root OS user fails `ERROR 1698 (28000)`.
//! `--auth-root-authentication-method=normal` is the lever that replaces that
//! with an ordinary empty password, and it was verified by connecting.
//!
//! **Root exists at FOUR hosts, not one.** That same init creates
//! `root@localhost`, `root@127.0.0.1`, `root@::1` AND `root@<hostname>` —
//! `--skip-name-resolve` does not suppress the last one. MySQL's
//! `--initialize-insecure` creates only `root@localhost`, which is why
//! `crate::mysql::alter_user_sql` is one statement. Setting only
//! `root@localhost` here was measured to leave a **real hole**: with the
//! server then bound to 127.0.0.1:3307, `mariadb --protocol=TCP --user=root`
//! with NO password connected as `root@127.0.0.1` with full privileges. See
//! [`root_password_sql`].
//!
//! **There is no `mysqlx` equivalent to close.** `--mysqlx=OFF` is rejected
//! outright (`unknown variable 'mysqlx=OFF'`, then `Aborting` — and it aborts
//! late, after InnoDB has written into the datadir). Nothing replaces it,
//! because nothing needs to: a temp server started exactly as
//! [`temp_server_spec`] starts one bound EXACTLY the socket it was told to.
//! `lsof -p <pid>` showed a single `unix` descriptor, `lsof -nP -p <pid>
//! -iTCP -sTCP:LISTEN` showed no row for the process at all, and a
//! `find /tmp -maxdepth 1 -type s` sweep found nothing new. MariaDB has never
//! shipped the X Protocol.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use openvhost_proc::{ProcessDriver, SpawnSpec, SpawnedChild};

use super::{MARIADB_SERIES, MariadbDatadirState, MariadbPaths, MariadbRuntime};
use super::{classify_mariadb_datadir, mariadb_paths};
use crate::mysql::{RootPassword, generate_root_password, write_generated_config};

/// How long `mariadb-install-db` gets. Measured at ~5.4 s on an idle M-series
/// machine; this is a ceiling for a cold, loaded one, not a target.
const INSTALL_DB_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the temp server gets to answer `mariadb-admin ping`. Mirrors the
/// MySQL path's own readiness cap.
const TEMP_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Gap between readiness polls — the MySQL path's interval, unchanged.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How much of the temp server's own output a failure reason may quote. Bounds
/// the drain: a server that never becomes ready but never stops logging must
/// not grow this buffer without limit.
const TEMP_SERVER_LOG_TAIL: usize = 8 * 1024;

// ---------------------------------------------------------------------------
// Runtime directories
// ---------------------------------------------------------------------------

/// The four directories a server must be TOLD about so it does not resolve
/// them out of its compiled-in prefix (spec D3).
///
/// Measured 2026-08-04: `mariadbd --verbose --help`, and `SHOW VARIABLES` on a
/// RUNNING server started with `--no-defaults`, both reported `basedir`,
/// `character_sets_dir` and `plugin_dir` under
/// `/opt/openvhost-build/mariadb-11.4.9/` — the build-time prefix, which
/// exists on no user's machine. The server did NOT derive them from `argv[0]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MariadbRuntimeDirs {
    pub basedir: PathBuf,
    pub plugin_dir: PathBuf,
    pub character_sets_dir: PathBuf,
    pub lc_messages_dir: PathBuf,
}

/// Derive [`MariadbRuntimeDirs`] from the discovered `mariadbd`.
///
/// `basedir` is `mariadbd`'s grandparent (`<basedir>/bin/mariadbd`), taken
/// from the DISCOVERED path — which spec D5 already guarantees is a concrete
/// version directory and never `current`. Nothing here consults a configured
/// or user-supplied value, so the tree these four name is always the tree the
/// binary about to be spawned came out of.
///
/// The layout is checked, not assumed: a tree missing any of the three
/// subdirectories yields `None` and the caller reports a Render failure.
/// Deliberately no fall-back to the compiled-in prefix — falling back is
/// precisely the dependence this exists to remove. (Unlike
/// `crate::mysql::mysql_runtime_dirs`, there is only one candidate layout to
/// check: this package tree is one OpenVHost builds itself.)
pub fn mariadb_runtime_dirs(mariadbd: &Path) -> Option<MariadbRuntimeDirs> {
    let bin = mariadbd.parent()?;
    if bin.file_name() != Some(std::ffi::OsStr::new("bin")) {
        return None;
    }
    let basedir = bin.parent()?.to_path_buf();
    let plugin_dir = basedir.join("lib/plugin");
    let character_sets_dir = basedir.join("share/charsets");
    // The PARENT of the per-language directories: the server appends
    // `lc_messages` (e.g. `english/`) itself.
    let lc_messages_dir = basedir.join("share");
    if !(plugin_dir.is_dir()
        && character_sets_dir.is_dir()
        && lc_messages_dir.join("english").is_dir())
    {
        return None;
    }
    Some(MariadbRuntimeDirs {
        basedir,
        plugin_dir,
        character_sets_dir,
        lc_messages_dir,
    })
}

/// `<basedir>/scripts/mariadb-install-db`, when it is really there.
///
/// Derived from the basedir rather than added to [`MariadbRuntime`]: that
/// type's "all three or nothing" rule (`mariadbd`/`mariadb`/`mariadb-admin`)
/// is Task 1's contract and names the binaries the SERVICE lifecycle needs.
/// The initializer is needed once, by this module, and a runtime that can be
/// supervised but not initialized is still a runtime.
pub fn mariadb_install_db_path(dirs: &MariadbRuntimeDirs) -> Option<PathBuf> {
    let p = dirs.basedir.join("scripts").join("mariadb-install-db");
    p.is_file().then_some(p)
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

/// A fresh staging directory PATH for one init attempt:
/// `<staging_parent>/init-11-4-<uuid>`. Pure — never touches the filesystem.
///
/// The name shape is MySQL's exactly, deliberately: `sweep_stale_staging` and
/// `remove_staging_dir` both gate on `is_stale_staging_name`, which accepts
/// any `init-<major>-<minor>-<suffix>` whose major.minor is version-shaped —
/// so `init-11-4-…` is swept and removable by those two functions with no
/// second copy of either. A cross-check test below pins that, because the day
/// the shapes drift is the day an abandoned MariaDB staging directory becomes
/// permanent litter that nothing recognises.
///
/// **No leading dot**, for the reason `crate::mysql::staging_dir_path`
/// documents at length: a datadir basename starting with `.` cannot be
/// restarted after init — decisively isolated against real mysqld 8.4.11.
pub fn mariadb_staging_dir_path(staging_parent: &Path) -> PathBuf {
    let series_dashed = MARIADB_SERIES.replace('.', "-");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    staging_parent.join(format!("init-{series_dashed}-{suffix}"))
}

// ---------------------------------------------------------------------------
// Credential SQL
// ---------------------------------------------------------------------------

/// The SQL that closes root down to exactly the accounts this app hands out,
/// all of them holding `pw` — fed to `mariadb … --user=root` over **stdin**,
/// never as a `-e` argv value and never through the environment (either leaks
/// to any other process of this user through `ps`).
///
/// **Why this is more than MySQL's one `ALTER USER`.** Measured 2026-08-04:
/// `mariadb-install-db --auth-root-authentication-method=normal` creates root
/// at FOUR hosts — `localhost`, `127.0.0.1`, `::1` and the machine's own
/// hostname — every one with an empty password, and `--skip-name-resolve` does
/// not prevent the hostname row. Setting only `root@localhost`, the way
/// `crate::mysql::alter_user_sql` correctly does for a MySQL init that creates
/// exactly one row, left a server bound to 127.0.0.1:3307 accepting
/// `--protocol=TCP --user=root` **with no password at all**, as
/// `root@127.0.0.1`, with full privileges. Verifying the password over the
/// unix socket alone would have shown a clean pass while that hole was open.
///
/// So: the three loopback accounts (socket, IPv4, IPv6 — all three reachable
/// through the endpoint this service publishes) get the password, and the
/// hostname account is REMOVED rather than given one. It is unusable while
/// `skip-name-resolve` is set and becomes usable the moment a user drop-in
/// unsets it, which is not a door to leave for a credential the user was never
/// shown. The anonymous account is removed on the same principle; this init
/// does not create one **because `--skip-test-db` is on the init argv**, which
/// is what suppresses the anonymous pair upstream would otherwise add. So that
/// DELETE is defence-in-depth today — and it stops being defence-in-depth the
/// moment someone drops the flag, which is exactly why the flag is named here
/// rather than left as a fact about this init that a reader has to take on
/// faith. (Two independent live measurements on 2026-08-04 confirmed the
/// account list: four root rows plus a locked `mariadb.sys`, no anonymous row.
/// A review reading this comment without the flag named concluded the claim
/// was false, so the flag earns its mention.) Both DELETEs use
/// `mariadb-secure-installation`'s own idiom
/// against `mysql.global_priv`, and `FLUSH PRIVILEGES` after them is what makes
/// a direct grant-table edit take effect in the running server's ACL cache.
///
/// The two defensive layers `crate::mysql::alter_user_sql` documents apply
/// unchanged and for the identical reason (today's generator emits pure
/// lowercase hex; a future user-chosen password would not):
/// `NO_BACKSLASH_ESCAPES` first, so the only character with meaning inside the
/// literal is the quote itself, then every embedded `'` doubled.
pub fn root_password_sql(pw: &RootPassword) -> String {
    let escaped = pw.expose().replace('\'', "''");
    format!(
        "SET SESSION sql_mode='NO_BACKSLASH_ESCAPES';\n\
         ALTER USER 'root'@'localhost' IDENTIFIED BY '{escaped}';\n\
         ALTER USER IF EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '{escaped}';\n\
         ALTER USER IF EXISTS 'root'@'::1' IDENTIFIED BY '{escaped}';\n\
         DELETE FROM mysql.global_priv \
         WHERE User='root' AND Host NOT IN ('localhost','127.0.0.1','::1');\n\
         DELETE FROM mysql.global_priv WHERE User='';\n\
         FLUSH PRIVILEGES;\n"
    )
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Which step of a staged init failed — a stable discriminator, never parsed
/// out of `reason`'s free text (the `MysqlInitStep`/`ScaffoldStep` precedent).
///
/// There is no `Validate` variant, and its absence is deliberate: MariaDB has
/// no `--validate-config`, so there is no pre-flight step to fail at (see
/// `openvhost_conf::mariadb`'s module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MariadbInitStep {
    /// Rendering and writing `my.cnf`.
    Render,
    /// `mariadb-install-db --no-defaults … --datadir=<staging>`.
    Initialize,
    /// Spawning the network-less temporary server against `<staging>`.
    StartTempServer,
    /// [`root_password_sql`] over the temp server's socket.
    SetPassword,
    /// `mariadb-admin shutdown` of the temp server.
    Shutdown,
    /// Verifying `<staging>`'s sentinels and moving it into place.
    Finalize,
}

/// The result of attempting to initialize the datadir.
///
/// Not itself a `Result`: `AlreadyInitialized`/`Foreign` are expected outcomes
/// read directly off the filesystem — never a state.db boolean — mirroring
/// `MysqlInitOutcome`'s identical reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MariadbInitOutcome {
    /// A fresh datadir was created and the root password set.
    Initialized,
    /// The final datadir already carried both sentinels; nothing was touched.
    AlreadyInitialized,
    /// The final datadir exists, is non-empty, and is not recognizably a
    /// MariaDB datadir of this series. Rendered honestly, never adopted or
    /// deleted — a datadir recording another series is a MIGRATION, and this
    /// slice does not migrate.
    Foreign { detail: String },
    /// Failed partway through. The final datadir is never created, adopted or
    /// deleted by a failed attempt; only the staging directory this attempt
    /// created is ever removed.
    Failed {
        step: MariadbInitStep,
        reason: String,
    },
}

/// Verify `staging` looks like a completed MariaDB init (reusing
/// [`classify_mariadb_datadir`]), clear ONLY macOS Finder clutter from
/// `final_dir` if that is all that stands in the way, then perform the atomic
/// `rename(staging, final_dir)`.
///
/// Never touches `final_dir` at all unless `staging` already carries both
/// sentinels. On ANY failure here, `staging` is left exactly as it was —
/// removing it is `crate::mysql::remove_staging_dir`'s separate job, invoked
/// uniformly by the caller for a failure at any step.
///
/// `clear_ignorable_clutter` is reused in place from `crate::mysql::init`
/// (spec D5): `.DS_Store` is a property of macOS Finder, not of an engine.
pub fn finalize_mariadb_staging(staging: &Path, final_dir: &Path) -> MariadbInitOutcome {
    match classify_mariadb_datadir(staging) {
        Ok(MariadbDatadirState::Initialized { .. }) => {}
        Ok(other) => {
            return MariadbInitOutcome::Failed {
                step: MariadbInitStep::Finalize,
                reason: format!(
                    "staging directory {} did not contain the expected MariaDB sentinels \
                     after init: {other:?}",
                    staging.display()
                ),
            };
        }
        Err(e) => {
            return MariadbInitOutcome::Failed {
                step: MariadbInitStep::Finalize,
                reason: format!(
                    "failed to inspect staging directory {}: {e}",
                    staging.display()
                ),
            };
        }
    }

    if let Err(reason) = crate::mysql::clear_ignorable_clutter(final_dir) {
        return MariadbInitOutcome::Failed {
            step: MariadbInitStep::Finalize,
            reason,
        };
    }

    match std::fs::rename(staging, final_dir) {
        Ok(()) => MariadbInitOutcome::Initialized,
        Err(e) => MariadbInitOutcome::Failed {
            step: MariadbInitStep::Finalize,
            reason: format!(
                "failed to move {} into place at {}: {e}",
                staging.display(),
                final_dir.display()
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// Child-process specs
// ---------------------------------------------------------------------------

/// `--key=<path>`, built through `OsString` rather than `format!` + `.display()`
/// — the latter is a lossy UTF-8 conversion, and these go straight into an
/// argv (never a shell), so there is no reason to risk mangling a path.
fn path_arg(key: &str, value: &Path) -> OsString {
    let mut arg = OsString::from(key);
    arg.push(value.as_os_str());
    arg
}

/// `mariadb-install-db --no-defaults --basedir=… --datadir=<staging>
/// --auth-root-authentication-method=normal --skip-test-db --skip-name-resolve`.
///
/// `--no-defaults` is containment: the user's `!includedir` drop-ins must not
/// be able to steer a server whose root has no password yet.
///
/// `--auth-root-authentication-method=normal` is LOAD-BEARING (see this
/// module's doc): without it root authenticates by `unix_socket` and the
/// password this app is about to generate cannot be set or used at all.
///
/// `--skip-test-db` leaves out the `test` database and the anonymous account
/// that historically came with it. `--skip-name-resolve` matches the generated
/// `my.cnf`, so the running server and its initializer agree; note it does NOT
/// stop the hostname root row being created — [`root_password_sql`] removes it.
fn install_db_spec(install_db: &Path, dirs: &MariadbRuntimeDirs, staging: &Path) -> SpawnSpec {
    SpawnSpec {
        program: install_db.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            path_arg("--basedir=", &dirs.basedir),
            path_arg("--datadir=", staging),
            OsString::from("--auth-root-authentication-method=normal"),
            OsString::from("--skip-test-db"),
            OsString::from("--skip-name-resolve"),
        ],
        cwd: None,
        env: vec![],
    }
}

/// The network-less temporary server: `mariadbd --no-defaults --basedir=…
/// --datadir=<staging> --plugin-dir=… --character-sets-dir=… --lc-messages-dir=…
/// --skip-networking --socket=<init_socket>`.
///
/// `--no-defaults` MUST be first — the option parser recognises
/// `--no-defaults`/`--defaults-file` ONLY as the very first argument, and
/// anywhere else it is silently not applied.
///
/// **The four runtime directories are passed on argv here, not left to
/// `my.cnf`.** `--no-defaults` means the generated file is not read at all, so
/// without them this one server would resolve plugins and charsets out of the
/// compiled-in prefix — the exact dependence spec D3 exists to remove, and it
/// would be open during the one window when root has no password.
///
/// **No `--mysqlx=OFF`**: rejected outright by MariaDB, and nothing to close —
/// this exact invocation was measured binding EXACTLY the one socket named
/// here and no TCP listener at all (see this module's doc).
fn temp_server_spec(
    mariadbd: &Path,
    dirs: &MariadbRuntimeDirs,
    staging: &Path,
    init_socket: &Path,
) -> SpawnSpec {
    SpawnSpec {
        program: mariadbd.to_path_buf(),
        args: vec![
            OsString::from("--no-defaults"),
            path_arg("--basedir=", &dirs.basedir),
            path_arg("--datadir=", staging),
            path_arg("--plugin-dir=", &dirs.plugin_dir),
            path_arg("--character-sets-dir=", &dirs.character_sets_dir),
            path_arg("--lc-messages-dir=", &dirs.lc_messages_dir),
            OsString::from("--skip-networking"),
            path_arg("--socket=", init_socket),
        ],
        cwd: None,
        env: vec![],
    }
}

/// Kills the temp server's whole process group if the init future is ABANDONED
/// (aborted — e.g. the app quits mid-init) before the server was deliberately
/// shut down.
///
/// This child is spawned directly through [`ProcessDriver`], never through the
/// Supervisor — so unlike every other child this app spawns, NOTHING else
/// would ever kill or even notice it if this guard did not exist. That is
/// precisely the orphaned-process class P0-8's containment work exists to
/// prevent. Mirrors `commands.rs::TempServerGuard` exactly.
///
/// `Drop::drop` cannot `.await`, so it can only SIGNAL the kill, never confirm
/// the exit; the explicit kill+wait at each failure arm below remains the
/// CONFIRMED-dead path for an ordinary failure. `finished` is set once the
/// server has actually exited, so the normal paths do not attempt a second,
/// redundant signal (harmless either way — `kill` on a reaped pid is a no-op).
struct TempServerGuard {
    driver: Arc<dyn ProcessDriver>,
    child: SpawnedChild,
    finished: bool,
}

impl Drop for TempServerGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.driver.kill(&mut self.child);
        }
    }
}

/// A `[client]` defaults file holding the root password, at mode 0600 **from
/// the first byte on disk** — opened with `create_new` + `mode` together,
/// never `write` then a separate `chmod`, so there is no window in which it is
/// group- or world-readable. Removed on drop.
///
/// Exists because the password must never reach argv or the environment: `ps`
/// and `/proc` leak both to every other process running as this user. A
/// short-lived 0600 file and a pipe to stdin are the only two ways across, and
/// `mariadb-admin shutdown` has no stdin channel.
///
/// `protocol=SOCKET` pins the client to the unix socket regardless of the
/// `socket=` line: without it, a missing or stale socket silently falls back
/// to TCP, which could hand this app's credential to an unrelated server that
/// happens to be listening.
struct EphemeralDefaultsFile {
    path: PathBuf,
}

impl EphemeralDefaultsFile {
    fn write(socket: &Path, password: &RootPassword) -> std::io::Result<Self> {
        let run_dir = socket.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(run_dir)?;
        let name = format!(".mariadb-defaults-{}", uuid::Uuid::new_v4().simple());
        let path = run_dir.join(name);
        let contents = format!(
            "[client]\nuser=root\npassword={}\nsocket={}\nprotocol=SOCKET\n",
            password.expose(),
            socket.display()
        );
        {
            use std::io::Write as _;
            let mut f = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)?
                }
                #[cfg(not(unix))]
                {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)?
                }
            };
            f.write_all(contents.as_bytes())?;
            f.sync_all()?;
        }
        Ok(Self { path })
    }
}

impl Drop for EphemeralDefaultsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// Everything one [`initialize_mariadb`] run needs, gathered once so every
/// step shares identical values instead of re-deriving them.
#[derive(Debug, Clone)]
pub struct MariadbInitCtx {
    pub runtime: MariadbRuntime,
    pub paths: MariadbPaths,
    /// How long the temp server gets to answer `mariadb-admin ping`. A field
    /// rather than a constant read inside the loop purely so a test can cap a
    /// deliberately-never-ready server at seconds instead of
    /// [`TEMP_SERVER_READY_TIMEOUT`]; production never sets it.
    pub ready_timeout: Duration,
}

impl MariadbInitCtx {
    /// The context for `home`, given a discovered runtime.
    pub fn new(home: &Path, runtime: MariadbRuntime) -> Self {
        Self {
            runtime,
            paths: mariadb_paths(home),
            ready_timeout: TEMP_SERVER_READY_TIMEOUT,
        }
    }
}

/// Poll `mariadb-admin ping` until it succeeds, the temp server dies on its
/// own, or `deadline` elapses.
async fn poll_until_ready(
    mariadb_admin: &Path,
    socket: &Path,
    server: &mut SpawnedChild,
    deadline: Duration,
) -> bool {
    let deadline_at = tokio::time::Instant::now() + deadline;
    loop {
        if ping(mariadb_admin, socket).await {
            return true;
        }
        if matches!(server.try_wait(), Ok(Some(_))) {
            // The temp server died on its own — nothing left to poll.
            return false;
        }
        if tokio::time::Instant::now() >= deadline_at {
            return false;
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// One `mariadb-admin ping` against `socket`, as the still-passwordless root.
/// `--no-defaults` for the same containment reason every other init-time child
/// carries it.
async fn ping(mariadb_admin: &Path, socket: &Path) -> bool {
    let mut cmd = tokio::process::Command::new(mariadb_admin);
    cmd.arg("--no-defaults")
        .arg("--protocol=SOCKET")
        .arg(path_arg("--socket=", socket))
        .arg("--user=root")
        .arg("ping");
    matches!(openvhost_conf::run_bounded(&mut cmd, None).await, Ok(o) if o.status.success())
}

/// Drive the staged init. Returns the generated password alongside the outcome
/// ONLY when that outcome is [`MariadbInitOutcome::Initialized`] — persisting
/// it is the caller's job (`crate::mariadb::MariadbInstanceRepo`), because this
/// function owns no `Db`.
///
/// Every failure path removes ONLY the staging directory this attempt created,
/// and from `StartTempServer` onward kills the temp server. **The final datadir
/// is never created, adopted, or touched by a failed attempt** — the property
/// this module's tests assert by inode as well as by content, because a
/// delete-and-recreate with identical bytes passes a content-only check and
/// this project has proven that.
pub async fn initialize_mariadb(
    ctx: &MariadbInitCtx,
    driver: Arc<dyn ProcessDriver>,
) -> (MariadbInitOutcome, Option<RootPassword>) {
    use MariadbInitStep as Step;

    macro_rules! fail {
        ($step:expr, $reason:expr) => {
            return (
                MariadbInitOutcome::Failed {
                    step: $step,
                    reason: $reason,
                },
                None,
            )
        };
    }

    // ---- Classify the FINAL datadir first, before anything is written ----
    //
    // An already-initialized or foreign datadir is reported having touched
    // nothing at all, not even a log line.
    match classify_mariadb_datadir(&ctx.paths.datadir) {
        Ok(MariadbDatadirState::Initialized { .. }) => {
            return (MariadbInitOutcome::AlreadyInitialized, None);
        }
        Ok(MariadbDatadirState::Foreign { detail }) => {
            return (MariadbInitOutcome::Foreign { detail }, None);
        }
        // Exhaustive by construction: no wildcard arm, so a fourth state must
        // break compilation here rather than silently authorise `--initialize`.
        Ok(MariadbDatadirState::NotInitialized) => {}
        Err(e) => fail!(
            Step::Render,
            format!(
                "failed to inspect the datadir {}: {e}",
                ctx.paths.datadir.display()
            )
        ),
    }

    if let Err(e) = ctx.paths.check_socket_lengths() {
        fail!(Step::Render, e.to_string());
    }

    let dirs = match mariadb_runtime_dirs(&ctx.runtime.mariadbd) {
        Some(d) => d,
        None => fail!(
            Step::Render,
            format!(
                "{} does not look like a usable MariaDB install: could not locate its \
                 plugin, charset and message directories",
                ctx.runtime.mariadbd.display()
            )
        ),
    };
    let install_db = match mariadb_install_db_path(&dirs) {
        Some(p) => p,
        None => fail!(
            Step::Render,
            format!(
                "{} has no scripts/mariadb-install-db",
                dirs.basedir.display()
            )
        ),
    };

    // ---- Render ----
    let conf_ctx = openvhost_conf::MariadbCtx {
        my_cnf: ctx.paths.my_cnf.clone(),
        datadir: ctx.paths.datadir.clone(),
        socket: ctx.paths.socket.clone(),
        pid_file: ctx.paths.pid_file.clone(),
        custom_confd: ctx.paths.custom_confd.clone(),
        basedir: dirs.basedir.clone(),
        plugin_dir: dirs.plugin_dir.clone(),
        character_sets_dir: dirs.character_sets_dir.clone(),
        lc_messages_dir: dirs.lc_messages_dir.clone(),
    };
    let generated = match openvhost_conf::generate_mariadb_my_cnf(&conf_ctx) {
        Ok(f) => f,
        Err(e) => fail!(Step::Render, e.to_string()),
    };
    // `write_generated_config` is reused in place (spec D5) — it is also the
    // chokepoint that creates the `!includedir` target, and a REAL server
    // aborts its defaults handling when that directory does not exist.
    if let Err(e) = write_generated_config(&generated, &ctx.paths.custom_confd) {
        fail!(Step::Render, e.to_string());
    }

    // The run directory must exist before ANY socket is bound. Found by the
    // live gate, not by a unit test: a real `mariadbd` aborts with
    // "Bind on unix socket: No such file or directory" followed by the
    // spectacularly misleading "Do you already have another server running on
    // socket …?", which sends a reader hunting for a phantom server. The same
    // shape as `write_generated_config`'s `!includedir` chokepoint, and
    // ensured here for the same reason: `provision_home` creates `<home>/run`
    // on a normal launch, but this function must not silently depend on
    // having been called after it.
    // Created at 0700 rather than with a bare `create_dir_all`, because of
    // WHEN this runs: for the whole of this init the temp server's root
    // password is still empty, and the only thing keeping its socket out of
    // another local user's reach is the home being untraversable.
    // `provision_home` is what applies 0700 to the home — so a defensive
    // create that carried no mode would be guarding the directory's EXISTENCE
    // while silently depending on the very property it was written not to
    // assume.
    //
    // The mode applies to components this call creates; an existing `run/` is
    // deliberately left as it is, because it is shared with nginx, php-fpm and
    // MySQL and silently re-moding it here would be a change to their
    // containment made from the wrong place. The home's own 0700 is what
    // covers that case. (MySQL's init has the identical shape and is recorded
    // as a merge precondition on the packaged-MySQL slice, not fixed here.)
    if let Some(run_dir) = ctx.paths.init_socket.parent() {
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt as _;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(run_dir)
        };
        #[cfg(not(unix))]
        let created = std::fs::create_dir_all(run_dir);
        if let Err(e) = created {
            fail!(
                Step::Render,
                format!("failed to create {}: {e}", run_dir.display())
            );
        }
    }

    // ---- Initialize into staging ----
    let staging = mariadb_staging_dir_path(&ctx.paths.staging_parent);
    if let Err(e) = std::fs::create_dir_all(&staging) {
        fail!(
            Step::Initialize,
            format!("failed to create {}: {e}", staging.display())
        );
    }

    /// Remove only the staging directory this attempt created, then fail.
    macro_rules! fail_cleanup {
        ($step:expr, $reason:expr) => {{
            let _ = crate::mysql::remove_staging_dir(&staging);
            fail!($step, $reason)
        }};
    }

    let spec = install_db_spec(&install_db, &dirs, &staging);
    let mut child = match driver.spawn(&spec) {
        Ok(c) => c,
        Err(e) => fail_cleanup!(
            Step::Initialize,
            format!("failed to run {}: {e}", install_db.display())
        ),
    };
    match tokio::time::timeout(INSTALL_DB_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(Ok(status)) => fail_cleanup!(
            Step::Initialize,
            match status.code() {
                Some(code) => format!("mariadb-install-db exited {code}"),
                None => "mariadb-install-db was terminated by a signal".to_string(),
            }
        ),
        Ok(Err(e)) => fail_cleanup!(
            Step::Initialize,
            format!("failed to wait for mariadb-install-db: {e}")
        ),
        Err(_) => {
            let _ = driver.kill(&mut child);
            let _ = child.wait().await;
            fail_cleanup!(
                Step::Initialize,
                format!(
                    "mariadb-install-db did not finish within {}s",
                    INSTALL_DB_TIMEOUT.as_secs()
                )
            )
        }
    }

    // ---- Start the temp server (NEVER through the Supervisor) ----
    let spec = temp_server_spec(
        &ctx.runtime.mariadbd,
        &dirs,
        &staging,
        &ctx.paths.init_socket,
    );
    let mut child = match driver.spawn(&spec) {
        Ok(c) => c,
        Err(e) => fail_cleanup!(
            Step::StartTempServer,
            format!("failed to start the temporary server: {e}")
        ),
    };
    // The driver pipes stdout/stderr and nothing else in this crate drains
    // them. Two reasons that matters, both learned the hard way: a child whose
    // pipe fills BLOCKS forever, and a temp server that dies during startup
    // otherwise reports "did not become ready" with no hint of why. Draining
    // into a bounded tail fixes both, and the tail is what a failure reason
    // gets to quote. The temp server never sees the password (it goes to a
    // DIFFERENT process, on stdin), so nothing here needs redacting.
    let log = Arc::new(std::sync::Mutex::new(String::new()));
    for stream in [child.take_stdout(), child.take_stderr()] {
        let Some(stream) = stream else { continue };
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut buf) = log.lock()
                    && buf.len() < TEMP_SERVER_LOG_TAIL
                {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });
    }
    let server_log = || {
        log.lock()
            .map(|b| b.trim_end().to_string())
            .unwrap_or_default()
    };
    let mut guard = TempServerGuard {
        driver: Arc::clone(&driver),
        child,
        finished: false,
    };

    /// Kill the temp server (confirmed dead), drop the staging dir, then fail.
    macro_rules! fail_temp_server {
        ($step:expr, $reason:expr) => {{
            let _ = guard.driver.kill(&mut guard.child);
            let _ = guard.child.wait().await;
            guard.finished = true;
            fail_cleanup!($step, $reason)
        }};
    }

    if !poll_until_ready(
        &ctx.runtime.mariadb_admin,
        &ctx.paths.init_socket,
        &mut guard.child,
        ctx.ready_timeout,
    )
    .await
    {
        let tail = server_log();
        fail_temp_server!(
            Step::StartTempServer,
            format!(
                "the temporary server did not become ready within {}s: {tail}",
                ctx.ready_timeout.as_secs()
            )
        );
    }

    // ---- Set the password (stdin only) ----
    let password = generate_root_password();
    let sql = root_password_sql(&password);
    let mut cmd = tokio::process::Command::new(&ctx.runtime.mariadb);
    cmd.arg("--no-defaults")
        .arg("--protocol=SOCKET")
        .arg(path_arg("--socket=", &ctx.paths.init_socket))
        .arg("--user=root");
    match openvhost_conf::run_bounded(&mut cmd, Some(sql.as_bytes())).await {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            // stderr is echoed, NOT the SQL — that text contains the password.
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            fail_temp_server!(
                Step::SetPassword,
                format!("failed to set the root password: {stderr}")
            );
        }
        Err(e) => fail_temp_server!(
            Step::SetPassword,
            format!("failed to run the root-password client: {e}")
        ),
    }

    // ---- Shut the temp server down cleanly ----
    let defaults = match EphemeralDefaultsFile::write(&ctx.paths.init_socket, &password) {
        Ok(f) => f,
        Err(e) => fail_temp_server!(
            Step::Shutdown,
            format!("failed to write the ephemeral defaults file: {e}")
        ),
    };
    let mut cmd = tokio::process::Command::new(&ctx.runtime.mariadb_admin);
    cmd.arg(path_arg("--defaults-file=", &defaults.path))
        .arg("shutdown");
    let shutdown_ok = matches!(
        openvhost_conf::run_bounded(&mut cmd, None).await,
        Ok(o) if o.status.success()
    );
    drop(defaults);
    if !shutdown_ok {
        fail_temp_server!(
            Step::Shutdown,
            "mariadb-admin shutdown did not succeed".to_string()
        );
    }
    // Confirmed dead before finalize: renaming a datadir out from under a live
    // server is how a half-flushed InnoDB gets left behind.
    let _ = guard.child.wait().await;
    guard.finished = true;

    // ---- Finalize ----
    match finalize_mariadb_staging(&staging, &ctx.paths.datadir) {
        MariadbInitOutcome::Initialized => (MariadbInitOutcome::Initialized, Some(password)),
        other => {
            let _ = crate::mysql::remove_staging_dir(&staging);
            (other, None)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mysql::sweep_stale_staging;

    // ---- mariadb_runtime_dirs ----

    /// Lay down the real package-tree shape under `base` and return
    /// `<base>/bin/mariadbd`.
    fn fake_tree(base: &Path) -> PathBuf {
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::create_dir_all(base.join("scripts")).unwrap();
        std::fs::create_dir_all(base.join("lib/plugin")).unwrap();
        std::fs::create_dir_all(base.join("share/charsets")).unwrap();
        std::fs::create_dir_all(base.join("share/english")).unwrap();
        std::fs::write(base.join("scripts/mariadb-install-db"), b"#!/bin/sh\n").unwrap();
        let mariadbd = base.join("bin/mariadbd");
        std::fs::write(&mariadbd, b"#!/bin/sh\n").unwrap();
        mariadbd
    }

    #[test]
    fn runtime_dirs_name_the_package_tree_the_binary_came_out_of() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("packages/mariadb/11.4/11.4.9");
        let mariadbd = fake_tree(&base);

        let d = mariadb_runtime_dirs(&mariadbd).expect("a complete tree must resolve");

        assert_eq!(d.basedir, base);
        assert_eq!(d.plugin_dir, base.join("lib/plugin"));
        assert_eq!(d.character_sets_dir, base.join("share/charsets"));
        // The PARENT of `english/`, never `english/` itself.
        assert_eq!(d.lc_messages_dir, base.join("share"));
        for p in [&d.plugin_dir, &d.character_sets_dir, &d.lc_messages_dir] {
            assert!(p.starts_with(&d.basedir), "{} escaped", p.display());
        }
    }

    /// VACUITY for the group: the same fixture with ONE directory removed.
    /// Restore it and the sibling tests above pass — that is the
    /// break-it-and-watch-it-fail step, standing. Refusing is the point: a
    /// fall-back would reinstate the compiled-in-prefix dependence spec D3
    /// exists to remove.
    #[test]
    fn runtime_dirs_refuse_a_tree_missing_its_plugin_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("11.4.9");
        let mariadbd = fake_tree(&base);
        std::fs::remove_dir_all(base.join("lib/plugin")).unwrap();

        assert!(mariadb_runtime_dirs(&mariadbd).is_none());
    }

    #[test]
    fn runtime_dirs_refuse_a_tree_with_no_message_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("11.4.9");
        let mariadbd = fake_tree(&base);
        std::fs::remove_dir_all(base.join("share/english")).unwrap();

        assert!(mariadb_runtime_dirs(&mariadbd).is_none());
    }

    #[test]
    fn runtime_dirs_refuse_a_binary_that_is_not_under_a_bin_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("11.4.9");
        fake_tree(&base);
        std::fs::create_dir_all(base.join("sbin")).unwrap();
        let elsewhere = base.join("sbin/mariadbd");
        std::fs::write(&elsewhere, b"#!/bin/sh\n").unwrap();

        assert!(mariadb_runtime_dirs(&elsewhere).is_none());
    }

    #[test]
    fn install_db_is_found_beside_the_binaries_and_refused_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("11.4.9");
        let mariadbd = fake_tree(&base);
        let dirs = mariadb_runtime_dirs(&mariadbd).unwrap();

        assert_eq!(
            mariadb_install_db_path(&dirs),
            Some(base.join("scripts/mariadb-install-db"))
        );

        std::fs::remove_file(base.join("scripts/mariadb-install-db")).unwrap();
        assert!(mariadb_install_db_path(&dirs).is_none());
    }

    // ---- staging ----

    #[test]
    fn staging_dir_path_is_series_shaped_under_the_given_parent_with_no_leading_dot() {
        let parent = PathBuf::from("/tmp/ovh/data/mariadb");
        let path = mariadb_staging_dir_path(&parent);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("init-11-4-"), "got {name:?}");
        assert!(
            !name.starts_with('.'),
            "a leading dot on a datadir basename is fatal to restarting on it: {name:?}"
        );
        assert_eq!(path.parent(), Some(parent.as_path()));
    }

    #[test]
    fn two_staging_dir_paths_differ() {
        let parent = PathBuf::from("/tmp/ovh/data/mariadb");
        assert_ne!(
            mariadb_staging_dir_path(&parent),
            mariadb_staging_dir_path(&parent)
        );
    }

    /// The cross-check that keeps MariaDB's staging names inside the sweeper's
    /// shape rule: whatever this function produces must be swept if abandoned,
    /// or an abandoned attempt becomes litter nothing recognises.
    #[test]
    fn a_mariadb_staging_dir_is_recognized_by_the_shared_stale_staging_sweep() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = mariadb_staging_dir_path(tmp.path());
        std::fs::create_dir(&staging).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert_eq!(removed, vec![staging]);
    }

    /// …and by the single-directory remover, which applies the same shape
    /// guard before deleting anything.
    #[test]
    fn a_mariadb_staging_dir_is_removable_by_the_shared_single_remover() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = mariadb_staging_dir_path(tmp.path());
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("partial"), b"x").unwrap();

        crate::mysql::remove_staging_dir(&staging).unwrap();

        assert!(!staging.exists());
    }

    // ---- root_password_sql ----

    #[test]
    fn root_password_sql_sets_every_loopback_root_account() {
        let pw = generate_root_password();
        let sql = root_password_sql(&pw);
        for host in ["localhost", "127.0.0.1", "::1"] {
            assert!(
                sql.contains(&format!("'root'@'{host}' IDENTIFIED BY")),
                "root@{host} must get the password — measured live, a server bound to \
                 127.0.0.1:3307 accepts root over TCP with NO password when only \
                 root@localhost is set. Got:\n{sql}"
            );
        }
    }

    #[test]
    fn root_password_sql_removes_the_hostname_and_anonymous_accounts_then_flushes() {
        let sql = root_password_sql(&generate_root_password());
        let delete_at = sql
            .find("DELETE FROM mysql.global_priv \nWHERE User='root'")
            .or_else(|| sql.find("WHERE User='root' AND Host NOT IN"))
            .expect("the hostname root row must be removed");
        let anon_at = sql
            .find("DELETE FROM mysql.global_priv WHERE User='';")
            .expect("the anonymous account must be removed");
        let flush_at = sql.find("FLUSH PRIVILEGES;").expect("must flush");
        assert!(
            delete_at < flush_at && anon_at < flush_at,
            "a direct grant-table DELETE only takes effect in the running ACL after \
             FLUSH PRIVILEGES: {sql}"
        );
    }

    #[test]
    fn root_password_sql_puts_the_no_backslash_escapes_preamble_first() {
        let sql = root_password_sql(&generate_root_password());
        let preamble = sql
            .find("SET SESSION sql_mode='NO_BACKSLASH_ESCAPES'")
            .expect("preamble missing");
        let alter = sql.find("ALTER USER").expect("ALTER USER missing");
        assert!(preamble < alter, "got {sql:?}");
    }

    #[test]
    fn root_password_sql_doubles_an_embedded_single_quote() {
        // Impossible from the real generator (pure hex); written defensively
        // for the deferred user-chosen-password case, exactly as MySQL's is.
        let pw = RootPassword::from_stored("ab'cd".to_string());
        let sql = root_password_sql(&pw);
        assert!(sql.contains("ab''cd"), "got {sql:?}");
        assert!(!sql.contains("BY 'ab'cd'"), "got {sql:?}");
    }

    // ---- child-process argv ----

    fn fixture_dirs() -> MariadbRuntimeDirs {
        MariadbRuntimeDirs {
            basedir: PathBuf::from("/pkg/11.4.9"),
            plugin_dir: PathBuf::from("/pkg/11.4.9/lib/plugin"),
            character_sets_dir: PathBuf::from("/pkg/11.4.9/share/charsets"),
            lc_messages_dir: PathBuf::from("/pkg/11.4.9/share"),
        }
    }

    fn argv(spec: &SpawnSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn install_db_argv_is_contained_and_asks_for_a_password_authenticable_root() {
        let spec = install_db_spec(
            Path::new("/pkg/11.4.9/scripts/mariadb-install-db"),
            &fixture_dirs(),
            Path::new("/home/data/mariadb/init-11-4-abc"),
        );
        let args = argv(&spec);
        assert_eq!(args.first().map(String::as_str), Some("--no-defaults"));
        assert!(
            args.contains(&"--auth-root-authentication-method=normal".to_string()),
            "LOAD-BEARING: without it root authenticates by unix_socket \
             (measured: ERROR 1698) and no password can be set. Got {args:?}"
        );
        assert!(args.contains(&"--datadir=/home/data/mariadb/init-11-4-abc".to_string()));
        assert!(args.contains(&"--basedir=/pkg/11.4.9".to_string()));
    }

    /// The temp server runs with `--no-defaults`, so the generated `my.cnf`'s
    /// pins do NOT apply to it — the four must be on argv or this one server
    /// resolves plugins and charsets out of the compiled-in prefix during the
    /// exact window when root has no password.
    ///
    /// VACUITY: proven by deleting any one `path_arg` line from
    /// `temp_server_spec` — the matching assertion fails, naming the flag.
    #[test]
    fn temp_server_argv_pins_all_four_runtime_directories_despite_no_defaults() {
        let spec = temp_server_spec(
            Path::new("/pkg/11.4.9/bin/mariadbd"),
            &fixture_dirs(),
            Path::new("/home/data/mariadb/init-11-4-abc"),
            Path::new("/home/run/mariadb-11.4-init.sock"),
        );
        let args = argv(&spec);
        assert_eq!(
            args.first().map(String::as_str),
            Some("--no-defaults"),
            "--no-defaults is recognised ONLY as the very first argument: {args:?}"
        );
        for expected in [
            "--basedir=/pkg/11.4.9",
            "--plugin-dir=/pkg/11.4.9/lib/plugin",
            "--character-sets-dir=/pkg/11.4.9/share/charsets",
            "--lc-messages-dir=/pkg/11.4.9/share",
        ] {
            assert!(
                args.contains(&expected.to_string()),
                "{expected} missing from {args:?}"
            );
        }
    }

    #[test]
    fn temp_server_argv_is_network_less_and_never_names_mysqlx() {
        let spec = temp_server_spec(
            Path::new("/pkg/11.4.9/bin/mariadbd"),
            &fixture_dirs(),
            Path::new("/home/data/mariadb/init-11-4-abc"),
            Path::new("/home/run/mariadb-11.4-init.sock"),
        );
        let args = argv(&spec);
        assert!(
            args.contains(&"--skip-networking".to_string()),
            "got {args:?}"
        );
        assert!(
            args.contains(&"--socket=/home/run/mariadb-11.4-init.sock".to_string()),
            "got {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("mysqlx")),
            "mariadbd rejects the directive outright and aborts AFTER InnoDB has \
             written into the datadir: {args:?}"
        );
    }

    /// The password must never be visible to `ps`. Pins both argv and env for
    /// every child spec this module builds.
    ///
    /// VACUITY: proven by appending
    /// `path_arg("--password=", Path::new(pw.expose()))` to
    /// `temp_server_spec` — this fails.
    #[test]
    fn no_spawn_spec_carries_the_password_in_argv_or_env() {
        let pw = generate_root_password();
        let dirs = fixture_dirs();
        let specs = [
            install_db_spec(
                Path::new("/pkg/11.4.9/scripts/mariadb-install-db"),
                &dirs,
                Path::new("/home/data/mariadb/init-11-4-abc"),
            ),
            temp_server_spec(
                Path::new("/pkg/11.4.9/bin/mariadbd"),
                &dirs,
                Path::new("/home/data/mariadb/init-11-4-abc"),
                Path::new("/home/run/mariadb-11.4-init.sock"),
            ),
        ];
        for spec in &specs {
            for a in &spec.args {
                assert!(
                    !a.to_string_lossy().contains(pw.expose()),
                    "the password reached argv: {a:?}"
                );
            }
            assert!(
                spec.env.is_empty(),
                "no init-time child sets an environment variable at all: {:?}",
                spec.env
            );
        }
    }

    // ---- the driver: what a FAILED init must not touch ----

    /// A hermetic home under `/tmp`, never `$TMPDIR`: the 103-byte `sun_path`
    /// ceiling has bitten this project twice, most recently at 159 bytes, and
    /// macOS's `$TMPDIR` alone is ~50 bytes before anything is joined onto it.
    fn tmp_home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ovh")
            .tempdir_in("/tmp")
            .unwrap()
    }

    fn sh(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A complete fake package tree whose `mariadb-install-db` runs `body`.
    fn fake_runtime(base: &Path, install_db_body: &str) -> MariadbRuntime {
        fake_tree(base);
        sh(&base.join("scripts/mariadb-install-db"), install_db_body);
        sh(&base.join("bin/mariadbd"), "sleep 300");
        sh(&base.join("bin/mariadb"), "exit 0");
        sh(&base.join("bin/mariadb-admin"), "exit 1");
        MariadbRuntime {
            series: MARIADB_SERIES,
            version: "11.4.9".to_string(),
            mariadbd: base.join("bin/mariadbd"),
            mariadb: base.join("bin/mariadb"),
            mariadb_admin: base.join("bin/mariadb-admin"),
        }
    }

    #[cfg(unix)]
    fn ino(p: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().ino()
    }

    /// A snapshot of `<home>/data/` deep enough to catch a delete-and-recreate
    /// with identical bytes — which a content-only check passes, and which this
    /// project has proven happens.
    #[cfg(unix)]
    fn snapshot(data_root: &Path) -> Vec<(PathBuf, u64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![data_root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            out.push((dir.clone(), ino(&dir), Vec::new()));
            for e in std::fs::read_dir(&dir).unwrap() {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.clone(), ino(&p), std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// THE containment property: a failed init creates no partial datadir and
    /// touches nothing that was already under `<home>/data/` — asserted by
    /// **inode as well as content**, because a delete-and-recreate with
    /// identical bytes passes a content-only check.
    ///
    /// VACUITY: proven by inserting `std::fs::remove_file(&keep)?;
    /// std::fs::write(&keep, b"user data, do not touch")?;` immediately before
    /// the snapshot comparison — the bytes match, and the inode assertion is
    /// the one that fails. Separately, replacing the failing
    /// `mariadb-install-db` with `exit 0` reaches a later step and the
    /// `Failed { step: Initialize }` match fails instead of passing vacuously.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_init_creates_no_partial_datadir_and_touches_nothing_under_data() {
        let home = tmp_home();
        let pkg = home.path().join("packages/mariadb/11.4/11.4.9");
        let runtime = fake_runtime(&pkg, "echo 'disk full' 1>&2; exit 1");

        // Pre-existing user data under `<home>/data/` — a sibling engine's
        // datadir is exactly what must survive an unrelated failure.
        let data_root = home.path().join("data");
        let neighbour = data_root.join("mysql/8.4");
        std::fs::create_dir_all(&neighbour).unwrap();
        let keep = neighbour.join("ibdata1");
        std::fs::write(&keep, b"user data, do not touch").unwrap();
        std::fs::create_dir_all(data_root.join("mariadb")).unwrap();
        let before = snapshot(&data_root);

        let ctx = MariadbInitCtx::new(home.path(), runtime);
        let (outcome, pw) = initialize_mariadb(&ctx, openvhost_proc::default_driver()).await;

        match &outcome {
            MariadbInitOutcome::Failed { step, .. } => {
                assert_eq!(*step, MariadbInitStep::Initialize)
            }
            other => panic!("expected Failed at Initialize, got {other:?}"),
        }
        assert!(pw.is_none(), "no credential may escape a failed init");
        assert!(
            !ctx.paths.datadir.exists(),
            "a failed init must never create the final datadir"
        );
        assert_eq!(
            before,
            snapshot(&data_root),
            "<home>/data/ changed — compare inodes, not just bytes"
        );
        let leftovers: Vec<_> = std::fs::read_dir(data_root.join("mariadb"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "the staging directory must be removed on failure, found {leftovers:?}"
        );
    }

    /// The same property one step later: the temp server starts but never
    /// becomes ready. This is the arm that must also leave no live child.
    ///
    /// VACUITY: proven by making `mariadb-admin` `exit 0` (a ping that always
    /// succeeds) — the run then advances past StartTempServer and the step
    /// assertion fails.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_init_whose_temp_server_never_answers_leaves_no_datadir_and_no_child() {
        let home = tmp_home();
        let pkg = home.path().join("packages/mariadb/11.4/11.4.9");
        // install-db "succeeds" and lays down both sentinels; the server then
        // starts (it sleeps) but `mariadb-admin ping` never succeeds.
        let runtime = fake_runtime(
            &pkg,
            r#"
for arg in "$@"; do
  case "$arg" in --datadir=*) d="${arg#--datadir=}" ;; esac
done
mkdir -p "$d/mysql"
printf '11.4.9-MariaDB\n' > "$d/mariadb_upgrade_info"
exit 0
"#,
        );

        let data_root = home.path().join("data");
        std::fs::create_dir_all(data_root.join("mariadb")).unwrap();
        let before = snapshot(&data_root);

        let mut ctx = MariadbInitCtx::new(home.path(), runtime);
        // Keep the test quick: the production cap is 30s.
        ctx.ready_timeout = Duration::from_secs(2);
        let started = std::time::Instant::now();
        let (outcome, pw) = initialize_mariadb(&ctx, openvhost_proc::default_driver()).await;
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the cap must apply"
        );

        match &outcome {
            MariadbInitOutcome::Failed { step, .. } => {
                assert_eq!(*step, MariadbInitStep::StartTempServer)
            }
            other => panic!("expected Failed at StartTempServer, got {other:?}"),
        }
        assert!(pw.is_none());
        assert!(!ctx.paths.datadir.exists());
        assert_eq!(before, snapshot(&data_root));

        // The temp server is spawned OUTSIDE the Supervisor, so nothing else
        // would ever reap it — the kill+wait on this arm is the only thing
        // that does.
        let still_running = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(ctx.paths.init_socket.to_string_lossy().into_owned())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(!still_running, "the temp server outlived the failed init");
    }

    /// An already-initialized datadir is reported without the init sequence
    /// running at all — asserted by giving it a `mariadb-install-db` that
    /// would fail loudly if it were ever reached.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_already_initialized_datadir_is_reported_without_running_anything() {
        let home = tmp_home();
        let pkg = home.path().join("packages/mariadb/11.4/11.4.9");
        let runtime = fake_runtime(&pkg, "exit 1");

        let ctx = MariadbInitCtx::new(home.path(), runtime);
        std::fs::create_dir_all(ctx.paths.datadir.join("mysql")).unwrap();
        std::fs::write(
            ctx.paths.datadir.join("mariadb_upgrade_info"),
            b"11.4.9-MariaDB\n",
        )
        .unwrap();
        let before = snapshot(&home.path().join("data"));

        let (outcome, pw) = initialize_mariadb(&ctx, openvhost_proc::default_driver()).await;

        assert_eq!(outcome, MariadbInitOutcome::AlreadyInitialized);
        assert!(pw.is_none());
        assert_eq!(before, snapshot(&home.path().join("data")));
        assert!(
            !ctx.paths.my_cnf.exists(),
            "not even my.cnf may be written for a datadir we are not initializing"
        );
    }

    /// A datadir written by another series is a MIGRATION, and this slice does
    /// not migrate. Reported honestly, touched not at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_foreign_datadir_is_reported_and_never_initialized_over() {
        let home = tmp_home();
        let pkg = home.path().join("packages/mariadb/11.4/11.4.9");
        let runtime = fake_runtime(&pkg, "exit 1");

        let ctx = MariadbInitCtx::new(home.path(), runtime);
        std::fs::create_dir_all(ctx.paths.datadir.join("mysql")).unwrap();
        std::fs::write(
            ctx.paths.datadir.join("mariadb_upgrade_info"),
            b"11.8.1-MariaDB\n",
        )
        .unwrap();
        let before = snapshot(&home.path().join("data"));

        let (outcome, pw) = initialize_mariadb(&ctx, openvhost_proc::default_driver()).await;

        assert!(
            matches!(outcome, MariadbInitOutcome::Foreign { .. }),
            "got {outcome:?}"
        );
        assert!(pw.is_none());
        assert_eq!(before, snapshot(&home.path().join("data")));
    }

    // ---- EphemeralDefaultsFile ----

    #[cfg(unix)]
    #[test]
    fn the_defaults_file_is_0600_from_the_first_byte_and_gone_after_drop() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("run").join("mariadb-11.4-init.sock");
        let pw = generate_root_password();

        let path = {
            let f = EphemeralDefaultsFile::write(&socket, &pw).unwrap();
            let mode = std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "got {mode:o}");
            let contents = std::fs::read_to_string(&f.path).unwrap();
            assert!(contents.contains(pw.expose()));
            assert!(
                contents.lines().any(|l| l == "protocol=SOCKET"),
                "without it a stale socket silently falls back to TCP: {contents:?}"
            );
            f.path.clone()
        };

        assert!(!path.exists(), "the guard must remove the file on drop");
    }

    // ---- The success path, hermetically ----

    /// A package tree whose whole init sequence SUCCEEDS.
    ///
    /// [`fake_runtime`] hardcodes `mariadb-admin` to `exit 1`, so every test
    /// built on it stops at `StartTempServer` — which meant that until this
    /// fixture existed, `SetPassword` → `Shutdown` → `Finalize` →
    /// `Initialized` was reachable ONLY through `tests/mariadb_live.rs`,
    /// behind `OPENVHOST_MARIADB_LIVE_TESTS=1` and a gitignored ~125 MB
    /// artifact. An ordinary `cargo test --workspace` proved nothing about the
    /// single outcome this module exists to produce. (Found by the 2026-08-04
    /// whole-branch review.)
    ///
    /// The shutdown handshake is real rather than timed: `initialize_mariadb`
    /// WAITS on the temp server after `mariadb-admin shutdown` returns, so a
    /// fake server that merely slept would hang the suite for its whole sleep.
    /// Here `mariadb-admin shutdown` creates a file and `mariadbd` polls for
    /// it, so the sequence is causally ordered instead of racing a clock, and
    /// the poll is bounded so a regression cannot hang CI either.
    #[cfg(unix)]
    fn fake_runtime_that_completes(base: &Path) -> MariadbRuntime {
        fake_tree(base);
        let stop = base.join("shutdown-requested").display().to_string();

        sh(
            &base.join("scripts/mariadb-install-db"),
            r#"
for arg in "$@"; do
  case "$arg" in --datadir=*) d="${arg#--datadir=}" ;; esac
done
mkdir -p "$d/mysql"
printf '11.4.9-MariaDB\n' > "$d/mariadb_upgrade_info"
exit 0
"#,
        );
        sh(
            &base.join("bin/mariadbd"),
            &r#"
i=0
while [ ! -f "@STOP@" ] && [ "$i" -lt 600 ]; do
  sleep 0.05
  i=$((i+1))
done
exit 0
"#
            .replace("@STOP@", &stop),
        );
        sh(
            &base.join("bin/mariadb-admin"),
            &r#"
for arg in "$@"; do
  if [ "$arg" = shutdown ]; then : > "@STOP@"; fi
done
exit 0
"#
            .replace("@STOP@", &stop),
        );
        sh(&base.join("bin/mariadb"), "exit 0");

        MariadbRuntime {
            series: MARIADB_SERIES,
            version: "11.4.9".to_string(),
            mariadbd: base.join("bin/mariadbd"),
            mariadb: base.join("bin/mariadb"),
            mariadb_admin: base.join("bin/mariadb-admin"),
        }
    }

    /// The outcome the whole module exists to produce, reached without a real
    /// server: `Initialized`, a credential handed back exactly once, the
    /// datadir at its FINAL path classifying `Initialized`, and no staging
    /// directory left behind.
    ///
    /// VACUITY: proven by making `scripts/mariadb-install-db` write only
    /// `mysql/` and skip `mariadb_upgrade_info` — `finalize_mariadb_staging`
    /// then refuses and the run ends `Failed { step: Finalize }`, so the
    /// `Initialized` assertion fails rather than passing on a technicality.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_init_that_completes_lands_the_datadir_and_yields_one_credential() {
        let home = tmp_home();
        let pkg = home.path().join("packages/mariadb/11.4/11.4.9");
        let runtime = fake_runtime_that_completes(&pkg);

        let ctx = MariadbInitCtx::new(home.path(), runtime);
        let (outcome, pw) = initialize_mariadb(&ctx, openvhost_proc::default_driver()).await;

        assert_eq!(
            outcome,
            MariadbInitOutcome::Initialized,
            "the hermetic success path must be reachable without the live artifact"
        );
        assert!(
            pw.is_some(),
            "a successful init is the ONLY path that yields a credential"
        );
        assert!(
            matches!(
                classify_mariadb_datadir(&ctx.paths.datadir),
                Ok(MariadbDatadirState::Initialized { .. })
            ),
            "the datadir must be Initialized at its final path, not just present"
        );
        assert!(
            !mariadb_staging_dir_path(&ctx.paths.staging_parent).exists(),
            "staging must not survive a successful finalize"
        );
    }

    // ---- finalize_mariadb_staging ----
    //
    // VACUITY for this whole group: proven by returning
    // `MariadbInitOutcome::Initialized` unconditionally at the top of
    // `finalize_mariadb_staging` — all six fail.
    //
    // Worth knowing before reading them: the two "refuses a destination"
    // cases are **doubly enforced**, and a narrower mutation showed it.
    // Ignoring `clear_ignorable_clutter`'s error left all three refusal tests
    // passing, because `rename(2)` independently refuses a non-empty
    // destination. So these tests pin the OUTCOME (and that nothing is
    // deleted), not the identity of the guard that produced it — do not read
    // a green run here as evidence that the clutter guard specifically still
    // works.

    /// A staging directory carrying both sentinels, which is the only input
    /// `finalize_mariadb_staging` will act on.
    #[cfg(unix)]
    fn initialized_staging(at: &Path) {
        std::fs::create_dir_all(at.join("mysql")).unwrap();
        std::fs::write(at.join("mariadb_upgrade_info"), b"11.4.9-MariaDB\n").unwrap();
    }

    /// The ordinary case: `rename` creates `final_dir` fresh.
    #[cfg(unix)]
    #[test]
    fn finalize_moves_staging_into_an_absent_final_dir() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let final_dir = tmp.path().join("final");
        initialized_staging(&staging);

        assert_eq!(
            finalize_mariadb_staging(&staging, &final_dir),
            MariadbInitOutcome::Initialized
        );
        assert!(!staging.exists(), "staging must be consumed by the rename");
        assert!(final_dir.join("mariadb_upgrade_info").is_file());
    }

    /// `rename(2)` accepts an existing destination directory when it is empty.
    #[cfg(unix)]
    #[test]
    fn finalize_moves_staging_into_an_empty_existing_final_dir() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let final_dir = tmp.path().join("final");
        initialized_staging(&staging);
        std::fs::create_dir_all(&final_dir).unwrap();

        assert_eq!(
            finalize_mariadb_staging(&staging, &final_dir),
            MariadbInitOutcome::Initialized
        );
        assert!(final_dir.join("mysql").is_dir());
    }

    /// Finder-only clutter is cleared first — `rename` has no notion of it and
    /// would refuse a non-empty destination.
    #[cfg(unix)]
    #[test]
    fn finalize_clears_finder_clutter_before_renaming() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let final_dir = tmp.path().join("final");
        initialized_staging(&staging);
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join(".DS_Store"), b"finder").unwrap();

        assert_eq!(
            finalize_mariadb_staging(&staging, &final_dir),
            MariadbInitOutcome::Initialized
        );
        assert!(!final_dir.join(".DS_Store").exists());
        assert!(final_dir.join("mariadb_upgrade_info").is_file());
    }

    /// Anything that is not clutter blocks the move, and nothing is deleted —
    /// the destination is somebody's data until proven otherwise.
    #[cfg(unix)]
    #[test]
    fn finalize_refuses_a_final_dir_holding_more_than_clutter() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let final_dir = tmp.path().join("final");
        initialized_staging(&staging);
        std::fs::create_dir_all(&final_dir).unwrap();
        let theirs = final_dir.join("ibdata1");
        std::fs::write(&theirs, b"somebody else's data").unwrap();

        match finalize_mariadb_staging(&staging, &final_dir) {
            MariadbInitOutcome::Failed { step, .. } => {
                assert_eq!(step, MariadbInitStep::Finalize)
            }
            other => panic!("expected Failed at Finalize, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&theirs).unwrap(),
            b"somebody else's data",
            "a refused finalize must not touch the destination"
        );
        assert!(
            staging.exists(),
            "staging is the caller's to remove, not ours"
        );
    }

    /// Half a datadir is not a datadir. `mysql/` without
    /// `mariadb_upgrade_info` is exactly what a killed direct-bootstrap init
    /// leaves behind, and promoting it would put an unusable datadir at the
    /// path everything else trusts.
    #[cfg(unix)]
    #[test]
    fn finalize_refuses_staging_that_is_missing_a_sentinel() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let final_dir = tmp.path().join("final");
        std::fs::create_dir_all(staging.join("mysql")).unwrap();

        match finalize_mariadb_staging(&staging, &final_dir) {
            MariadbInitOutcome::Failed { step, .. } => {
                assert_eq!(step, MariadbInitStep::Finalize)
            }
            other => panic!("expected Failed at Finalize, got {other:?}"),
        }
        assert!(
            !final_dir.exists(),
            "the destination must not be touched AT ALL when staging is not initialized"
        );
    }

    /// A symlink at the destination is refused rather than followed —
    /// otherwise the clutter sweep would delete through it, into a directory
    /// nobody named.
    #[cfg(unix)]
    #[test]
    fn finalize_refuses_a_symlinked_final_dir() {
        let tmp = tmp_home();
        let staging = tmp.path().join("staging");
        let real_target = tmp.path().join("elsewhere");
        let final_dir = tmp.path().join("final");
        initialized_staging(&staging);
        std::fs::create_dir_all(&real_target).unwrap();
        std::fs::write(real_target.join(".DS_Store"), b"finder").unwrap();
        std::os::unix::fs::symlink(&real_target, &final_dir).unwrap();

        match finalize_mariadb_staging(&staging, &final_dir) {
            MariadbInitOutcome::Failed { step, .. } => {
                assert_eq!(step, MariadbInitStep::Finalize)
            }
            other => panic!("expected Failed at Finalize, got {other:?}"),
        }
        assert!(staging.exists());
        assert!(
            real_target.join(".DS_Store").exists(),
            "refusing must not delete through the symlink"
        );
    }
}
