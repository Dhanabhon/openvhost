// SPDX-License-Identifier: GPL-3.0-or-later
//! The bounded, filtering log-window reader (P1 live-log-viewer design,
//! spec D3/D4:
//! `docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md`).
//!
//! [`read_window`] is the ONLY way anything in this codebase turns a log
//! path into text: it never loads a whole file (proven in this module's own
//! tests against a fixture well over the scan bound, not merely intended),
//! and it applies [`LogQuery`] server-side, DURING the scan, rather than as
//! a post-filter over whatever window it happens to return — the cursor
//! advances across non-matching lines exactly as it does across matching
//! ones, so a match older than a plain tail's visible window is still
//! findable within [`LogLimits::scan`] bytes (spec D4; this is the entire
//! reason this design beats filtering client-side over the loaded window).
//!
//! # Algorithm
//!
//! - `cursor: None` seeks back from EOF — by [`LogLimits::scan`] bytes when
//!   [`LogQuery`] is active (a needle or a level floor), so a match older
//!   than a plain tail can still be found, or by [`LogLimits::payload`]
//!   bytes otherwise, which comfortably holds the last [`LogLimits::rows`]
//!   ordinary lines without paying for a much larger scan nothing needs —
//!   and discards whatever partial line fragment that landed inside (the
//!   Docroot-lesson-adjacent simplification: after an arbitrary seek there
//!   is no reliable way to tell alignment without reading backward, so the
//!   first record found is always treated as unreliable, even on the rare
//!   luck-of-alignment call where it happened to be a genuine whole line).
//! - A cursor whose file identity no longer matches the file at `path`
//!   (different device/inode — e.g. logrotate's rename-and-recreate), or
//!   whose recorded offset now exceeds the file's current length (e.g. `: >
//!   file.log`), restarts the same way a fresh `None` cursor would and
//!   reports [`LogReset::Rotated`] / [`LogReset::Truncated`] respectively.
//! - Otherwise this resumes forward from the cursor's exact byte offset —
//!   never re-scanning, never skipping.
//! - A trailing line with no `\n` yet is neither returned nor counted, and
//!   the returned cursor's offset never advances past the end of the last
//!   COMPLETE line, so the very next call sees the whole line once the
//!   newline lands (proven by a dedicated test: nothing here special-cases
//!   "wait for more data," the cursor bookkeeping just naturally does not
//!   move past an unterminated fragment).
//! - [`LogQuery`]'s predicate runs on every complete line the scan visits,
//!   not only on what ends up in [`LogWindow::rows`] — a non-match still
//!   commits the cursor past it (spec D4's "cursor advances across
//!   non-matches").
//! - The scan can never read more than [`LogLimits::scan`] bytes from disk,
//!   via [`std::io::Read::take`] wrapping the file handle itself — the
//!   bound is a property of the I/O source, not a tally this loop could get
//!   wrong.
//!
//! # Confinement
//!
//! This module trusts `path` completely: it refuses anything at that path
//! that is not a plain file (`symlink_metadata`, never `canonicalize` — the
//! same discipline as `site::apply::plan`'s `read_if_exists`/
//! `read_dir_or_empty`), and a missing file is `exists: false`, not an
//! error. What it deliberately does NOT do is decide whether `path` was
//! safe to derive in the first place, or assert it against a confinement
//! root — that is the IPC layer's job (spec D5: typed source enum →
//! newtype ingress → live-catalogue check → [`crate::LogPaths`] derivation
//! → a `starts_with(<home>/logs)` post-condition), deliberately kept out of
//! this pure reader so the same function works identically for any log
//! path a future caller (the CLI, a test) already trusts.
//!
//! # The ring-classifier seam
//!
//! [`classify_level`] is the ONE classifier for FILE lines (spec D4).
//! `openvhost_proc`'s supervisor keeps its OWN separate classifier
//! (`openvhost_proc::log::classify_level`, private to that crate) for ring
//! lines — a different input shape (it also knows which stream, stdout or
//! stderr, a line came from) that was already shipped before this slice.
//! The two are deliberately NOT unified; see that function's doc comment
//! for the reverse cross-reference spec D4 asks for.
//!
//! [`LogLevel`] itself IS shared (re-exported here from `openvhost_proc`,
//! which already depends on nothing this crate cannot also depend on) —
//! not duplicated — precisely so the UI's row renderer (spec D6) has ONE
//! severity type to color for both the ring-backed `LogPane` and this
//! reader's rows, and "level colours cannot drift between the two
//! surfaces" is a type-level fact instead of two enums a mapping function
//! could quietly desync.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

// `pub use`, not a private `use`: `LogLevel` is part of this module's public
// interface (see the module doc's "ring-classifier seam" section) and
// `logs::mod` re-exports it from here.
pub use openvhost_proc::LogLevel;

use crate::error::CoreError;

/// Default row cap (spec D3). Safe precisely because the UI has no
/// virtualization (spec D6) — the rendered set is bounded by construction,
/// so a fixed cap is enough on its own, no paging required for THIS slice.
pub const DEFAULT_ROWS: usize = 500;
/// Default cumulative cap, in bytes, on the text actually returned in
/// [`LogWindow::rows`] (spec D3).
pub const DEFAULT_PAYLOAD_BYTES: u64 = 512 * 1024;
/// Default per-line cap, in bytes, before truncation (spec D3).
pub const DEFAULT_LINE_BYTES: u64 = 16 * 1024;
/// Default cumulative cap, in bytes, on how much of the file ONE call may
/// read from disk (spec D3) — the number that makes "the reader never
/// loads a whole file" true, enforced physically via [`std::io::Read::take`]
/// rather than by a bookkeeping check this loop could get wrong.
pub const DEFAULT_SCAN_BYTES: u64 = 16 * 1024 * 1024;

/// A file's on-disk identity, used only to detect that the file now at a
/// given path is not the same file a [`LogCursor`] was issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn of(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }

    /// Non-unix fallback (Windows support is deferred — project scope memo,
    /// master plan §7): every file reports the SAME identity here, so a
    /// same-or-larger-sized replacement file is not detected as a rotation
    /// on that platform — only a length shrink is, via the separate
    /// `len < cursor.offset` check in [`read_window`]. Stated, not papered
    /// over, mirroring this crate's existing `#[cfg(unix)]` /
    /// `#[cfg(not(unix))]` pairs (e.g. `db::mod::harden_state_db_permissions`).
    #[cfg(not(unix))]
    fn of(_meta: &std::fs::Metadata) -> Self {
        Self { dev: 0, ino: 0 }
    }
}

/// An opaque resume point for [`read_window`]: which file (by identity, not
/// path) and how far into it. Round-trips through IPC as a plain
/// serializable value (its fields stay private, so nothing outside this
/// module can construct or mutate one — the only way to get a `LogCursor`
/// is to have already called [`read_window`] once).
///
/// Reusing a cursor issued for one file against a DIFFERENT file at the
/// same `path` is safe, not a confinement concern: the identity mismatch
/// is simply read as [`LogReset::Rotated`] and this restarts from a fresh
/// tail of whatever is actually at `path` now — the cursor can only ever
/// change WHERE within an already-trusted `path` a scan resumes, never
/// WHICH path gets opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogCursor {
    identity: FileIdentity,
    offset: u64,
}

/// A server-side filter applied DURING the bounded scan (spec D4), not as a
/// post-filter over whatever [`read_window`] happens to return.
///
/// Changing the query for an ALREADY-ISSUED cursor continues the scan
/// forward from that cursor's offset — it does not re-search history from a
/// fresh tail. A caller that wants a full, filter-aware search after the
/// user edits the filter text should pass `cursor: None` again, the same as
/// a first load.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogQuery {
    /// Case-insensitive literal substring by default (`case_sensitive`
    /// toggles that); `None` matches every line. No regex (spec D4): the
    /// pattern comes straight from the renderer, and a backtracking regex
    /// engine given attacker-shaped input is a UI-freezing hazard this
    /// crate does not take on for what is, today, a literal-search feature.
    pub needle: Option<String>,
    /// Only consulted when `needle` is `Some`.
    pub case_sensitive: bool,
    /// Keep only lines whose [`classify_level`] result is at least this
    /// severe (`Info < Warn < Error`).
    pub min_level: Option<LogLevel>,
}

impl LogQuery {
    /// Whether this query narrows the scan at all. An active query is what
    /// makes a fresh tail seek back [`LogLimits::scan`] bytes instead of
    /// [`LogLimits::payload`] (spec D4) — see [`fresh_tail_start`].
    fn is_active(&self) -> bool {
        self.needle.is_some() || self.min_level.is_some()
    }
}

/// Bounds every dimension a single [`read_window`] call may cost: how many
/// rows it may return, how many cumulative bytes of row text, how long any
/// one line's stored text may be before truncation, and — the load-bearing
/// one — how many bytes of the FILE it may read regardless of the file's
/// real size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLimits {
    /// Maximum number of rows [`LogWindow::rows`] may contain.
    pub rows: usize,
    /// Maximum cumulative bytes of (post-truncation) row text across all of
    /// [`LogWindow::rows`].
    pub payload: u64,
    /// Maximum bytes of stored text for any one line before truncation.
    pub line: u64,
    /// Maximum bytes read from the file in one [`read_window`] call — the
    /// load-bearing limit; see [`DEFAULT_SCAN_BYTES`].
    pub scan: u64,
}

impl Default for LogLimits {
    /// Spec D3's production numbers: 500 rows / 512 KiB payload / 16 KiB
    /// per line / 16 MiB scanned per request.
    fn default() -> Self {
        Self {
            rows: DEFAULT_ROWS,
            payload: DEFAULT_PAYLOAD_BYTES,
            line: DEFAULT_LINE_BYTES,
            scan: DEFAULT_SCAN_BYTES,
        }
    }
}

/// Why a call restarted from a fresh tail instead of continuing forward
/// from the cursor it was given (spec D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogReset {
    /// The file now at `path` is not the same file the cursor was issued
    /// for (different device/inode) — e.g. logrotate's rename-and-recreate.
    Rotated,
    /// The same file is now SHORTER than the cursor's recorded offset —
    /// e.g. `: > file.log`.
    Truncated,
}

/// One returned log line: already classified, and already truncated if it
/// was over [`LogLimits::line`] bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub level: LogLevel,
    pub text: String,
}

/// The result of one [`read_window`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogWindow {
    /// Matching, classified, possibly-truncated lines, oldest first —
    /// capped at `limits.rows` rows and `limits.payload` bytes of text.
    pub rows: Vec<LogRow>,
    /// Pass this back on the next call to resume forward from here.
    /// `None` only when `exists` is `false` — there is nothing to resume
    /// from.
    pub cursor: Option<LogCursor>,
    /// `false` when `path` does not exist right now. Not an error — a log
    /// for a service that has not started yet, or a site whose Apply has
    /// not run, is a normal, pollable state (spec D3).
    pub exists: bool,
    /// `Some` when this call had to restart from a fresh tail because the
    /// file at `path` was rotated or truncated since the given cursor was
    /// issued.
    pub reset: Option<LogReset>,
    /// Whether the file has unread bytes beyond the returned `cursor` —
    /// this call stopped on a `rows`/`payload`/`scan` limit rather than
    /// because the scan ran out of file.
    pub has_more: bool,
    /// The file's total size, as observed at the start of this call.
    pub size_bytes: u64,
    /// How many bytes of the file THIS call actually read — always
    /// `<= limits.scan`, regardless of `size_bytes` (the guarantee this
    /// module exists to make true; see this module's large-file test).
    pub scanned_bytes: u64,
    /// How many of the returned rows had their stored text truncated at
    /// `limits.line` bytes.
    pub truncated_lines: u32,
    /// `true` when the `scan` bound — not `rows`, not `payload`, not EOF —
    /// is what stopped this call from reading further.
    pub scan_bound_reached: bool,
}

fn missing_window() -> LogWindow {
    LogWindow {
        rows: Vec::new(),
        cursor: None,
        exists: false,
        reset: None,
        has_more: false,
        size_bytes: 0,
        scanned_bytes: 0,
        truncated_lines: 0,
        scan_bound_reached: false,
    }
}

/// Where a fresh tail (no cursor, or a reset one) starts scanning from, and
/// whether that start lands inside a line whose leading fragment must be
/// discarded. See this module's doc comment for why an active query seeks
/// back further than a plain tail does.
fn fresh_tail_start(len: u64, query_active: bool, limits: &LogLimits) -> (u64, bool) {
    let span = if query_active {
        limits.scan
    } else {
        limits.payload
    };
    let start = len.saturating_sub(span);
    (start, start > 0)
}

/// Read a bounded, filtered window of `path` (spec D3/D4). See this
/// module's doc comment for the full algorithm and the confinement
/// boundary this function does and does not enforce.
///
/// # Errors
///
/// Returns [`CoreError::Io`] on an unexpected filesystem failure, and
/// [`CoreError::NotAPlainFile`] when `path` exists but is not a regular
/// file (refused rather than followed — a symlink's target is never read).
/// A `path` that does not exist at all is NOT an error: see
/// [`LogWindow::exists`].
pub fn read_window(
    path: &Path,
    cursor: Option<LogCursor>,
    query: &LogQuery,
    limits: &LogLimits,
) -> Result<LogWindow, CoreError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(missing_window()),
        Err(source) => {
            return Err(CoreError::Io {
                op: "symlink_metadata",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let file_type = meta.file_type();
    if !file_type.is_file() {
        let found = if file_type.is_symlink() {
            "a symlink"
        } else if file_type.is_dir() {
            "a directory"
        } else {
            "a special file"
        };
        return Err(CoreError::NotAPlainFile {
            path: path.to_path_buf(),
            found,
        });
    }

    let mut file = match File::open(path) {
        Ok(file) => file,
        // A benign TOCTOU: the file could have been removed between the
        // check above and this open (e.g. a race with rotation). Reported
        // the same way a `NotFound` from the check above is — a state, not
        // an error — rather than surfacing a transient failure the very
        // next poll would not reproduce.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(missing_window()),
        Err(source) => {
            return Err(CoreError::Io {
                op: "open",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let file_meta = file.metadata().map_err(|source| CoreError::Io {
        op: "metadata",
        path: path.to_path_buf(),
        source,
    })?;
    let len = file_meta.len();
    let identity = FileIdentity::of(&file_meta);

    let (start, discard_leading_partial, reset) = match cursor {
        None => {
            let (s, d) = fresh_tail_start(len, query.is_active(), limits);
            (s, d, None)
        }
        Some(c) if c.identity != identity => {
            let (s, d) = fresh_tail_start(len, query.is_active(), limits);
            (s, d, Some(LogReset::Rotated))
        }
        Some(c) if len < c.offset => {
            let (s, d) = fresh_tail_start(len, query.is_active(), limits);
            (s, d, Some(LogReset::Truncated))
        }
        Some(c) => (c.offset, false, None),
    };

    file.seek(SeekFrom::Start(start))
        .map_err(|source| CoreError::Io {
            op: "seek",
            path: path.to_path_buf(),
            source,
        })?;

    let scan = scan_forward(file, path, start, discard_leading_partial, query, limits)?;

    Ok(LogWindow {
        rows: scan.rows,
        cursor: Some(LogCursor {
            identity,
            offset: scan.pos,
        }),
        exists: true,
        reset,
        has_more: scan.pos < len,
        size_bytes: len,
        scanned_bytes: scan.scanned_bytes,
        truncated_lines: scan.truncated_lines,
        scan_bound_reached: scan.scan_bound_reached,
    })
}

/// The mutable bookkeeping [`scan_forward`] accumulates as it walks the
/// file line by line.
struct ScanOutcome {
    rows: Vec<LogRow>,
    /// The byte offset right after the last COMPLETE line consumed. Never
    /// advances past an unterminated trailing fragment.
    pos: u64,
    scanned_bytes: u64,
    truncated_lines: u32,
    scan_bound_reached: bool,
}

/// Walk `file` forward from `start` one line at a time, applying `query`
/// and the truncation/caps in `limits`, never reading more than
/// `limits.scan` bytes from `file` in total.
fn scan_forward(
    file: File,
    path: &Path,
    start: u64,
    discard_leading_partial: bool,
    query: &LogQuery,
    limits: &LogLimits,
) -> Result<ScanOutcome, CoreError> {
    let mut pos = start;
    let mut rows: Vec<LogRow> = Vec::new();
    let mut scanned_bytes: u64 = 0;
    let mut payload_bytes: u64 = 0;
    let mut truncated_lines: u32 = 0;
    let mut scan_bound_reached = false;
    let mut first = true;

    // `Read::take` makes the scan bound PHYSICAL: once `limits.scan` bytes
    // have been pulled from `file` — regardless of how many `read_until`
    // calls that spans, or how long any single line turns out to be — the
    // wrapped reader reports EOF even if the real file has more. This is
    // what makes "never reads more than `scan` bytes" a property of the
    // I/O source instead of a tally this loop could get wrong (and it
    // transitively bounds a single pathological line with no `\n` at all:
    // `read_until` cannot accumulate more than `limits.scan` bytes into one
    // buffer either, since its source goes dry at that point).
    let mut reader = BufReader::new(file.take(limits.scan));
    let mut buf: Vec<u8> = Vec::new();

    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|source| CoreError::Io {
                op: "read",
                path: path.to_path_buf(),
                source,
            })?;
        if n == 0 {
            break; // true EOF: nothing more to scan
        }
        scanned_bytes += n as u64;
        let complete = buf.last() == Some(&b'\n');

        if first {
            first = false;
            if discard_leading_partial {
                if complete {
                    pos += n as u64;
                }
                // else: never found a boundary within the scan span at
                // all — nothing usable comes out of this call, and `pos`
                // must not move past `start` either way.
                if !complete && scanned_bytes >= limits.scan {
                    scan_bound_reached = true;
                }
                continue;
            }
        }

        if !complete {
            // A trailing line with no `\n` yet: spec — neither returned
            // nor counted, and the cursor must not advance past its start
            // so the NEXT call sees the whole thing once the newline
            // arrives.
            if scanned_bytes >= limits.scan {
                scan_bound_reached = true;
            }
            break;
        }

        pos += n as u64;
        let line_bytes = &buf[..buf.len() - 1]; // drop the trailing '\n'
        let (text, was_truncated) = decode_and_cap(line_bytes, limits.line);
        if was_truncated {
            truncated_lines += 1;
        }
        let level = classify_level(&text);
        if line_matches(&text, level, query) {
            payload_bytes += text.len() as u64;
            rows.push(LogRow { level, text });
        }

        if scanned_bytes >= limits.scan {
            scan_bound_reached = true;
            break;
        }
        if rows.len() >= limits.rows {
            break;
        }
        if payload_bytes >= limits.payload {
            break;
        }
    }

    Ok(ScanOutcome {
        rows,
        pos,
        scanned_bytes,
        truncated_lines,
        scan_bound_reached,
    })
}

/// Decode raw line bytes (sans the trailing `\n`) as UTF-8, replacing
/// anything invalid rather than failing — a log line is program- or
/// attacker-controlled content, never a place to introduce a new I/O error
/// kind over. Truncates to `cap` bytes first when the line is longer, so a
/// single absurd line cannot grow the returned payload without bound.
/// Returns the text and whether truncation happened.
fn decode_and_cap(line: &[u8], cap: u64) -> (String, bool) {
    let cap = cap as usize;
    if line.len() > cap {
        (String::from_utf8_lossy(&line[..cap]).into_owned(), true)
    } else {
        (String::from_utf8_lossy(line).into_owned(), false)
    }
}

/// The server-side predicate (spec D4): a level floor plus an optional
/// literal substring, case-insensitive unless `query.case_sensitive`. Runs
/// on every complete line the scan visits, not only on what gets returned
/// — see this module's doc comment.
fn line_matches(text: &str, level: LogLevel, query: &LogQuery) -> bool {
    if let Some(min) = query.min_level
        && level_rank(level) < level_rank(min)
    {
        return false;
    }
    if let Some(needle) = &query.needle {
        let hit = if query.case_sensitive {
            text.contains(needle.as_str())
        } else {
            text.to_lowercase().contains(&needle.to_lowercase())
        };
        if !hit {
            return false;
        }
    }
    true
}

/// [`LogLevel`] (defined in `openvhost-proc`, for the ring buffer) carries
/// no `Ord` — nothing there needs to compare severities, only match on
/// them — so `min_level` filtering ranks them locally rather than adding an
/// `Ord` bound (and a cross-crate change) to a type this crate does not
/// own.
fn level_rank(level: LogLevel) -> u8 {
    match level {
        LogLevel::Info => 0,
        LogLevel::Warn => 1,
        LogLevel::Error => 2,
    }
}

/// The ONE level classifier for FILE lines (spec D4). See this module's
/// doc comment for why `openvhost_proc`'s ring classifier is a deliberately
/// separate function, not this one reused.
///
/// nginx's severities appear bracketed and lowercase (`[error]`, `[warn]`,
/// `[notice]`, `[crit]`, `[alert]`, `[emerg]`); php-fpm's appear bare and
/// uppercase (`WARNING:`, `ERROR:`, `ALERT:`, `NOTICE:`). Lowercasing the
/// whole line and matching plain keyword substrings reads both formats
/// with one pass. `Error`-tier keywords are checked before `Warn` so a line
/// that happens to mention both — nginx's own `FastCGI sent in stderr:
/// "PHP message: PHP Fatal error: ..."` capture, which already contains
/// its OWN `[error]` — reads as the more severe outcome, the same
/// precedence `openvhost_proc`'s ring classifier uses for the same reason
/// on a different input shape. Anything that matches none of these
/// (`[notice]`, `[info]`, `[debug]`, an access-log line, or a line with no
/// recognizable marker at all) reads as the neutral `Info`.
pub fn classify_level(line: &str) -> LogLevel {
    const ERROR_MARKERS: [&str; 5] = ["error", "crit", "alert", "emerg", "fatal"];
    const WARN_MARKERS: [&str; 1] = ["warn"];
    let lower = line.to_ascii_lowercase();
    if ERROR_MARKERS.iter().any(|m| lower.contains(m)) {
        LogLevel::Error
    } else if WARN_MARKERS.iter().any(|m| lower.contains(m)) {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Small, fast-to-run limits for tests that are not exercising a
    /// specific cap — big enough that `rows`/`payload`/`scan` never bind
    /// unexpectedly for a handful of short lines.
    fn small_limits() -> LogLimits {
        LogLimits {
            rows: 1000,
            payload: 10_000,
            line: 1000,
            scan: 10_000,
        }
    }

    fn row_texts(w: &LogWindow) -> Vec<&str> {
        w.rows.iter().map(|r| r.text.as_str()).collect()
    }

    // -- Tail window + forward resume ---------------------------------
    //
    // Vacuity: written against the not-yet-existing `read_window`/`LogRow`
    // etc. (this whole module did not compile until they were added), so
    // RED was "does not compile," not a false-passing assertion.

    #[test]
    fn fresh_tail_discards_the_leading_partial_line_then_forward_read_resumes_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        // Four 6-byte lines ("AAAAA\n" etc), 24 bytes total.
        std::fs::write(&path, "AAAAA\nBBBBB\nCCCCC\nDDDDD\n").unwrap();
        // payload=10 seeks back to offset 14, landing inside "CCCCC\n"
        // (global bytes 12..=17) at its third 'C' — never a '\n' boundary.
        let limits = LogLimits {
            rows: 10,
            payload: 10,
            line: 100,
            scan: 10_000,
        };
        let w1 = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert_eq!(
            row_texts(&w1),
            vec!["DDDDD"],
            "the mid-line fragment of CCCCC must be discarded, not returned"
        );
        assert!(w1.reset.is_none());

        // Append new content and resume from the returned cursor: only the
        // NEW line must come back, proving the resume point is exact.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"EEEEE\n").unwrap();
        drop(f);

        let w2 = read_window(&path, w1.cursor, &LogQuery::default(), &limits).unwrap();
        assert_eq!(row_texts(&w2), vec!["EEEEE"]);
        assert!(w2.reset.is_none());
    }

    #[test]
    fn trailing_line_without_newline_is_held_back_until_the_newline_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        std::fs::write(&path, "line1\nline2\nline3").unwrap(); // no trailing \n
        let limits = small_limits();

        let w1 = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert_eq!(
            row_texts(&w1),
            vec!["line1", "line2"],
            "the unterminated 'line3' must not appear yet"
        );
        assert_eq!(w1.truncated_lines, 0);

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);

        let w2 = read_window(&path, w1.cursor, &LogQuery::default(), &limits).unwrap();
        assert_eq!(row_texts(&w2), vec!["line3"]);
    }

    // -- Reset detection ------------------------------------------------

    #[test]
    fn truncating_the_file_in_place_reports_reset_truncated_and_restarts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("error.log");
        std::fs::write(&path, "line1\nline2\nline3\nline4\nline5\n").unwrap();
        let limits = small_limits();
        let w1 = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert!(w1.reset.is_none());

        // `std::fs::write` opens the EXISTING path and truncates in place —
        // same inode, drastically shorter content.
        std::fs::write(&path, "new1\n").unwrap();

        let w2 = read_window(&path, w1.cursor, &LogQuery::default(), &limits).unwrap();
        assert_eq!(w2.reset, Some(LogReset::Truncated));
        assert_eq!(row_texts(&w2), vec!["new1"]);
    }

    #[cfg(unix)]
    #[test]
    fn replacing_the_file_reports_reset_rotated_and_restarts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("error.log");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        let limits = small_limits();
        let w1 = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert!(w1.reset.is_none());

        // logrotate's actual move: rename the old file aside, then create a
        // brand-new one at the original path — guaranteeing a new inode.
        std::fs::rename(&path, dir.path().join("error.log.1")).unwrap();
        std::fs::write(&path, "fresh1\nfresh2\n").unwrap();

        let w2 = read_window(&path, w1.cursor, &LogQuery::default(), &limits).unwrap();
        assert_eq!(w2.reset, Some(LogReset::Rotated));
        assert_eq!(row_texts(&w2), vec!["fresh1", "fresh2"]);
    }

    // -- Existence and confinement ---------------------------------------

    #[test]
    fn a_missing_file_reports_exists_false_without_erroring() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.log");
        let w = read_window(&path, None, &LogQuery::default(), &small_limits()).unwrap();
        assert!(!w.exists);
        assert!(w.rows.is_empty());
        assert!(w.cursor.is_none());
        assert_eq!(w.size_bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_path_is_refused_and_its_target_is_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret-elsewhere.log");
        std::fs::write(&secret, "top secret contents\n").unwrap();
        let link = dir.path().join("error.log");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let err = read_window(&link, None, &LogQuery::default(), &small_limits()).unwrap_err();
        match err {
            CoreError::NotAPlainFile { path, found } => {
                assert_eq!(path, link);
                assert_eq!(found, "a symlink");
            }
            other => panic!("expected NotAPlainFile, got {other:?}"),
        }
    }

    // -- Truncation of an over-long line -----------------------------------

    #[test]
    fn an_over_long_line_is_truncated_at_the_line_cap_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let long_line = "a".repeat(500);
        std::fs::write(&path, format!("{long_line}\nshort\n")).unwrap();
        let limits = LogLimits {
            rows: 10,
            payload: 10_000,
            line: 16,
            scan: 10_000,
        };
        let w = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert_eq!(w.truncated_lines, 1);
        assert_eq!(w.rows[0].text.len(), 16);
        assert!(w.rows[0].text.chars().all(|c| c == 'a'));
        assert_eq!(w.rows[1].text, "short");
    }

    // -- The load-bearing guarantee: never loads a whole file -------------

    #[test]
    fn a_large_file_scan_never_exceeds_the_scan_bound_and_is_fast() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
            // No "ZZZ" anywhere in the filler content.
            let line = b"the quick brown fox jumps over the lazy dog repeatedly\n";
            let target_bytes = 5 * 1024 * 1024; // well over the scan bound below
            let mut written = 0usize;
            while written < target_bytes {
                f.write_all(line).unwrap();
                written += line.len();
            }
            f.flush().unwrap();
        }
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 4 * 1024 * 1024);

        let limits = LogLimits {
            rows: 10_000,
            payload: 50 * 1024 * 1024,
            line: 1024,
            scan: 512 * 1024, // << file length
        };
        let query = LogQuery {
            needle: Some("ZZZ_NEVER_PRESENT_ZZZ".to_string()),
            ..Default::default()
        };

        // A CONTINUATION cursor pinned at the very start of the huge file
        // (real identity, offset 0): the realistic "Follow was left on for
        // hours and the file grew by megabytes since the last poll" case.
        // This is also the only scenario that actually exercises the
        // FORWARD-SCAN loop's own bound rather than a fresh tail's already
        // scan-bounded seek-back distance — a `cursor: None` query-active
        // load starts at `len - limits.scan` by construction (see
        // `fresh_tail_start`), so reading forward from there can never
        // exceed `limits.scan` bytes no matter what the loop itself does,
        // which would make that scenario pass even with the loop's own
        // bound removed. Pinning a continuation cursor at offset 0 removes
        // that confound: nothing but the loop's own `Read::take` bound
        // stands between this call and the rest of the file. `LogCursor`'s
        // fields are private everywhere outside this module, but this
        // `tests` module is a descendant of `read` and may build one
        // directly for exactly this reason.
        let cursor = LogCursor {
            identity: FileIdentity::of(&meta),
            offset: 0,
        };

        let started = std::time::Instant::now();
        let w = read_window(&path, Some(cursor), &query, &limits).unwrap();
        let elapsed = started.elapsed();

        assert!(
            w.reset.is_none(),
            "identity matches; this is a plain continuation"
        );
        assert!(
            w.scanned_bytes <= limits.scan,
            "scanned {} bytes, limit was {}",
            w.scanned_bytes,
            limits.scan
        );
        assert!(w.rows.is_empty(), "the needle never appears in the fixture");
        assert!(
            w.has_more,
            "the scan must stop far short of this huge file's real EOF"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "took {elapsed:?} — looks like the whole {}-byte file was read",
            meta.len()
        );
    }

    #[test]
    fn scan_bound_reached_is_set_when_the_bound_stops_an_unmatched_search() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let mut content = String::new();
        for i in 0..200 {
            content.push_str(&format!("filler line number {i}\n"));
        }
        std::fs::write(&path, &content).unwrap();
        assert!(
            content.len() > 500,
            "fixture must exceed the scan bound below"
        );

        let limits = LogLimits {
            rows: 1000,
            payload: 1_000_000,
            line: 1000,
            scan: 200,
        };
        let query = LogQuery {
            needle: Some("NEVER_PRESENT".to_string()),
            ..Default::default()
        };
        let w = read_window(&path, None, &query, &limits).unwrap();
        assert!(w.scan_bound_reached);
        assert!(w.rows.is_empty());
        assert!(w.scanned_bytes <= limits.scan);
    }

    // -- Filtering reaches back through the file ---------------------------

    #[test]
    fn filtering_reaches_back_through_the_file_further_than_a_plain_tail_would() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let mut content = String::from("NEEDLE old-timer entry\n");
        for i in 0..30 {
            content.push_str(&format!("filler line number {i}\n"));
        }
        std::fs::write(&path, &content).unwrap();

        // payload is far too small to ever reach the first line; scan is
        // generous enough to reach all the way back to it.
        let limits = LogLimits {
            rows: 10,
            payload: 30,
            line: 200,
            scan: 10_000,
        };

        // Control: an UNFILTERED plain tail (payload-bounded) must NOT
        // reach the needle line at all.
        let plain = read_window(&path, None, &LogQuery::default(), &limits).unwrap();
        assert!(
            !row_texts(&plain).iter().any(|t| t.contains("NEEDLE")),
            "a plain tail must not reach all the way back to the first line"
        );

        // The filtered scan (scan-bounded) DOES reach it.
        let query = LogQuery {
            needle: Some("NEEDLE".to_string()),
            case_sensitive: true,
            min_level: None,
        };
        let filtered = read_window(&path, None, &query, &limits).unwrap();
        assert_eq!(filtered.rows.len(), 1);
        assert!(filtered.rows[0].text.contains("NEEDLE"));
    }

    #[test]
    fn non_matching_lines_still_advance_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        std::fs::write(&path, "filler a\nfiller b\nfiller c\nfiller d\nfiller e\n").unwrap();
        let limits = small_limits();

        // Nothing in the file matches; the scan must still run to EOF and
        // commit the cursor past every filler line.
        let query = LogQuery {
            needle: Some("ZZZ".to_string()),
            ..Default::default()
        };
        let w1 = read_window(&path, None, &query, &limits).unwrap();
        assert!(w1.rows.is_empty());

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"new-line-a\nnew-line-b\n").unwrap();
        drop(f);

        // Resuming unfiltered must return ONLY the newly appended lines —
        // proving the earlier filtered call already consumed the fillers.
        let w2 = read_window(&path, w1.cursor, &LogQuery::default(), &limits).unwrap();
        assert_eq!(row_texts(&w2), vec!["new-line-a", "new-line-b"]);
    }

    #[test]
    fn case_sensitive_toggle_changes_whether_a_differently_cased_needle_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        std::fs::write(&path, "line with NEEDLE upper\nline with needle lower\n").unwrap();
        let limits = small_limits();

        let sensitive = LogQuery {
            needle: Some("NEEDLE".to_string()),
            case_sensitive: true,
            min_level: None,
        };
        let w1 = read_window(&path, None, &sensitive, &limits).unwrap();
        assert_eq!(row_texts(&w1), vec!["line with NEEDLE upper"]);

        let insensitive = LogQuery {
            needle: Some("NEEDLE".to_string()),
            case_sensitive: false,
            min_level: None,
        };
        let w2 = read_window(&path, None, &insensitive, &limits).unwrap();
        assert_eq!(w2.rows.len(), 2);
    }

    #[test]
    fn min_level_filter_keeps_only_lines_at_or_above_the_floor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("error.log");
        std::fs::write(
            &path,
            "plain info line one\n\
             2024/01/01 00:00:00 [warn] something\n\
             2024/01/01 00:00:00 [error] something\n\
             plain info line two\n",
        )
        .unwrap();
        let limits = small_limits();
        let query = LogQuery {
            needle: None,
            case_sensitive: false,
            min_level: Some(LogLevel::Warn),
        };
        let w = read_window(&path, None, &query, &limits).unwrap();
        assert_eq!(w.rows.len(), 2);
        assert_eq!(w.rows[0].level, LogLevel::Warn);
        assert_eq!(w.rows[1].level, LogLevel::Error);
    }

    // -- classify_level -----------------------------------------------------

    #[test]
    fn classify_level_reads_nginx_and_php_fpm_severities_and_a_plain_line_is_neutral() {
        assert_eq!(
            classify_level("2024/01/15 10:30:00 [error] 123#0: open() failed"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level("2024/01/15 10:30:00 [warn] 123#0: conflicting server name"),
            LogLevel::Warn
        );
        assert_eq!(
            classify_level("2024/01/15 10:30:00 [notice] 123#0: signal process started"),
            LogLevel::Info
        );
        assert_eq!(
            classify_level(
                "[15-Jan-2026 10:30:00] WARNING: [pool www] child exited on signal 11 (SIGSEGV)"
            ),
            LogLevel::Warn
        );
        assert_eq!(
            classify_level("[15-Jan-2026 10:30:00] ERROR: unable to bind listening socket"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level(
                r#"2024/01/15 10:30:00 [error] 123#0: *5 FastCGI sent in stderr: "PHP message: PHP Fatal error:  Uncaught Error: Call to undefined function foo()" while reading response header from upstream"#
            ),
            LogLevel::Error
        );
        assert_eq!(
            classify_level(
                r#"127.0.0.1 - - [15/Jan/2026:10:30:00 +0000] "GET /favicon.ico HTTP/1.1" 200 512"#
            ),
            LogLevel::Info
        );
        assert_eq!(
            classify_level("just a plain line with no markers"),
            LogLevel::Info
        );
    }

    #[test]
    fn default_limits_match_the_documented_spec_d3_numbers() {
        let l = LogLimits::default();
        assert_eq!(l.rows, 500);
        assert_eq!(l.payload, 512 * 1024);
        assert_eq!(l.line, 16 * 1024);
        assert_eq!(l.scan, 16 * 1024 * 1024);
    }
}
