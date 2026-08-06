// SPDX-License-Identifier: GPL-3.0-or-later
//! nginx: the compiled-in package pin and the wiring that installs it into
//! `<home>/packages/nginx/1.30/<version>/`. See
//! docs/superpowers/specs/2026-08-06-p2-nginx-recipe-design.md.
//!
//! **Off-Homebrew slice 4A — catalogue and install only.** This is the same
//! starting scope [`crate::mariadb`] had before its datadir/discover/init/
//! paths/repo modules joined it in later slices: installing a runtime is not
//! running one, and discovery, replacing Homebrew at runtime and any UI are
//! deliberately absent here (design §2, §12).
//!
//! [`crate::PackageTarget`] is reused rather than redefined, for the identical
//! reason [`crate::mariadb`] already gives: it answers "which OS/architecture
//! pair can a prebuilt package be published for", a question about packages
//! and not about nginx.
//!
//! [`Availability`] is **not** re-exported from this crate's root the way
//! [`crate::mariadb::Availability`] is — the two are deliberately duplicated
//! types with the same name (this module's own catalogue explains why), and
//! Rust has no way to flatten two distinctly-named-the-same items into one
//! module's namespace. It stays reachable at `crate::nginx::Availability`,
//! which is namespaced by construction.

mod package;

pub use package::{
    Availability, NGINX_PACKAGE_NAME, NGINX_PACKAGES, NGINX_SERIES, NGINX_WARMUP_BINARY,
    NginxPackage, NginxPackageInstall, install_nginx_package, nginx_package_for_host,
    nginx_package_for_target,
};
