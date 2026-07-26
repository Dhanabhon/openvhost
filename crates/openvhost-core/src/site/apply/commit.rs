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

/// Write via a temp file in the SAME directory, then rename: a rename is
/// atomic only within one filesystem, which is the whole reason the temp file
/// cannot go in `/tmp` — a target under the user's home may live on a
/// different filesystem than the system temp directory. The temp name does
/// not end in `.conf` so `plan()`'s owned-file scan never mistakes a
/// leftover for a real site config.
fn atomic_write(path: &Path, contents: &str) -> Result<(), ApplyError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| ApplyError::Io {
        op: "create_dir_all",
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, contents).map_err(|source| ApplyError::Io {
        op: "write",
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| {
        // Best-effort: the rename error is the one worth propagating, and if
        // this cleanup also fails there is nothing useful left to report.
        let _ = std::fs::remove_file(&tmp);
        ApplyError::Io {
            op: "rename",
            path: path.to_path_buf(),
            source,
        }
    })
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
            ChangeKind::Modified | ChangeKind::Removed => {
                atomic_write(&c.path, c.before.as_deref().unwrap_or_default())
            }
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
