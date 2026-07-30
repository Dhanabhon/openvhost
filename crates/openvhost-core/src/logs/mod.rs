// SPDX-License-Identifier: GPL-3.0-or-later
//! Every on-disk log path OpenVHost derives, owned in one place (P1
//! live-log-viewer design, spec D1/D1a:
//! `docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md`).
//!
//! Before this module, `<home>/logs/nginx.error.log` (and friends) were
//! hardcoded at every call site that needed them, and every php-fpm major
//! pointed at one shared `logs/php-fpm.log` — a line in it could never be
//! attributed to a pool. [`paths::LogPaths`] is now the single source of
//! truth for every path under `<home>/logs`, on the core/desktop side of the
//! `openvhost-conf` dependency boundary (see that module's doc comment for
//! the one place this crate could not reach and why).
//!
//! Task 2 (the bounded log reader) lives alongside this as a sibling module.

mod paths;

pub use paths::LogPaths;
