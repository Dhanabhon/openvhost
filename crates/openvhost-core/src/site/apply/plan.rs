// SPDX-License-Identifier: GPL-3.0-or-later
//! Diff the desired config set against what is on disk. Read-only: this is
//! what the pending-changes banner calls, so it must not spawn anything.

use std::path::{Path, PathBuf};

use openvhost_conf::GeneratedFile;

use super::{ApplyError, ApplyInput, render_set};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
    /// The contents currently on disk. `None` only for `Added`.
    pub before: Option<String>,
    /// The contents to be written. `None` only for `Removed`.
    pub after: Option<String>,
    /// Unified diff, rendered once here so the CLI and the UI cannot disagree.
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPlan {
    pub gen_root: PathBuf,
    pub main_conf: PathBuf,
    /// Sorted by path. EMPTY means the disk already matches the sites — that
    /// is exactly the condition the banner hides on.
    pub changes: Vec<FileChange>,
}

fn read_if_exists(path: &Path) -> Result<Option<String>, ApplyError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ApplyError::Io {
            op: "read",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn unified(path: &Path, before: &str, after: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .header(
            &format!("a/{}", path.display()),
            &format!("b/{}", path.display()),
        )
        .to_string()
}

/// Generated files this pipeline owns, and therefore may delete: the site
/// configs and the per-major pools. `config/custom/` is never listed, so it can
/// never be planned for removal.
fn owned_files(gen_root: &Path) -> Result<Vec<PathBuf>, ApplyError> {
    let mut out = Vec::new();
    let sites_dir = gen_root.join("nginx/sites");
    for entry in read_dir_or_empty(&sites_dir)? {
        if entry.extension().is_some_and(|e| e == "conf") {
            out.push(entry);
        }
    }
    let php_dir = gen_root.join("php");
    for major_dir in read_dir_or_empty(&php_dir)? {
        let pool = major_dir.join("php-fpm.conf");
        if pool.is_file() {
            out.push(pool);
        }
    }
    out.sort();
    Ok(out)
}

/// Directory entries, or an empty list when the directory does not exist —
/// a home that has never been applied is not an error condition.
fn read_dir_or_empty(dir: &Path) -> Result<Vec<PathBuf>, ApplyError> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ApplyError::Io {
                op: "read_dir",
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|source| ApplyError::Io {
            op: "read_dir",
            path: dir.to_path_buf(),
            source,
        })?;
        out.push(e.path());
    }
    Ok(out)
}

pub fn plan(input: &ApplyInput) -> Result<ApplyPlan, ApplyError> {
    let desired: Vec<GeneratedFile> = render_set(input)?;
    let gen_root = input.home.join("config/generated");
    let mut changes = Vec::new();

    for f in &desired {
        let before = read_if_exists(&f.path)?;
        match &before {
            Some(b) if *b == f.contents => continue,
            Some(b) => changes.push(FileChange {
                diff: unified(&f.path, b, &f.contents),
                path: f.path.clone(),
                kind: ChangeKind::Modified,
                before: before.clone(),
                after: Some(f.contents.clone()),
            }),
            None => changes.push(FileChange {
                diff: unified(&f.path, "", &f.contents),
                path: f.path.clone(),
                kind: ChangeKind::Added,
                before: None,
                after: Some(f.contents.clone()),
            }),
        }
    }

    let desired_paths: std::collections::BTreeSet<&Path> =
        desired.iter().map(|f| f.path.as_path()).collect();
    for stale in owned_files(&gen_root)? {
        if desired_paths.contains(stale.as_path()) {
            continue;
        }
        let before = read_if_exists(&stale)?;
        let before_text = before.clone().unwrap_or_default();
        changes.push(FileChange {
            diff: unified(&stale, &before_text, ""),
            path: stale,
            kind: ChangeKind::Removed,
            before,
            after: None,
        });
    }

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ApplyPlan {
        main_conf: gen_root.join("nginx/nginx.conf"),
        gen_root,
        changes,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::apply::tests_support::{input_with_home, site};
    use crate::site::model::Docroot;

    /// Write a rendered set to disk the way a previous apply would have left it.
    /// Deliberately not `commit` — that is Task 5's unit under test, and planning
    /// only needs *a* tree on disk, not the production writer.
    fn materialize(input: &ApplyInput) {
        for f in render_set(input).unwrap() {
            std::fs::create_dir_all(f.path.parent().unwrap()).unwrap();
            std::fs::write(&f.path, &f.contents).unwrap();
        }
    }

    #[test]
    fn everything_is_added_against_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = plan(&i).unwrap();
        assert_eq!(p.changes.len(), 4);
        assert!(p.changes.iter().all(|c| c.kind == ChangeKind::Added));
        assert_eq!(
            p.main_conf,
            home.path().join("config/generated/nginx/nginx.conf")
        );
    }

    #[test]
    fn an_unchanged_tree_plans_nothing() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        materialize(&i);
        let second = plan(&i).unwrap();
        assert!(
            second.changes.is_empty(),
            "re-planning an applied tree must be a no-op"
        );
    }

    #[test]
    fn editing_a_site_shows_exactly_one_modified_file() {
        let home = tempfile::tempdir().unwrap();
        let before = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        materialize(&before);

        let mut moved = site("app", "app.localhost", "8.4", true);
        moved.docroot = Docroot::parse("/tmp/projects/moved").unwrap();
        let after = input_with_home(home.path(), vec![moved], &["8.4"]);

        let p = plan(&after).unwrap();
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].kind, ChangeKind::Modified);
        assert!(p.changes[0].path.ends_with("app.localhost.conf"));
        assert!(
            p.changes[0]
                .diff
                .contains("-    root \"/tmp/projects/app\";")
        );
        assert!(
            p.changes[0]
                .diff
                .contains("+    root \"/tmp/projects/moved\";")
        );
    }

    #[test]
    fn disabling_a_site_removes_its_file() {
        let home = tempfile::tempdir().unwrap();
        let on = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        materialize(&on);

        let off = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", false)],
            &["8.4"],
        );
        let p = plan(&off).unwrap();
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].kind, ChangeKind::Removed);
        assert!(p.changes[0].path.ends_with("app.localhost.conf"));
        assert!(p.changes[0].after.is_none());
        assert!(p.changes[0].before.is_some());
    }

    #[test]
    fn custom_config_is_invisible_to_planning() {
        let home = tempfile::tempdir().unwrap();
        let custom = home.path().join("config/custom/sites");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("mine.conf"), "# hand written\n").unwrap();

        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = plan(&i).unwrap();
        assert!(
            p.changes
                .iter()
                .all(|c| !c.path.starts_with(home.path().join("config/custom"))),
            "planning must never name a file under config/custom"
        );
    }

    #[test]
    fn a_stray_file_in_the_generated_tree_is_removed() {
        let home = tempfile::tempdir().unwrap();
        let sites_dir = home.path().join("config/generated/nginx/sites");
        std::fs::create_dir_all(&sites_dir).unwrap();
        std::fs::write(sites_dir.join("ghost.localhost.conf"), "# left over\n").unwrap();

        let i = input_with_home(home.path(), vec![], &["8.4"]);
        let p = plan(&i).unwrap();
        assert!(
            p.changes
                .iter()
                .any(|c| c.kind == ChangeKind::Removed && c.path.ends_with("ghost.localhost.conf"))
        );
    }
}
