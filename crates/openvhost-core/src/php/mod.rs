// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtimes: which are installed, and how to install another.

mod brew;
mod discover;
mod package;

pub use brew::{
    CATALOGUE, PhpMajor, brew_formula, brew_install_spec, brew_uninstall_spec, find_brew,
};
/// `discover_php` is the ONLY discovery entry point exported here, and
/// deliberately so: it reads both install sources. The Homebrew-only half
/// (`discover::discover_php_in`) is private to that module — a caller outside
/// this crate that could see only Homebrew would be blind to every PHP
/// OpenVHost installed itself, which is the whole point of slice 5B.
pub use discover::{
    BREW_PREFIXES, PackagedPhpInstall, PhpRuntimeSource, discover_php, packaged_php_install,
    php_runtime_for_major,
};
pub use package::{
    Availability, PHP_PACKAGE_NAME, PHP_PACKAGES, PHP_WARMUP_BINARY, PhpPackage, PhpPackageInstall,
    install_php_package, php_package_for_host, php_package_for_target,
};
