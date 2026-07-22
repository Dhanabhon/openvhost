// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened archive extraction. The validation primitives in `validate` are
//! the trusted-computing-base: every path/entry check fails closed and the
//! whole archive is rejected on any violation, BEFORE a single byte is
//! written (spec §5 S10–S19). Format walks (`targz`, `zip`) build a plan
//! with these primitives, then materialize it. `common` holds the
//! reject/mode-clamp/capped-copy helpers both walks share; `targz` alone
//! defines the `PlannedKind`/`PlannedEntry` plan types its own two-pass walk
//! needs (`zip`'s random-access central directory uses its own lighter
//! `Staged`/`PlannedFile` locals instead).

pub(crate) mod common;
pub(crate) mod targz;
pub(crate) mod validate;
pub(crate) mod zip;
