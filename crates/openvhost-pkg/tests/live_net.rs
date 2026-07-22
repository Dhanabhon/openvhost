// SPDX-License-Identifier: GPL-3.0-or-later
//! Exit-criterion proof (master plan P0-6): install a REAL php.net source
//! tarball. Network + ~22 MB download — gated behind OPENVHOST_NET_TESTS=1
//! so the default `cargo test` stays hermetic and offline.
//!
//! Run: OPENVHOST_NET_TESTS=1 cargo test -p openvhost-pkg --test live_net -- --nocapture
//!
//! If php.net has rotated 8.4.23 out of /distributions (moved to the museum),
//! update PIN_URL + PIN_SHA to the current 8.4 release from
//! https://www.php.net/releases/index.php?json&version=8.4
#![allow(clippy::unwrap_used)]

use openvhost_pkg::{ArchiveFormat, InstallRequest, PackagesRoot, install_package};

const PIN_URL: &str = "https://www.php.net/distributions/php-8.4.23.tar.gz";
const PIN_SHA: &str = "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573";

#[tokio::test]
async fn installs_real_php_tarball() {
    if std::env::var("OPENVHOST_NET_TESTS").as_deref() != Ok("1") {
        eprintln!("SKIP live_net: set OPENVHOST_NET_TESTS=1 to run the real php.net download");
        return;
    }
    let home = tempfile::Builder::new()
        .prefix("ovh-live")
        .tempdir_in("/tmp")
        .unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let req = InstallRequest::new(
        "php",
        "8.4",
        "8.4.23",
        PIN_URL,
        PIN_SHA,
        ArchiveFormat::TarGz,
    )
    .unwrap();
    let installed = install_package(&req, &root, |_| {}).await.unwrap();
    // php source tarball has configure + main/php_version.h
    assert!(
        installed.dir.join("configure").is_file(),
        "expected configure at package root"
    );
    assert!(installed.dir.join("main/php_version.h").is_file());
    assert_eq!(
        std::fs::read_link(&installed.current_link)
            .unwrap()
            .to_str()
            .unwrap(),
        "8.4.23"
    );
    eprintln!(
        "LIVE OK: installed php-8.4.23 at {}",
        installed.dir.display()
    );
}
