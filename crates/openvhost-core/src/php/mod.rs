// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtimes: which are installed, and how to install another.

mod brew;
mod discover;

pub use brew::{CATALOGUE, PhpMajor, brew_install_spec, find_brew};
pub use discover::{BREW_PREFIXES, discover_php_in};
