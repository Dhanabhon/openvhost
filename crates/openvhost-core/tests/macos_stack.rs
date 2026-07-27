// SPDX-License-Identifier: GPL-3.0-or-later
//! `provision_home`'s directory/seed contract. The old exit-criterion proof
//! (real nginx + php-fpm spawned against a hand-written demo config, serving
//! phpinfo over a unix socket) is retired along with the demo stack itself:
//! `site::apply` now owns every generated file, and
//! `openvhost-conf/tests/validate_live.rs` proves the GENERATED config passes
//! the native validators. `openvhost-proc/tests/e2e.rs` covers the supervised
//! lifecycle. What is left to prove here is narrower and platform-located
//! rather than platform-specific: provisioning creates the directories the
//! generated tree expects, seeds the welcome page, and writes no config of
//! its own.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use openvhost_core::platform::macos::demo_stack::provision_home;

/// Short-path tempdir: /tmp keeps generated paths far under Darwin's
/// 104-byte `sun_path` limit that `site::apply::socket_path` guards
/// elsewhere (TMPDIR is /var/folders/... and brittle-long).
fn short_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ovh")
        .tempdir_in("/tmp")
        .unwrap()
}

#[test]
fn provisioning_creates_the_directories_and_seeds_the_welcome_page() {
    let home = short_home();
    provision_home(home.path()).unwrap();
    for dir in ["www", "run", "run/nginx", "logs"] {
        assert!(home.path().join(dir).is_dir(), "{dir} must exist");
    }
    let index = home.path().join("www/index.php");
    assert!(index.is_file());
    let page = std::fs::read_to_string(index).unwrap();
    assert!(
        page.contains("PHP_VERSION"),
        "the page must still prove PHP runs"
    );
    assert!(
        page.contains("DO NOT EDIT"),
        "it is rewritten every launch, so say so"
    );
    // The catch-all answers ANY unmatched Host on 127.0.0.1:8080, which makes it
    // readable by any local process and, under DNS rebinding, by a web page.
    // phpinfo() there would hand out absolute paths, the extension inventory and
    // every php.ini value (security audit A1).
    assert!(
        !page.contains("phpinfo"),
        "the landing page must not disclose phpinfo to an unmatched host"
    );
}

#[test]
fn provisioning_no_longer_writes_any_config() {
    // The generated tree is the only config source now; a stale hand-written
    // conf/ would be a second source of truth nobody updates.
    let home = short_home();
    provision_home(home.path()).unwrap();
    assert!(!home.path().join("conf/nginx.conf").exists());
    assert!(!home.path().join("conf/php-fpm.conf").exists());
}

#[test]
fn provisioning_is_idempotent() {
    let home = short_home();
    provision_home(home.path()).unwrap();
    provision_home(home.path()).unwrap();
    assert!(home.path().join("www/index.php").is_file());
}
