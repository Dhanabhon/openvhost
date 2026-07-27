// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtimes: which are installed, and how to install another.

mod discover;
// Task 3 adds `mod brew;` here (install-a-version) and re-exports its names
// alongside `discover`'s below.

pub use discover::{BREW_PREFIXES, discover_php_in};
