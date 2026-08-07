// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP from OpenVHost's own package tree — like nginx and MariaDB (and
//! unlike MySQL), from OpenVHost's own **build**: PHP is compiled by
//! static-php-cli from verified, individually pinned sources
//! (`build/recipes/php.sh`, `build/recipes/_php-pins.sh`; php-recipe design
//! D1).
//!
//! Two pieces, in the order they run:
//!
//! - [`catalogue`] — the compiled-in pin: which build we install for which
//!   host and PHP major, from where, what its bytes must hash to, whether
//!   the release exists yet, and the two dates spec §14's security
//!   obligation fires on. Nothing a user supplies can reach any of those
//!   values. Unlike nginx's and MariaDB's catalogues, the lookup takes a
//!   major — see that module's header for why.
//! - [`install`] — the wiring to `openvhost-pkg`'s download → verify →
//!   extract → atomic-install pipeline. This module adds no install
//!   machinery; it is that pipeline's fourth consumer and reuses it
//!   unchanged.
//!
//! There is no `ledger` here: [`crate::mysql::InstallLedger`] is keyed by
//! package name and already records any package, so a second one would be a
//! second shape for one fact — the same reasoning [`crate::mariadb`] and
//! [`crate::nginx`] already give for reusing it rather than growing a copy.

mod catalogue;
mod install;

pub use catalogue::{
    Availability, PHP_PACKAGE_NAME, PHP_PACKAGES, PHP_WARMUP_BINARY, PhpPackage,
    php_package_for_host, php_package_for_target,
};
pub use install::{PhpPackageInstall, install_php_package};
