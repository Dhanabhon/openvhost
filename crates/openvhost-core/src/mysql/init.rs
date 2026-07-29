// SPDX-License-Identifier: GPL-3.0-or-later
//! Init/reset primitives for one MySQL major's datadir (spec D2/D3:
//! docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md). Everything
//! here is pure-ish: it takes paths/values and returns data, never spawns a
//! child process. The staged-init SEQUENCE spec D2 describes (
//! `mysqld --initialize-insecure`, a network-less temp server, `ALTER USER`
//! over its socket, `mysqladmin shutdown`) is driven by the command layer
//! (plan Task 5), mirroring the existing PHP install task — this module
//! supplies the credential ([`RootPassword`], [`generate_root_password`],
//! [`alter_user_sql`]), the staging-directory naming ([`staging_dir_path`]),
//! and the two ends of that sequence this crate CAN do without a child
//! process: verifying and moving a completed staging directory into place
//! ([`finalize_staging`]), and cleaning up after a failed attempt
//! ([`remove_staging_dir`]).

use std::io;
use std::path::{Path, PathBuf};

use super::datadir::{DS_STORE, is_stale_staging_name};
use super::{DatadirState, MysqlMajor, classify_datadir};
use crate::error::CoreError;

/// Write a rendered `my.cnf` ([`openvhost_conf::GeneratedFile`]) atomically
/// (spec D5: "written with `atomicfile::write_atomic` as a `GeneratedFile`").
/// Thin — mirrors `site::apply::commit`'s own atomic-write wrapper
/// (`crate::atomicfile::write_atomic`) rather than duplicating its logic;
/// exists as its own `pub fn` here (Task 5) because that wrapper is scoped to
/// `ApplyPlan`/`ApplyError` and `crate::atomicfile` itself is `pub(crate)` —
/// nothing outside this crate could otherwise reach the hardened write at
/// all. The command layer (plan Task 5) drives the render (Task 3's
/// `openvhost_conf::generate_my_cnf`) and calls this to persist it, exactly
/// like `site::apply::commit` drives its own `GeneratedFile`s.
pub fn write_generated_config(file: &openvhost_conf::GeneratedFile) -> Result<(), CoreError> {
    Ok(crate::atomicfile::write_atomic(&file.path, &file.contents)?)
}

/// A fresh v4 UUID rendered in "simple" form: 32 lowercase hex characters,
/// no hyphens. Shared by [`generate_root_password`] (spec D3: "uuid v4
/// simple hex, 32 chars") and [`staging_dir_path`]'s unique suffix — both
/// want the identical shape, so the `uuid` crate call (already a workspace
/// dependency via `crate::site::SiteId::new` — no new dependency) lives in
/// exactly one place.
fn uuid_v4_simple_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// A generated MySQL root credential (spec D3): random, never user-chosen
/// this slice, held only in `state.db` under the OpenVHost home, and sent
/// to `mysqld`/`mysql` only via stdin or an ephemeral 0600 defaults-file —
/// NEVER argv, NEVER env, NEVER a log line. `Debug` is hand-written to
/// redact the value so an incidental `{:?}` in a trace/log/panic message
/// can never leak it; this type deliberately does NOT implement
/// `Serialize`/`specta::Type` either, so an outbound DTO can never be built
/// by accidentally deriving through it — the IPC layer (plan Task 5) must
/// always go through [`RootPassword::expose`] explicitly to cross that
/// boundary, the same discipline `expose`'s own name is meant to enforce.
#[derive(Clone, PartialEq, Eq)]
pub struct RootPassword(String);

impl std::fmt::Debug for RootPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootPassword(<redacted>)")
    }
}

impl RootPassword {
    /// Wrap a value already stored in `state.db` back into this type. Not
    /// `pub`: outside this crate the only way to obtain a `RootPassword` is
    /// [`generate_root_password`] — this constructor exists purely for
    /// `crate::mysql::MysqlInstanceRepo`'s own row decoding, reading back a
    /// value THIS process generated and wrote moments (or days) earlier.
    /// There is no untrusted-input boundary here the way there is for e.g.
    /// `crate::site::Docroot::parse`, so no shape check is applied.
    pub(crate) fn from_stored(s: String) -> Self {
        Self(s)
    }

    /// The one escape hatch out of the redaction wrapper. Named `expose`,
    /// not `as_str`, so every call site visibly announces what it is doing
    /// — the discipline this type exists to enforce (spec D3): the result
    /// must go to a child's stdin or an ephemeral 0600 defaults-file,
    /// never to argv, env, or anything that gets logged.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// A fresh, random root credential: a v4 UUID in "simple" form — 32
/// lowercase hex characters, no hyphens (122 bits of randomness; spec D3).
/// Uses the `uuid` crate's CSPRNG-backed `new_v4`, never a hand-rolled RNG.
pub fn generate_root_password() -> RootPassword {
    RootPassword(uuid_v4_simple_hex())
}

/// The SQL script that sets MySQL's `root@localhost` password to `pw`,
/// meant to be fed to `mysql --protocol=SOCKET ... --user=root` over
/// **stdin** (spec D2 step 4) — never as a `mysql -e` argv value, never
/// through an environment variable: both `ps`/`/proc` can leak either to
/// any other process running as this user.
///
/// Two defensive layers apply, regardless of what `pw` actually contains
/// (spec D3: "hex charset makes escaping trivial but write it defensively
/// anyway" — the real generator only ever produces pure lowercase hex, but
/// a future user-chosen password, spec D3's deferred narrowing, would not):
///
/// 1. A `SET SESSION sql_mode='NO_BACKSLASH_ESCAPES';` preamble. MySQL's
///    default `sql_mode` treats `\` as an escape character inside a quoted
///    string literal; disabling that first means the ONLY character with
///    special meaning inside the single-quoted literal below is the quote
///    character itself — no run of trailing backslashes in `pw` can
///    interact with the closing quote.
/// 2. Doubling every embedded `'` to `''` — the ANSI-standard escape for a
///    single-quoted string literal, which (only once backslash-escaping is
///    off) is sufficient on its own: no input can produce a `''` in the
///    output that is anything other than one literal quote character
///    embedded in the string.
pub fn alter_user_sql(pw: &RootPassword) -> String {
    let escaped = pw.expose().replace('\'', "''");
    format!(
        "SET SESSION sql_mode='NO_BACKSLASH_ESCAPES';\n\
         ALTER USER 'root'@'localhost' IDENTIFIED BY '{escaped}';\n"
    )
}

/// Which step of a MySQL staged init (spec D2) a
/// [`MysqlInitOutcome::Failed`] failed at — a stable discriminator for the
/// UI, never parsed out of `reason`'s free text (the `ScaffoldStep`
/// precedent: `crate::site::scaffold::ScaffoldStep`). `Render`/`Validate`
/// cover `openvhost-conf`'s my.cnf generation and pre-flight check (Task
/// 3); everything from `Initialize` onward is a child process the command
/// layer spawns (plan Task 5) mirroring the PHP install task — this crate
/// spawns none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MysqlInitStep {
    /// Rendering `my.cnf` for the candidate datadir.
    Render,
    /// `mysqld --validate-config` against the rendered file.
    Validate,
    /// `mysqld --initialize-insecure --datadir=<staging>`.
    Initialize,
    /// Spawning the network-less temporary server against `<staging>`.
    StartTempServer,
    /// `ALTER USER` over the temp server's socket.
    SetPassword,
    /// `mysqladmin shutdown` of the temp server.
    Shutdown,
    /// Verifying `<staging>`'s sentinels and moving it into place — see
    /// [`finalize_staging`].
    Finalize,
}

/// The result of attempting to initialize one MySQL major's datadir (spec
/// D2). Not itself a `Result`: `AlreadyInitialized`/`Foreign` are expected,
/// non-error outcomes read directly off the filesystem — never a state.db
/// boolean (see [`classify_datadir`]) — mirroring
/// `crate::site::scaffold::ScaffoldOutcome`'s identical reasoning for why
/// this is an enum of outcomes rather than an error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MysqlInitOutcome {
    /// A fresh datadir was created and the root password set.
    Initialized,
    /// The final datadir already had both sentinels present; nothing was
    /// touched.
    AlreadyInitialized,
    /// The final datadir exists, is non-empty, and is not recognizably a
    /// MySQL datadir. Rendered honestly, never adopted or deleted.
    Foreign { detail: String },
    /// Failed partway through. `step` names which stage; the final datadir
    /// is never created, adopted, or deleted by a failed attempt — only the
    /// staging directory this attempt was using is ever removed (see
    /// [`remove_staging_dir`]), and that removal is the caller's separate
    /// responsibility, run uniformly regardless of which step failed.
    Failed { step: MysqlInitStep, reason: String },
}

/// Generate a fresh staging directory PATH for one init attempt (spec D2):
/// `<staging_parent>/.<major>.init-<uuid>`. Pure — never touches the
/// filesystem; the command layer creates a directory there before invoking
/// `mysqld --initialize-insecure --datadir=<staging>` (spec D2 step 1).
/// The returned name is always recognized by [`sweep_stale_staging`] and
/// [`remove_staging_dir`] (both reuse the identical shape check this name
/// satisfies), so an attempt abandoned mid-init is always cleanable up
/// later.
///
/// [`sweep_stale_staging`]: super::sweep_stale_staging
pub fn staging_dir_path(staging_parent: &Path, major: &MysqlMajor) -> PathBuf {
    let suffix = uuid_v4_simple_hex();
    staging_parent.join(format!(".{}.init-{suffix}", major.as_str()))
}

/// Remove exactly the ONE staging directory a failed init attempt was
/// using — the "remove-only-marked-staging" half of spec D2's failure
/// handling ("only the marked staging dir is removed"). Reuses the
/// identical name-shape guard `sweep_stale_staging` applies before removing
/// anything, rather than trusting the caller blindly: if `staging` is not
/// shaped like `.{major}.init-{suffix}`, NOTHING is removed and an error is
/// returned — this must never become a general-purpose `rm -rf`. A missing
/// directory is not an error: there is nothing to clean up.
pub fn remove_staging_dir(staging: &Path) -> io::Result<()> {
    let is_staging_shaped = staging
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_stale_staging_name);
    if !is_staging_shaped {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} does not look like a staging directory — refusing to remove it",
                staging.display()
            ),
        ));
    }
    match std::fs::remove_dir_all(staging) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// If `final_dir` exists and contains ONLY [`DS_STORE`] entries — never
/// anything broader (see this module's doc comment and the macOS clutter
/// amendment on `classify_datadir`) — delete them so the directory the OS
/// sees is genuinely empty and a plain [`std::fs::rename`] into it
/// succeeds. `rename(2)` requires an existing destination directory to be
/// empty; it has no notion of the [`classify_datadir`] amendment that
/// treats Finder-only clutter as [`DatadirState::NotInitialized`]. A
/// missing `final_dir` is not an error — `rename` creates it fresh. A
/// symlink or a non-directory at `final_dir`, or anything else found
/// inside it, refuses to delete ANYTHING (not even the `.DS_Store` files
/// alongside it) and reports what blocked it.
fn clear_ignorable_clutter(final_dir: &Path) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(final_dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", final_dir.display())),
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a symlink — refusing to finalize into it",
            final_dir.display()
        ));
    }
    if !meta.is_dir() {
        return Err(format!(
            "{} already exists and is not a folder",
            final_dir.display()
        ));
    }

    let entries =
        std::fs::read_dir(final_dir).map_err(|e| format!("{}: {e}", final_dir.display()))?;
    let mut ignorable = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", final_dir.display()))?;
        if entry.file_name() == DS_STORE {
            ignorable.push(entry.path());
        } else {
            return Err(format!(
                "{} is not empty and contains {:?}, which is not macOS Finder clutter \
                 — refusing to remove anything",
                final_dir.display(),
                entry.file_name()
            ));
        }
    }
    for path in ignorable {
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Verify `staging` looks like a completed MySQL init (spec D2 step 6's
/// sentinel check, reusing [`classify_datadir`]), clear ONLY macOS Finder
/// clutter from `final_dir` if that is all that stands in the way (see
/// [`clear_ignorable_clutter`]), then perform the atomic
/// `rename(staging, final_dir)`. Never touches `final_dir` at all unless
/// `staging` already carries both sentinels — a failed init never creates,
/// adopts, or deletes the final datadir (spec D2). On ANY failure here,
/// `staging` itself is left exactly as it was: removing it is
/// [`remove_staging_dir`]'s separate job, invoked uniformly by the caller
/// for a failure at ANY step, not just this one.
pub fn finalize_staging(staging: &Path, final_dir: &Path) -> MysqlInitOutcome {
    match classify_datadir(staging) {
        Ok(DatadirState::Initialized) => {}
        Ok(other) => {
            return MysqlInitOutcome::Failed {
                step: MysqlInitStep::Finalize,
                reason: format!(
                    "staging directory {} did not contain the expected MySQL sentinels \
                     after init: {other:?}",
                    staging.display()
                ),
            };
        }
        Err(e) => {
            return MysqlInitOutcome::Failed {
                step: MysqlInitStep::Finalize,
                reason: format!(
                    "failed to inspect staging directory {}: {e}",
                    staging.display()
                ),
            };
        }
    }

    if let Err(reason) = clear_ignorable_clutter(final_dir) {
        return MysqlInitOutcome::Failed {
            step: MysqlInitStep::Finalize,
            reason,
        };
    }

    match std::fs::rename(staging, final_dir) {
        Ok(()) => MysqlInitOutcome::Initialized,
        Err(e) => MysqlInitOutcome::Failed {
            step: MysqlInitStep::Finalize,
            reason: format!(
                "failed to move {} into place at {}: {e}",
                staging.display(),
                final_dir.display()
            ),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mysql::sweep_stale_staging;

    // ---- generate_root_password / RootPassword ----

    #[test]
    fn generate_root_password_is_32_lowercase_hex_chars() {
        let pw = generate_root_password();
        let s = pw.expose();
        assert_eq!(s.len(), 32, "got {s:?}");
        assert!(
            s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "expected 32 lowercase hex chars, got {s:?}"
        );
    }

    #[test]
    fn two_generated_passwords_differ() {
        let a = generate_root_password();
        let b = generate_root_password();
        assert_ne!(a.expose(), b.expose());
    }

    #[test]
    fn root_password_debug_is_exactly_redacted() {
        let pw = generate_root_password();
        let debug = format!("{pw:?}");
        assert_eq!(debug, "RootPassword(<redacted>)");
        assert!(
            !debug.contains(pw.expose()),
            "the Debug output must never contain the real value: {debug:?}"
        );
    }

    // ---- alter_user_sql ----

    #[test]
    fn alter_user_sql_includes_the_no_backslash_escapes_preamble_before_the_alter() {
        let pw = generate_root_password();
        let sql = alter_user_sql(&pw);
        let preamble_pos = sql
            .find("SET SESSION sql_mode='NO_BACKSLASH_ESCAPES'")
            .expect("preamble missing");
        let alter_pos = sql
            .find("ALTER USER 'root'@'localhost'")
            .expect("ALTER USER missing");
        assert!(
            preamble_pos < alter_pos,
            "preamble must come before ALTER USER: {sql:?}"
        );
    }

    #[test]
    fn alter_user_sql_embeds_a_plain_hex_password_verbatim() {
        let pw = generate_root_password();
        let sql = alter_user_sql(&pw);
        assert!(sql.contains(pw.expose()), "got {sql:?}");
    }

    #[test]
    fn alter_user_sql_doubles_an_embedded_single_quote() {
        // Impossible from the real generator (pure hex) — spec D3 says
        // write it defensively anyway, for the deferred user-chosen
        // password case.
        let pw = RootPassword::from_stored("ab'cd".to_string());
        let sql = alter_user_sql(&pw);
        assert!(sql.contains("ab''cd"), "expected a doubled quote: {sql:?}");
        assert!(
            !sql.contains("'ab'cd'"),
            "a single, undoubled quote would break out of the string literal: {sql:?}"
        );
    }

    // ---- write_generated_config ----

    #[test]
    fn write_generated_config_writes_the_file_and_creates_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("generated").join("mysql").join("my.cnf");
        let file = openvhost_conf::GeneratedFile {
            path: path.clone(),
            contents: "[mysqld]\ndatadir=/x\n".to_string(),
        };

        write_generated_config(&file).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), file.contents);
    }

    #[test]
    fn write_generated_config_is_atomic_no_temp_file_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("my.cnf");
        let file = openvhost_conf::GeneratedFile {
            path: path.clone(),
            contents: "[mysqld]\n".to_string(),
        };

        write_generated_config(&file).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "my.cnf")
            .collect();
        assert!(leftovers.is_empty(), "got {leftovers:?}");
    }

    // ---- staging_dir_path ----

    #[test]
    fn staging_dir_path_is_hidden_major_shaped_and_under_the_given_parent() {
        let parent = PathBuf::from("/tmp/ovh/data/mysql");
        let major = MysqlMajor::parse("8.4").unwrap();
        let path = staging_dir_path(&parent, &major);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".8.4.init-"), "got {name:?}");
        assert_eq!(path.parent(), Some(parent.as_path()));
    }

    #[test]
    fn two_staging_dir_paths_for_the_same_major_differ() {
        let parent = PathBuf::from("/tmp/ovh/data/mysql");
        let major = MysqlMajor::parse("8.4").unwrap();
        assert_ne!(
            staging_dir_path(&parent, &major),
            staging_dir_path(&parent, &major)
        );
    }

    #[test]
    fn staging_dir_path_is_recognized_by_the_stale_staging_sweep() {
        // Cross-check: whatever this function produces must be swept by
        // `sweep_stale_staging` if abandoned — both reuse the identical
        // shape check, so this must never drift.
        let tmp = tempfile::tempdir().unwrap();
        let major = MysqlMajor::parse("8.4").unwrap();
        let staging = staging_dir_path(tmp.path(), &major);
        std::fs::create_dir(&staging).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert_eq!(removed, vec![staging]);
    }

    // ---- remove_staging_dir ----

    #[test]
    fn remove_staging_dir_removes_a_staging_shaped_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".8.4.init-deadbeef");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("partial"), b"x").unwrap();

        remove_staging_dir(&staging).unwrap();

        assert!(!staging.exists());
    }

    #[test]
    fn remove_staging_dir_refuses_a_non_staging_shaped_path() {
        // Vacuity: point it at the FINAL datadir shape (no leading dot) and
        // confirm it survives untouched rather than silently no-opping.
        let tmp = tempfile::tempdir().unwrap();
        let final_dir = tmp.path().join("8.4");
        std::fs::create_dir(&final_dir).unwrap();

        let err = remove_staging_dir(&final_dir).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            final_dir.exists(),
            "a non-staging path must never be removed"
        );
    }

    #[test]
    fn remove_staging_dir_on_an_already_missing_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".8.4.init-neverexisted");
        remove_staging_dir(&staging).unwrap();
    }

    // ---- finalize_staging ----

    /// A staging directory carrying both sentinels — as if the command
    /// layer's spawn sequence (steps 1-5, spec D2) already succeeded.
    fn make_initialized_staging(parent: &Path) -> PathBuf {
        let major = MysqlMajor::parse("8.4").unwrap();
        let staging = staging_dir_path(parent, &major);
        std::fs::create_dir(&staging).unwrap();
        std::fs::create_dir(staging.join("mysql")).unwrap();
        std::fs::write(staging.join("auto.cnf"), b"[auto]\n").unwrap();
        staging
    }

    #[test]
    fn finalize_renames_staging_into_a_final_dir_that_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = make_initialized_staging(tmp.path());
        let final_dir = tmp.path().join("final");

        let outcome = finalize_staging(&staging, &final_dir);

        assert_eq!(outcome, MysqlInitOutcome::Initialized);
        assert!(!staging.exists());
        assert!(final_dir.join("mysql").is_dir());
        assert!(final_dir.join("auto.cnf").is_file());
    }

    #[test]
    fn finalize_renames_into_a_final_dir_that_already_exists_and_is_truly_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = make_initialized_staging(tmp.path());
        let final_dir = tmp.path().join("final");
        std::fs::create_dir(&final_dir).unwrap();

        let outcome = finalize_staging(&staging, &final_dir);

        assert_eq!(outcome, MysqlInitOutcome::Initialized);
        assert!(final_dir.join("mysql").is_dir());
    }

    #[test]
    fn finalize_deletes_ds_store_only_clutter_then_renames() {
        // The macOS clutter amendment: Finder touched the final dir before
        // init ever ran, leaving a `.DS_Store` that a plain `rename` would
        // refuse (ENOTEMPTY).
        let tmp = tempfile::tempdir().unwrap();
        let staging = make_initialized_staging(tmp.path());
        let final_dir = tmp.path().join("final");
        std::fs::create_dir(&final_dir).unwrap();
        std::fs::write(final_dir.join(".DS_Store"), b"clutter").unwrap();

        let outcome = finalize_staging(&staging, &final_dir);

        assert_eq!(outcome, MysqlInitOutcome::Initialized);
        assert!(!staging.exists());
        assert!(final_dir.join("mysql").is_dir());
        assert!(final_dir.join("auto.cnf").is_file());
        assert!(!final_dir.join(".DS_Store").exists());
    }

    #[test]
    fn finalize_refuses_and_deletes_nothing_when_final_dir_has_more_than_ds_store() {
        // "Only .DS_Store is ignorable — nothing broader": a genuine
        // stray file alongside it must block finalize entirely, and must
        // NOT be silently swept away together with the `.DS_Store`.
        let tmp = tempfile::tempdir().unwrap();
        let staging = make_initialized_staging(tmp.path());
        let final_dir = tmp.path().join("final");
        std::fs::create_dir(&final_dir).unwrap();
        std::fs::write(final_dir.join(".DS_Store"), b"clutter").unwrap();
        std::fs::write(final_dir.join("notes.txt"), b"do not touch").unwrap();

        let outcome = finalize_staging(&staging, &final_dir);

        match outcome {
            MysqlInitOutcome::Failed { step, .. } => assert_eq!(step, MysqlInitStep::Finalize),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            final_dir.join(".DS_Store").exists(),
            "must not delete .DS_Store when something else is present too"
        );
        assert!(final_dir.join("notes.txt").exists());
        assert!(
            staging.exists(),
            "staging must survive a failed finalize — cleanup is the caller's separate job"
        );
    }

    #[test]
    fn finalize_fails_when_staging_is_missing_sentinels() {
        let tmp = tempfile::tempdir().unwrap();
        let major = MysqlMajor::parse("8.4").unwrap();
        let staging = staging_dir_path(tmp.path(), &major);
        std::fs::create_dir(&staging).unwrap(); // empty: no sentinels
        let final_dir = tmp.path().join("final");

        let outcome = finalize_staging(&staging, &final_dir);

        match outcome {
            MysqlInitOutcome::Failed { step, .. } => assert_eq!(step, MysqlInitStep::Finalize),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            !final_dir.exists(),
            "must never create the final dir on a failed finalize"
        );
        assert!(
            staging.exists(),
            "must not delete staging itself; that is the caller's cleanup job"
        );
    }

    #[cfg(unix)]
    #[test]
    fn finalize_refuses_a_symlinked_final_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = make_initialized_staging(tmp.path());
        let real_target = tmp.path().join("elsewhere");
        std::fs::create_dir(&real_target).unwrap();
        let final_dir = tmp.path().join("final");
        std::os::unix::fs::symlink(&real_target, &final_dir).unwrap();

        let outcome = finalize_staging(&staging, &final_dir);

        match outcome {
            MysqlInitOutcome::Failed { step, .. } => assert_eq!(step, MysqlInitStep::Finalize),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(staging.exists());
    }
}
