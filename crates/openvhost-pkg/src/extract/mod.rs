// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened archive extraction. The validation primitives in `validate` are
//! the trusted-computing-base: every path/entry check fails closed and the
//! whole archive is rejected on any violation, BEFORE a single byte is
//! written (spec §5 S10–S19). Format walks (`targz`, `zip`) build a plan
//! with these primitives, then materialize it.

pub(crate) mod targz;
pub(crate) mod validate;

/// The extraction-plan contract that `targz`/`zip`'s format walks build
/// (pass 1) and materialize (pass 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedKind {
    Dir,
    File { mode: u32 },
    Symlink { target: String },
    Hardlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedEntry {
    pub rel: String,
    pub kind: PlannedKind,
}
