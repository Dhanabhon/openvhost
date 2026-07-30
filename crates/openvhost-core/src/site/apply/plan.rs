// SPDX-License-Identifier: GPL-3.0-or-later
//! Diff the desired config set against what is on disk. Read-only: this is
//! what the pending-changes banner calls, so it must not spawn anything.

use std::path::{Path, PathBuf};

use openvhost_conf::GeneratedFile;

use crate::{LogPaths, PhpVersion};

use super::{ApplyError, ApplyInput, PhpRuntime, render_set};

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
    /// The contents currently on disk. `None` for `Added`, and also for the
    /// narrow race where an owned file is observed during the scan but has
    /// disappeared by the time it is read for the `Removed` diff.
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
    /// The php-fpm per-major log directory for every INSTALLED PHP runtime
    /// (`render_set` renders a pool config for each one regardless of
    /// whether any site currently uses it), so `commit()` can create it
    /// before that config ever points a php-fpm process at a file inside it.
    ///
    /// A narrow, targeted fix, not the general mechanism: this crate's P1
    /// live-log-viewer bug fix moved php-fpm's `error_log` from a flat
    /// `<home>/logs/php-fpm.log` (whose parent, `logs/`, `provision_home`
    /// already creates) to `<home>/logs/services/php-fpm-<major>/error.log`
    /// — one directory LEVEL DEEPER, per major, which nothing else yet
    /// creates. Without this, every existing install breaks the moment that
    /// fix ships: php-fpm refuses to start at all ("failed to open
    /// error_log ... No such file or directory"). The P1 design (spec D2)
    /// plans a fuller `log_dirs` mechanism covering the NEW per-site
    /// directories too (`logs/sites/<domain>/`, `0700`, `provision_home`
    /// seeding) — this field covers only what THIS fix requires, computed
    /// the same read-only way `plan()` computes everything else, and is
    /// expected to be folded into that fuller mechanism when it lands.
    pub php_fpm_log_dirs: Vec<PathBuf>,
    /// Sorted by path. EMPTY means the disk already matches the sites — that
    /// is exactly the condition the banner hides on.
    pub changes: Vec<FileChange>,
}

/// See [`ApplyPlan::php_fpm_log_dirs`]. Pure path arithmetic — no I/O — so
/// `plan()` stays read-only. `PhpVersion::parse` is not expected to fail for
/// an already-probed, already-installed runtime major; if it somehow does,
/// this fails the whole plan rather than silently dropping that major's
/// directory (and therefore its ability to start).
fn php_fpm_log_dirs(home: &Path, php: &[PhpRuntime]) -> Result<Vec<PathBuf>, ApplyError> {
    let paths = LogPaths::new(home);
    php.iter()
        .map(|rt| {
            let major = PhpVersion::parse(&rt.major)?;
            let error_log = paths.php_fpm_error(&major);
            error_log.parent().map(Path::to_path_buf).ok_or_else(|| {
                // Structurally unreachable — `php_fpm_error` always nests at
                // least `services/php-fpm-<major>/error.log` below `root`,
                // so it always has a parent — but an honest error beats
                // `unwrap`/`expect` for a case the compiler cannot itself
                // rule out.
                ApplyError::Io {
                    op: "parent",
                    path: error_log.clone(),
                    source: std::io::Error::other(
                        "LogPaths::php_fpm_error returned a path with no parent directory",
                    ),
                }
            })
        })
        .collect()
}

/// Read a generated-config path if it exists, refusing anything that is not a
/// plain file rather than following or ignoring it.
///
/// The entry-type check lives here — behind `symlink_metadata`, which does not
/// follow a symlink — rather than being duplicated at each call site, so that
/// both the "desired" loop (Added/Modified) and the stale-scan "Removed" loop
/// in `plan()` are covered by construction instead of by two checks that can
/// drift apart. For a *desired* path this error is the honest outcome: apply
/// genuinely cannot put a config file where a directory or symlink already
/// sits, and following the symlink would leak its target's contents into the
/// diff shown to the user. For the stale-scan path, `owned_files()` already
/// excludes non-regular entries before they ever reach this function, so a
/// stray directory or symlink there is filtered out earlier and never trips
/// this error — that filter must stay in place; don't weaken it to "fix" this.
fn read_if_exists(path: &Path) -> Result<Option<String>, ApplyError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ApplyError::Io {
                op: "read",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let file_type = meta.file_type();
    if !file_type.is_file() {
        let found = if file_type.is_dir() {
            "a directory"
        } else if file_type.is_symlink() {
            "a symlink"
        } else {
            "a special file"
        };
        return Err(ApplyError::NotAPlainFile {
            path: path.to_path_buf(),
            expected: "a plain file",
            found,
        });
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|source| ApplyError::Io {
            op: "read",
            path: path.to_path_buf(),
            source,
        })
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
///
/// Ownership is decided by the entry's own type — reported by `read_dir`
/// without following the entry — never by `Path::is_file`/`metadata`, which
/// resolve symlinks. A symlink under the generated tree is not something this
/// pipeline wrote, so even one pointing at a real `.conf` file must not be
/// treated as owned: following it would let the plan read (and the apply step
/// delete) a file outside the confinement boundary the generated tree is
/// meant to enforce. A directory that merely has a matching name is likewise
/// excluded so a stray entry can never turn into a propagated I/O error.
fn owned_files(gen_root: &Path) -> Result<Vec<PathBuf>, ApplyError> {
    let mut out = Vec::new();
    let sites_dir = gen_root.join("nginx/sites");
    for (path, file_type) in read_dir_or_empty(&sites_dir)? {
        if file_type.is_file() && path.extension().is_some_and(|e| e == "conf") {
            out.push(path);
        }
    }
    let php_dir = gen_root.join("php");
    for (major_dir, major_type) in read_dir_or_empty(&php_dir)? {
        if !major_type.is_dir() {
            continue;
        }
        let pool = major_dir.join("php-fpm.conf");
        // Reached by path rather than by `DirEntry`, so use `symlink_metadata`
        // (not `Path::is_file`, which follows symlinks) to keep the same
        // regular-files-only rule for the file inside the major directory.
        let is_regular_file = std::fs::symlink_metadata(&pool)
            .map(|m| m.is_file())
            .unwrap_or(false);
        if is_regular_file {
            out.push(pool);
        }
    }
    out.sort();
    Ok(out)
}

/// Directory entries paired with their own file type (not the type of
/// whatever a symlink might point at), or an empty list when the directory
/// does not exist — a home that has never been applied is not an error
/// condition.
///
/// Refuses to scan anything that is not a REAL directory (A4), checked with
/// `symlink_metadata` — which does not follow — BEFORE `read_dir` is ever
/// called. `read_dir` itself silently follows a symlinked directory, so
/// without this gate a `sites`/`php` scan root replaced by a symlink would
/// have every `.conf` file inside the symlink's TARGET classified `Removed`
/// and deleted, and the newly generated set written into that target
/// instead — outside the confinement the generated tree exists to enforce.
/// A missing root is still not an error: it means nothing has been applied
/// here yet, so `owned_files()` reports nothing owned.
fn read_dir_or_empty(dir: &Path) -> Result<Vec<(PathBuf, std::fs::FileType)>, ApplyError> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(meta) => {
            let file_type = meta.file_type();
            let found = if file_type.is_symlink() {
                "a symlink"
            } else if file_type.is_file() {
                "a file"
            } else {
                "a special file"
            };
            return Err(ApplyError::NotAPlainFile {
                path: dir.to_path_buf(),
                expected: "a plain directory",
                found,
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ApplyError::Io {
                op: "read_dir",
                path: dir.to_path_buf(),
                source,
            });
        }
    }

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
        let file_type = e.file_type().map_err(|source| ApplyError::Io {
            op: "read_dir",
            path: e.path(),
            source,
        })?;
        out.push((e.path(), file_type));
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
        php_fpm_log_dirs: php_fpm_log_dirs(&input.home, &input.runtimes.php)?,
        changes,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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

    #[cfg(unix)]
    #[test]
    fn a_stray_directory_or_symlink_does_not_break_planning() {
        let home = tempfile::tempdir().unwrap();
        let sites_dir = home.path().join("config/generated/nginx/sites");
        std::fs::create_dir_all(&sites_dir).unwrap();

        // A directory that merely looks like a config file. Reading it as a string
        // fails with a non-NotFound error, which must not take the whole plan down:
        // the pending-changes banner would stop working entirely.
        std::fs::create_dir_all(sites_dir.join("ghost.conf")).unwrap();

        // A symlink is not something this pipeline wrote, so it is neither owned
        // nor read. Pointing it outside the generated tree makes the boundary
        // violation visible if the filter ever regresses.
        let outside = home.path().join("secret.txt");
        std::fs::write(&outside, "not for the diff view\n").unwrap();
        std::os::unix::fs::symlink(&outside, sites_dir.join("linked.conf")).unwrap();

        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = plan(&i).expect("a stray entry must not fail the plan");

        assert!(
            !p.changes.iter().any(|c| c.path.ends_with("ghost.conf")),
            "a directory is not an owned config file"
        );
        assert!(
            !p.changes.iter().any(|c| c.path.ends_with("linked.conf")),
            "a symlink is not an owned config file"
        );
        assert!(
            !p.changes
                .iter()
                .any(|c| c.diff.contains("not for the diff view")),
            "no symlink target's contents may reach the diff shown to the user"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_a_desired_path_is_refused_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let sites_dir = home.path().join("config/generated/nginx/sites");
        std::fs::create_dir_all(&sites_dir).unwrap();

        let outside = home.path().join("secret.txt");
        std::fs::write(&outside, "not for the diff view\n").unwrap();
        // Exactly the path render_set wants to write for this site.
        std::os::unix::fs::symlink(&outside, sites_dir.join("app.localhost.conf")).unwrap();

        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        match plan(&i) {
            Err(ApplyError::NotAPlainFile { path, .. }) => {
                assert!(path.ends_with("app.localhost.conf"));
            }
            Err(other) => panic!("expected NotAPlainFile, got {other:?}"),
            Ok(p) => panic!(
                "planning followed the symlink instead of refusing it: {:?}",
                p.changes.iter().map(|c| &c.diff).collect::<Vec<_>>()
            ),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_scan_root_is_refused_not_followed() {
        // A4: if `config/generated/nginx/sites` is itself a symlink, `read_dir`
        // would silently follow it and classify every `.conf` file in the
        // TARGET directory as `Removed` — planning it for deletion — which is
        // exactly the confinement break this test exists to catch.
        let home = tempfile::tempdir().unwrap();
        let gen_root = home.path().join("config/generated");
        std::fs::create_dir_all(gen_root.join("nginx")).unwrap();

        // The symlink target: a directory OUTSIDE the generated tree holding a
        // `.conf` file that must never be touched by planning.
        let outside = home.path().join("outside-sites");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("victim.conf"), "# not ours\n").unwrap();

        std::os::unix::fs::symlink(&outside, gen_root.join("nginx/sites")).unwrap();

        let i = input_with_home(home.path(), vec![], &["8.4"]);
        match plan(&i) {
            Err(ApplyError::NotAPlainFile {
                path,
                expected,
                found,
            }) => {
                assert!(path.ends_with("nginx/sites"), "got {path:?}");
                assert_eq!(expected, "a plain directory");
                assert_eq!(found, "a symlink");
            }
            Err(other) => panic!("expected NotAPlainFile, got {other:?}"),
            Ok(p) => panic!(
                "planning followed the symlinked scan root instead of refusing it: {:?}",
                p.changes.iter().map(|c| &c.path).collect::<Vec<_>>()
            ),
        }
        // The file outside the generated tree must be completely untouched.
        assert!(outside.join("victim.conf").exists());
    }

    /// P1 live-log-viewer bug fix (Task 1): moving php-fpm's `error_log` to a
    /// per-major directory means `plan()` must now also report that
    /// directory per installed major — see `ApplyPlan::php_fpm_log_dirs`'s
    /// doc comment for why. `plan()` must stay read-only, so neither
    /// directory may exist on disk merely from calling it.
    #[test]
    fn php_fpm_log_dirs_cover_every_installed_major_and_touch_no_disk() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.3", "8.4"],
        );
        let p = plan(&i).unwrap();

        let paths = crate::LogPaths::new(home.path());
        let expected_dir = |major: &str| {
            paths
                .php_fpm_error(&crate::PhpVersion::parse(major).unwrap())
                .parent()
                .unwrap()
                .to_path_buf()
        };
        let mut want = vec![expected_dir("8.3"), expected_dir("8.4")];
        want.sort();
        let mut got = p.php_fpm_log_dirs.clone();
        got.sort();
        assert_eq!(got, want);

        for dir in &p.php_fpm_log_dirs {
            assert!(!dir.exists(), "{dir:?} must not be created by plan() alone");
        }
    }

    #[test]
    fn a_directory_at_a_desired_path_is_reported_clearly() {
        let home = tempfile::tempdir().unwrap();
        let sites_dir = home.path().join("config/generated/nginx/sites");
        // A directory where a config file must go: apply cannot succeed, so this
        // must say so rather than surfacing a raw "Is a directory" io error.
        std::fs::create_dir_all(sites_dir.join("app.localhost.conf")).unwrap();

        let i = input_with_home(
            home.path(),
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        );
        let err = plan(&i).unwrap_err();
        assert!(
            matches!(&err, ApplyError::NotAPlainFile { found, .. } if *found == "a directory"),
            "got {err:?}"
        );
        assert!(err.to_string().contains("app.localhost.conf"));
    }
}
