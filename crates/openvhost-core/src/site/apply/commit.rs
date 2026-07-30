// SPDX-License-Identifier: GPL-3.0-or-later
//! Install a planned config set, validate the real files, and restore the
//! previous tree if the validator rejects them.

use std::path::{Path, PathBuf};

use super::{ApplyError, ApplyPlan, ChangeKind, RollbackReport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: usize,
    /// `nginx -t` writes to stderr even on success; kept so the UI can show it.
    pub validator_stderr: String,
}

#[async_trait::async_trait]
pub trait ConfigValidator: Send + Sync {
    async fn validate(
        &self,
        main_conf: &Path,
    ) -> Result<openvhost_conf::ValidationReport, ApplyError>;
}

/// The real validator. `-e <err_log>` is mandatory on every nginx invocation,
/// which `validate_live` handles.
pub struct NginxValidator {
    pub bin: PathBuf,
    pub err_log: PathBuf,
}

#[async_trait::async_trait]
impl ConfigValidator for NginxValidator {
    async fn validate(
        &self,
        main_conf: &Path,
    ) -> Result<openvhost_conf::ValidationReport, ApplyError> {
        Ok(openvhost_conf::validate_live(&self.bin, main_conf, &self.err_log).await?)
    }
}

/// Thin wrapper over the crate-shared hardened atomic write (see
/// `crate::atomicfile`), mapping its error type into `ApplyError::Io`. Kept
/// under the original name so the pre-planted-symlink regression test below
/// — the only remaining caller now that `atomic_write` calls
/// `crate::atomicfile::write_atomic` directly — compiles unchanged.
/// `#[cfg(test)]` because that is its only caller: without it, a plain
/// `cargo build`/`clippy` (which does not compile `#[cfg(test)]` code) sees
/// no non-test caller and flags this as dead code.
#[cfg(test)]
fn atomic_write_with_suffix(path: &Path, contents: &str, suffix: &str) -> Result<(), ApplyError> {
    Ok(crate::atomicfile::write_atomic_with_suffix(
        path, contents, suffix,
    )?)
}

/// Thin wrapper over `crate::atomicfile::write_atomic` — see
/// `atomic_write_with_suffix`.
fn atomic_write(path: &Path, contents: &str) -> Result<(), ApplyError> {
    Ok(crate::atomicfile::write_atomic(path, contents)?)
}

fn remove_if_exists(path: &Path) -> Result<(), ApplyError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ApplyError::Io {
            op: "remove",
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub fn commit(plan: &ApplyPlan) -> Result<(), ApplyError> {
    // Ahead of writing any pool config: see `ApplyPlan::php_fpm_log_dirs`'s
    // doc comment for why php-fpm cannot start at all without this.
    for dir in &plan.php_fpm_log_dirs {
        std::fs::create_dir_all(dir).map_err(|source| ApplyError::Io {
            op: "create_dir_all",
            path: dir.clone(),
            source,
        })?;
    }
    for c in &plan.changes {
        match c.kind {
            ChangeKind::Added | ChangeKind::Modified => {
                atomic_write(&c.path, c.after.as_deref().unwrap_or_default())?;
            }
            ChangeKind::Removed => remove_if_exists(&c.path)?,
        }
    }
    Ok(())
}

/// Undo a commit. Continues past a failure — restoring four files out of five
/// beats abandoning at the first error — and reports everything it could not
/// put back.
pub fn rollback(plan: &ApplyPlan) -> Result<(), RollbackReport> {
    let mut first_error: Option<ApplyError> = None;
    let mut stranded = Vec::new();
    for c in &plan.changes {
        let r = match c.kind {
            ChangeKind::Added => remove_if_exists(&c.path),
            // `before: None` here is NOT "no previous content" — the
            // FileChange doc comment on `before` says it is also `None` for
            // the scan-then-read race in `plan.rs`, where a `Removed` entry's
            // file genuinely disappeared between being observed and being
            // read. Falling back to `unwrap_or_default()` there would
            // atomic_write an EMPTY `.conf` into the generated tree — a file
            // that never existed, now visible to `owned_files()`, so the next
            // plan shows a spurious Removed change forever. The honest
            // action when there is no previous content to restore is to
            // ensure the path is absent, not to invent zero bytes for it.
            ChangeKind::Modified => atomic_write(&c.path, c.before.as_deref().unwrap_or_default()),
            ChangeKind::Removed => match c.before.as_deref() {
                Some(b) => atomic_write(&c.path, b),
                None => remove_if_exists(&c.path),
            },
        };
        if let Err(e) = r {
            stranded.push(c.path.clone());
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    match first_error {
        None => Ok(()),
        Some(first_error) => Err(RollbackReport {
            first_error,
            stranded,
        }),
    }
}

fn with_rollback(plan: &ApplyPlan, original: ApplyError) -> ApplyError {
    match rollback(plan) {
        Ok(()) => original,
        Err(report) => ApplyError::RollbackFailed {
            original: Box::new(original),
            rollback: Box::new(report.first_error),
            stranded: report.stranded,
        },
    }
}

/// Install, validate, and restore on rejection.
///
/// Writing before validating is safe because a running nginx holds its config
/// in memory: nothing on disk takes effect until the caller restarts it, which
/// it only does after this returns `Ok`. The payoff is that the validator sees
/// the exact files that will run.
pub async fn apply(
    plan: &ApplyPlan,
    validator: &dyn ConfigValidator,
) -> Result<ApplyOutcome, ApplyError> {
    if let Err(e) = commit(plan) {
        return Err(with_rollback(plan, e));
    }
    match validator.validate(&plan.main_conf).await {
        Ok(r) if r.ok => Ok(ApplyOutcome {
            applied: plan.changes.len(),
            validator_stderr: r.stderr,
        }),
        Ok(r) => Err(with_rollback(
            plan,
            ApplyError::ValidationFailed { stderr: r.stderr },
        )),
        Err(e) => Err(with_rollback(plan, e)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::apply::plan as make_plan;
    use crate::site::apply::tests_support::{input_with_home, site};
    use std::collections::BTreeMap;

    /// Every regular file under `root`, as path → contents. The whole point of
    /// the rollback test is a byte-for-byte comparison, so snapshot everything.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(s) = std::fs::read_to_string(&p) {
                    out.insert(p, s);
                }
            }
        }
        out
    }

    struct AlwaysRejects;
    #[async_trait::async_trait]
    impl ConfigValidator for AlwaysRejects {
        async fn validate(
            &self,
            _main: &Path,
        ) -> Result<openvhost_conf::ValidationReport, ApplyError> {
            Ok(openvhost_conf::ValidationReport {
                ok: false,
                stderr: "nginx: [emerg] simulated rejection".into(),
            })
        }
    }

    struct AlwaysAccepts;
    #[async_trait::async_trait]
    impl ConfigValidator for AlwaysAccepts {
        async fn validate(
            &self,
            _main: &Path,
        ) -> Result<openvhost_conf::ValidationReport, ApplyError> {
            Ok(openvhost_conf::ValidationReport {
                ok: true,
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn a_green_validation_leaves_the_new_config_in_place() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = make_plan(&i).unwrap();
        let out = apply(&p, &AlwaysAccepts).await.unwrap();
        assert_eq!(out.applied, 4);
        let conf = home
            .path()
            .join("config/generated/nginx/sites/app.localhost.conf");
        assert!(
            std::fs::read_to_string(conf)
                .unwrap()
                .contains("server_name app.localhost;")
        );
    }

    #[tokio::test]
    async fn a_rejected_config_restores_the_tree_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        let first = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        apply(&make_plan(&first).unwrap(), &AlwaysAccepts)
            .await
            .unwrap();
        let before = snapshot(home.path());

        // A second site, plus removing the first — exercises Added, Modified
        // and Removed in one rollback.
        let second = input_with_home(
            home.path(),
            vec![site("other", "other.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = make_plan(&second).unwrap();
        let err = apply(&p, &AlwaysRejects).await.unwrap_err();
        assert!(matches!(err, ApplyError::ValidationFailed { .. }));

        assert_eq!(
            snapshot(home.path()),
            before,
            "rollback must restore every byte"
        );
    }

    #[tokio::test]
    async fn the_validator_stderr_reaches_the_caller() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let err = apply(&make_plan(&i).unwrap(), &AlwaysRejects)
            .await
            .unwrap_err();
        match err {
            ApplyError::ValidationFailed { stderr } => {
                assert!(stderr.contains("simulated rejection"));
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    /// Reachable via the documented scan-then-read race in `plan.rs`: a
    /// `Removed` change whose file disappeared between being observed and
    /// being read carries `before: None`. Rolling that back must leave the
    /// path ABSENT — not write a zero-byte `.conf` there, which would make
    /// `owned_files()` see a file that never existed and show a spurious
    /// Removed change on every plan from then on.
    #[test]
    fn rolling_back_a_removed_change_with_no_before_content_leaves_the_path_absent() {
        use crate::site::apply::FileChange;

        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config/generated/nginx/sites/ghost.conf");
        // Deliberately not created on disk: this is the race case, where the
        // file was already gone by the time plan() tried to read it for the
        // diff.
        assert!(!path.exists());

        let plan = ApplyPlan {
            gen_root: home.path().join("config/generated"),
            main_conf: home.path().join("config/generated/nginx/nginx.conf"),
            php_fpm_log_dirs: vec![],
            changes: vec![FileChange {
                path: path.clone(),
                kind: ChangeKind::Removed,
                before: None,
                after: None,
                diff: String::new(),
            }],
        };

        rollback(&plan).unwrap();
        assert!(
            !path.exists(),
            "rollback of a Removed change with no previous content must not create an empty \
             file at {}",
            path.display()
        );
    }

    #[test]
    fn commit_leaves_no_temp_files_behind() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        commit(&make_plan(&i).unwrap()).unwrap();
        let sites = home.path().join("config/generated/nginx/sites");
        let leftovers: Vec<_> = std::fs::read_dir(&sites)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".conf"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files must not survive: {leftovers:?}"
        );
    }

    /// P1 live-log-viewer bug fix (Task 1): without this, every install
    /// breaks the moment php-fpm's `error_log` moves to a per-major
    /// directory — see `ApplyPlan::php_fpm_log_dirs`'s doc comment. No site
    /// is needed to reproduce it: the pool config is rendered for every
    /// INSTALLED runtime regardless of whether a site uses it yet
    /// (`render_set`), so an empty site list still exercises this.
    #[test]
    fn commit_creates_the_php_fpm_log_directory_before_anything_needs_it() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![], &["8.4"]);
        commit(&make_plan(&i).unwrap()).unwrap();
        let dir = crate::LogPaths::new(home.path())
            .php_fpm_error(&crate::PhpVersion::parse("8.4").unwrap())
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(dir.is_dir(), "{dir:?} must exist after commit()");
    }

    #[cfg(unix)]
    #[test]
    fn commit_replaces_a_file_by_rename_rather_than_writing_in_place() {
        use std::os::unix::fs::MetadataExt;

        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        commit(&make_plan(&i).unwrap()).unwrap();
        let target = home
            .path()
            .join("config/generated/nginx/sites/app.localhost.conf");
        let first_inode = std::fs::metadata(&target).unwrap().ino();

        // Change the site so the same path is rewritten with different content.
        let mut moved = site("app", "app.localhost", "8.4", true);
        moved.docroot = crate::site::model::Docroot::parse("/tmp/projects/moved").unwrap();
        let second = input_with_home(home.path(), vec![moved], &["8.4"]);
        commit(&make_plan(&second).unwrap()).unwrap();

        let second_inode = std::fs::metadata(&target).unwrap().ino();
        // A plain std::fs::write would truncate and refill the SAME inode, leaving a
        // window where a reader sees a half-written config. tmp+rename swaps in a new
        // inode instead, so the file is never observed partially written.
        assert_ne!(
            first_inode, second_inode,
            "the config was written in place, not swapped in by rename"
        );
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("/tmp/projects/moved")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_preplanted_symlink_at_the_temp_path_is_not_written_through() {
        // A5: pin a known suffix (production always uses a fresh random uuid;
        // see `atomic_write`) so this test can plant a symlink at the EXACT
        // temp path `atomic_write_with_suffix` will try to create, at the
        // exact spot an attacker who somehow guessed/raced the suffix would
        // plant one. `create_new` must refuse to follow it — proving the old
        // `std::fs::write`-based implementation's arbitrary-file-overwrite
        // primitive is gone.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.localhost.conf");
        let suffix = "deadbeef";
        let tmp_path = dir.path().join(".app.localhost.conf.deadbeef.tmp");

        let victim = dir.path().join("victim-outside-generated-tree");
        std::fs::write(&victim, "must not be overwritten\n").unwrap();
        std::os::unix::fs::symlink(&victim, &tmp_path).unwrap();

        let err = atomic_write_with_suffix(&target, "server {}\n", suffix).unwrap_err();
        assert!(
            matches!(err, ApplyError::Io { op: "create", .. }),
            "got {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "must not be overwritten\n",
            "the symlink's target must be completely untouched"
        );
        // The symlink itself must also survive unreplaced — `create_new`
        // refuses to touch the path at all, it does not swap it out.
        assert!(
            std::fs::symlink_metadata(&tmp_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_rename_does_not_leave_its_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app.localhost.conf");
        // A directory at the target makes rename fail after the temp write succeeded.
        std::fs::create_dir_all(target.join("occupied")).unwrap();

        let err = atomic_write(&target, "server {}\n").unwrap_err();
        assert!(
            matches!(err, ApplyError::Io { op: "rename", .. }),
            "got {err:?}"
        );

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp file survived a failed rename: {leftovers:?}"
        );
    }
}
