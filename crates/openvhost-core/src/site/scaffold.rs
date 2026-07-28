// SPDX-License-Identifier: GPL-3.0-or-later
//! Turn a picked parent folder into a site's docroot and, later, a starter
//! page (spec: docs/superpowers/specs/2026-07-29-p1-site-scaffold-design.md).
//!
//! This file currently holds only the pure pieces: [`scaffold_path`] (the
//! parent+name join, re-validated as a `Docroot` before anything touches the
//! filesystem) and the [`ScaffoldOutcome`]/[`ScaffoldStep`] shapes that the
//! filesystem-touching `scaffold()` (a later slice) reports through. Kept
//! serde/specta-free by design — the app layer mirrors these as DTOs.

use crate::error::CoreError;
use crate::site::model::{Docroot, SiteName};

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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::model::{Docroot, SiteName};

    fn d(s: &str) -> Docroot {
        Docroot::parse(s).unwrap()
    }
    fn n(s: &str) -> SiteName {
        SiteName::parse(s).unwrap()
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
}
