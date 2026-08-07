// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtimes: which are installed, and how to install another.

mod brew;
mod discover;
mod package;

pub use brew::{
    CATALOGUE, PhpMajor, brew_formula, brew_install_spec, brew_uninstall_spec, find_brew,
};
pub use discover::{BREW_PREFIXES, discover_php_in, php_runtime_for_major};
pub use package::{
    Availability, PHP_PACKAGE_NAME, PHP_PACKAGES, PHP_WARMUP_BINARY, PhpPackage, PhpPackageInstall,
    install_php_package, php_package_for_host, php_package_for_target,
};
