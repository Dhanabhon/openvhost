// SPDX-License-Identifier: GPL-3.0-or-later
//! Every on-disk log path OpenVHost derives, owned in one place
//! ([`paths::LogPaths`]), plus the bounded, filtering log-window reader
//! that turns those paths into a small, memory-safe slice of a (possibly
//! huge) log file ([`read::read_window`]) (P1 live-log-viewer design, spec
//! D1/D1a for paths, D3/D4 for the reader:
//! `docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md`).
//!
//! Before [`paths::LogPaths`] existed, `<home>/logs/nginx.error.log` (and
//! friends) were hardcoded at every call site that needed them, and every
//! php-fpm major pointed at one shared `logs/php-fpm.log` — a line in it
//! could never be attributed to a pool. `LogPaths` is now the single
//! source of truth for every path under `<home>/logs`, on the core/desktop
//! side of the `openvhost-conf` dependency boundary (see that module's doc
//! comment for the one place this crate could not reach and why).
//!
//! [`read::read_window`] is this crate's only file-reading entry point for
//! logs: it never loads a whole file, and it applies filtering server-side
//! during the bounded scan rather than over whatever it returns — see that
//! module's doc comment for the full algorithm and its confinement
//! boundary. The IPC layer (not this crate) is responsible for turning a
//! caller-supplied request into a `LogPaths`-derived path before either of
//! these is ever called with it (spec D5).
//!
//! [`dirs::ensure_log_dir`] is the single function every REAL (persistent,
//! on the actual home) log-directory creation call site in this crate and
//! the desktop app goes through (spec D5: `0700`, explicitly, not merely
//! inherited from `<home>`) — the creation-side counterpart to `LogPaths`
//! owning every log-directory PATH. `openvhost-conf`'s own `validate()`
//! methods still create a directory independently, under a THROWAWAY
//! validation home for shape-checking only — see [`dirs`]'s doc comment and
//! that crate's module docs for why this function cannot reach them.

mod dirs;
mod paths;
mod read;

pub use dirs::ensure_log_dir;
pub use paths::LogPaths;
pub use read::{
    DEFAULT_LINE_BYTES, DEFAULT_PAYLOAD_BYTES, DEFAULT_ROWS, DEFAULT_SCAN_BYTES, LogCursor,
    LogLevel, LogLimits, LogQuery, LogReset, LogRow, LogWindow, classify_level, read_window,
};
