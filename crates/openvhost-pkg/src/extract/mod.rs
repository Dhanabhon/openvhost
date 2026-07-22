// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened archive extraction. The validation primitives in `validate` are
//! the trusted-computing-base: every path/entry check fails closed and the
//! whole archive is rejected on any violation, BEFORE a single byte is
//! written (spec §5 S10–S19). Format walks (`targz`, `zip`) build a plan
//! with these primitives, then materialize it.

pub(crate) mod validate;

// `PlannedKind`/`PlannedEntry` are the extraction-plan contract Tasks 3–4
// build and materialize; nothing constructs them yet (not even a test), so
// they're unconditionally dead code until those tasks land. Remove this
// allow once `targz`/`zip` construct them.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedKind {
    Dir,
    File { mode: u32 },
    Symlink { target: String },
    Hardlink { target: String },
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedEntry {
    pub rel: String,
    pub kind: PlannedKind,
}
