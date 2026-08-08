// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-pkg — download → SHA-256 verify → extract → install pipeline.
//!
//! Responsibility (master plan §3.1): fetch a pinned (url, sha256) archive,
//! verify BEFORE parsing, extract through hardened manual walks, install
//! atomically to packages/<name>/<major>/<version>/ with a per-major
//! `current` link. The signed-manifest layer is a separate future slice that
//! produces `InstallRequest`s for this API. Security invariants: see
//! docs/superpowers/specs/2026-07-22-p06-pkg-pipeline-design.md §5.

mod download;
mod error;
mod extract;
mod install;
mod layout;
mod platform;
mod request;
#[cfg(test)]
mod testkit;

pub use error::PkgError;
pub use install::install_package;
/// The removal half of `current`-link maintenance, under the name a caller
/// outside this crate reads it by. The installer's `update_current` stays
/// private — it is only ever called by [`install_package`] — but an uninstall
/// lives elsewhere and still must not spell a per-OS link removal by hand.
pub use layout::remove_current as remove_current_link;
pub use request::{ArchiveFormat, InstallRequest, InstalledPackage, PackagesRoot, Progress};
