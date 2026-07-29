// SPDX-License-Identifier: GPL-3.0-or-later
//! Turn a picked parent folder into a site's docroot and a starter page
//! (spec: docs/superpowers/specs/2026-07-29-p1-site-scaffold-design.md).
//!
//! Holds [`scaffold_path`] (the parent+name join, re-validated as a
//! `Docroot` before anything touches the filesystem), the
//! [`ScaffoldOutcome`]/[`ScaffoldStep`] shapes that [`scaffold`] reports
//! through, and `scaffold` itself: creating the docroot directory and, unless
//! an `index.*` entry point already exists there, writing an escaped
//! placeholder `index.html`. Kept serde/specta-free by design — the app
//! layer mirrors `ScaffoldOutcome` as a DTO.

use crate::error::CoreError;
use crate::site::model::{Docroot, Domain, SiteName};

/// Pure join of the picked parent folder and the site name, re-validated as a
/// `Docroot` so the over-length case fails before anything is created.
pub fn scaffold_path(parent: &Docroot, name: &SiteName) -> Result<Docroot, CoreError> {
    let joined = format!(
        "{}/{}",
        parent.as_str().trim_end_matches('/'),
        name.as_str()
    );
    Docroot::parse(&joined)
}

/// The result of scaffolding a new site's docroot: creating the folder and,
/// unless one already exists, a placeholder `index.html`.
///
/// Deliberately not a `Result` — scaffolding runs *after* the site row has
/// already been persisted, and a filesystem problem here must never look
/// like the save itself failed. All three variants are non-error outcomes at
/// the type level; `Failed` still carries the reason for the UI to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaffoldOutcome {
    /// The docroot did not exist; it was created with a placeholder page.
    Created,
    /// The docroot already existed (or already had an `index.*` entry
    /// point), so nothing was written. `existing` names what was found,
    /// e.g. `"index.php"`.
    KeptExisting { existing: String },
    /// Scaffolding failed partway through. `step` is a stable discriminator
    /// for the UI (never parse English out of `reason`); `reason` is the
    /// underlying error message.
    Failed { step: ScaffoldStep, reason: String },
}

/// Which step of [`ScaffoldOutcome::Failed`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaffoldStep {
    /// Creating the docroot directory itself.
    CreateDir,
    /// Checking whether an `index.*` entry point already exists there.
    Inspect,
    /// Writing the placeholder `index.html`.
    WritePlaceholder,
}

const PLACEHOLDER_HTML: &str = include_str!("placeholder.html");

/// Create the docroot folder and starter page. Infallible by design: every
/// failure is data (`ScaffoldOutcome::Failed`), because the caller has already
/// persisted the site row and must not roll it back over a filesystem problem.
pub fn scaffold(docroot: &Docroot, name: &SiteName, domain: &Domain) -> ScaffoldOutcome {
    let dir = docroot.as_path();
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // lstat, no follow: a file or symlink squatting on the docroot is
            // refused rather than written into.
            match std::fs::symlink_metadata(dir) {
                Ok(md) if md.is_dir() => {}
                Ok(_) => {
                    return ScaffoldOutcome::Failed {
                        step: ScaffoldStep::CreateDir,
                        reason: format!("{} already exists and is not a folder", dir.display()),
                    };
                }
                Err(e) => {
                    return ScaffoldOutcome::Failed {
                        step: ScaffoldStep::CreateDir,
                        reason: format!("{}: {e}", dir.display()),
                    };
                }
            }
        }
        Err(e) => {
            return ScaffoldOutcome::Failed {
                step: ScaffoldStep::CreateDir,
                reason: format!("{}: {e}", dir.display()),
            };
        }
    }

    match existing_index(dir) {
        Ok(Some(existing)) => return ScaffoldOutcome::KeptExisting { existing },
        Ok(None) => {}
        Err(e) => {
            return ScaffoldOutcome::Failed {
                step: ScaffoldStep::Inspect,
                reason: format!("{}: {e}", dir.display()),
            };
        }
    }

    let html = render_placeholder(name, domain, docroot);
    match crate::atomicfile::write_atomic(&dir.join("index.html"), &html) {
        Ok(()) => ScaffoldOutcome::Created,
        Err(e) => ScaffoldOutcome::Failed {
            step: ScaffoldStep::WritePlaceholder,
            reason: format!("{}: {}", e.path.display(), e.source),
        },
    }
}

/// Blocks generation only for a real web entry point: file stem `index` AND
/// extension `html` / `htm` / `php` (both compared `eq_ignore_ascii_case`, so
/// `INDEX.HTML` / `Index.Php` still block). That is what nginx's template
/// actually serves (`index index.php index.html;`), so a non-web `index.*`
/// file (`.js`, `.ts`, `.css`, …) no longer suppresses the placeholder.
/// `.htm` stays blocking even though nginx never serves it: generating
/// `index.html` beside a user's `index.htm` would silently shadow it, which
/// is worse than the 404 it would get otherwise.
fn existing_index(dir: &std::path::Path) -> std::io::Result<Option<String>> {
    const WEB_INDEX_EXTENSIONS: [&str; 3] = ["html", "htm", "php"];

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }
        let fname = entry.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        let path = std::path::Path::new(fname);
        let is_web_entry_point = path
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("index"))
            && path.extension().is_some_and(|ext| {
                WEB_INDEX_EXTENSIONS
                    .iter()
                    .any(|web_ext| ext.eq_ignore_ascii_case(web_ext))
            });
        if is_web_entry_point {
            return Ok(Some(fname.to_string()));
        }
    }
    Ok(None)
}

/// Escaping lives HERE, unconditionally, for all three values — the newtypes
/// are charset guards, not encoders, and Docroot legally contains & < > '.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn render_placeholder(name: &SiteName, domain: &Domain, docroot: &Docroot) -> String {
    PLACEHOLDER_HTML
        .replace("{{name}}", &html_escape(name.as_str()))
        .replace("{{domain}}", &html_escape(domain.as_str()))
        .replace("{{docroot}}", &html_escape(docroot.as_str()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::model::{Docroot, Domain, SiteName};

    fn d(s: &str) -> Docroot {
        Docroot::parse(s).unwrap()
    }
    fn n(s: &str) -> SiteName {
        SiteName::parse(s).unwrap()
    }
    fn dom(s: &str) -> Domain {
        Domain::parse(s).unwrap()
    }

    #[test]
    fn scaffold_path_joins_parent_and_name() {
        assert_eq!(
            scaffold_path(&d("/Users/x/Downloads"), &n("my-site"))
                .unwrap()
                .as_str(),
            "/Users/x/Downloads/my-site"
        );
    }

    #[test]
    fn scaffold_path_normalizes_trailing_slash() {
        assert_eq!(
            scaffold_path(&d("/Users/x/Downloads/"), &n("my-site"))
                .unwrap()
                .as_str(),
            "/Users/x/Downloads/my-site"
        );
    }

    #[test]
    fn scaffold_path_handles_root_parent() {
        assert_eq!(scaffold_path(&d("/"), &n("a")).unwrap().as_str(), "/a");
    }

    #[test]
    fn scaffold_path_rejects_over_length_join() {
        // A parent that is itself valid but whose join with the name exceeds
        // DOCROOT_MAX_LEN must fail as a docroot validation error, before
        // anything touches the filesystem. model.rs's DOCROOT_MAX_LEN is
        // 1023 (private to that module, so duplicated here as a literal);
        // size the parent so parent + "/" + name is one byte over:
        // 1023 - 63 (name) - 1 (join slash) = 959 filler bytes.
        let parent = format!("/{}", "a".repeat(1023 - 63 - 1));
        let err = scaffold_path(&d(&parent), &n(&"b".repeat(63))).unwrap_err();
        // Same assertion style as site::repo's tests on CoreError::Validation.
        assert!(matches!(
            err,
            CoreError::Validation {
                field: "docroot",
                ..
            }
        ));
    }

    #[test]
    fn scaffold_creates_dir_and_placeholder() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        let name = n("my-site");
        let domain = dom("my-site.localhost");

        let outcome = scaffold(&docroot, &name, &domain);
        assert_eq!(outcome, ScaffoldOutcome::Created);

        assert!(docroot.as_path().is_dir());
        let contents = std::fs::read_to_string(docroot.as_path().join("index.html")).unwrap();
        assert!(
            contents.contains("my-site"),
            "missing site name: {contents}"
        );
        assert!(
            contents.contains("my-site.localhost"),
            "missing domain: {contents}"
        );
        assert!(
            contents.contains(docroot.as_str()),
            "missing docroot path: {contents}"
        );
    }

    #[test]
    fn scaffold_second_run_keeps_existing() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        let name = n("my-site");
        let domain = dom("my-site.localhost");

        assert_eq!(scaffold(&docroot, &name, &domain), ScaffoldOutcome::Created);

        let index = docroot.as_path().join("index.html");
        let before_contents = std::fs::read_to_string(&index).unwrap();
        let before_mtime = std::fs::metadata(&index).unwrap().modified().unwrap();

        let second = scaffold(&docroot, &name, &domain);
        assert_eq!(
            second,
            ScaffoldOutcome::KeptExisting {
                existing: "index.html".to_string()
            }
        );

        assert_eq!(
            std::fs::read_to_string(&index).unwrap(),
            before_contents,
            "content must be untouched by a second run"
        );
        assert_eq!(
            std::fs::metadata(&index).unwrap().modified().unwrap(),
            before_mtime,
            "mtime must be untouched by a second run"
        );
    }

    #[test]
    fn scaffold_keeps_existing_index_php() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        std::fs::create_dir(docroot.as_path()).unwrap();
        std::fs::write(docroot.as_path().join("index.php"), "<?php echo 'hi'; ?>").unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        assert_eq!(
            outcome,
            ScaffoldOutcome::KeptExisting {
                existing: "index.php".to_string()
            }
        );
        assert!(!docroot.as_path().join("index.html").exists());
    }

    #[test]
    fn scaffold_keeps_existing_uppercase_index() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        std::fs::create_dir(docroot.as_path()).unwrap();
        std::fs::write(docroot.as_path().join("INDEX.HTML"), "already here").unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        assert_eq!(
            outcome,
            ScaffoldOutcome::KeptExisting {
                existing: "INDEX.HTML".to_string()
            }
        );
    }

    #[test]
    fn scaffold_ignores_directory_named_index() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        std::fs::create_dir(docroot.as_path()).unwrap();
        std::fs::create_dir(docroot.as_path().join("index")).unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        assert_eq!(outcome, ScaffoldOutcome::Created);
        assert!(docroot.as_path().join("index.html").exists());
    }

    #[test]
    fn scaffold_generates_despite_non_web_index_files() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        std::fs::create_dir(docroot.as_path()).unwrap();
        std::fs::write(docroot.as_path().join("index.js"), "console.log('hi');").unwrap();
        std::fs::write(docroot.as_path().join("index.ts"), "console.log('hi');").unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        assert_eq!(outcome, ScaffoldOutcome::Created);
        assert!(docroot.as_path().join("index.html").exists());
    }

    #[test]
    fn scaffold_fails_when_parent_missing() {
        let parent_dir = tempfile::tempdir().unwrap();
        let missing = parent_dir.path().join("does-not-exist/my-site");
        let docroot = d(missing.to_str().unwrap());

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        match outcome {
            ScaffoldOutcome::Failed { step, .. } => assert_eq!(step, ScaffoldStep::CreateDir),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn scaffold_fails_when_target_is_a_file() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();
        std::fs::write(docroot.as_path(), "just a file").unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        match outcome {
            ScaffoldOutcome::Failed { step, reason } => {
                assert_eq!(step, ScaffoldStep::CreateDir);
                assert!(reason.contains("not a folder"), "reason was: {reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_fails_when_target_is_a_symlink() {
        let parent_dir = tempfile::tempdir().unwrap();
        let parent = d(parent_dir.path().to_str().unwrap());
        let docroot = scaffold_path(&parent, &n("my-site")).unwrap();

        let real_dir = parent_dir.path().join("real-dir");
        std::fs::create_dir(&real_dir).unwrap();
        std::os::unix::fs::symlink(&real_dir, docroot.as_path()).unwrap();

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        match outcome {
            ScaffoldOutcome::Failed { step, .. } => assert_eq!(step, ScaffoldStep::CreateDir),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn placeholder_html_escapes_interpolations() {
        let parent_dir = tempfile::tempdir().unwrap();
        let docroot = d(parent_dir.path().join("<b>&'x").to_str().unwrap());

        let outcome = scaffold(&docroot, &n("my-site"), &dom("my-site.localhost"));
        assert_eq!(outcome, ScaffoldOutcome::Created);

        let html = std::fs::read_to_string(docroot.as_path().join("index.html")).unwrap();
        assert!(
            html.contains("&lt;b&gt;&amp;&#39;x"),
            "escaped docroot missing: {html}"
        );
        assert!(
            !html.contains("<b>"),
            "raw unescaped docroot leaked: {html}"
        );
    }
}
