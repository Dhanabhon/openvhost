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
    for dir in [
        "www",
        "run",
        "run/nginx",
        "logs",
        "logs/sites",
        "logs/services",
    ] {
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

/// Security audit H1 (THE merge blocker): spec D3's at-rest argument for the
/// MySQL root credential stored in `state.db` assumes `<home>` is 0700 —
/// otherwise any other local account can walk in and read it (macOS puts
/// every account in the `staff` group, and `/Users/<name>` is
/// group-traversable by default). `short_home()`'s own tempdir already
/// happens to be 0700 (the `tempfile` crate's own default), which would make
/// a naive assertion here vacuous — so this deliberately loosens it first to
/// prove `provision_home` is the one actually tightening it, on EVERY call,
/// not only when it creates the directory fresh (an install that predates
/// this fix must be repaired the very next time it launches).
#[cfg(unix)]
#[test]
fn provisioning_locks_the_home_directory_to_0700_even_when_it_already_existed_looser() {
    use std::os::unix::fs::PermissionsExt;
    let home = short_home();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

    provision_home(home.path()).unwrap();

    let mode = std::fs::metadata(home.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "home directory must be 0700 after provisioning, even though it already existed looser"
    );
}

/// The other half of "apply on every provision, not only creation": a home
/// directory that does not exist YET must also come out at 0700, not
/// whatever the ambient umask would otherwise give `create_dir_all`.
#[cfg(unix)]
#[test]
fn provisioning_creates_a_brand_new_home_directory_at_0700() {
    use std::os::unix::fs::PermissionsExt;
    let outer = tempfile::Builder::new()
        .prefix("ovh-outer")
        .tempdir_in("/tmp")
        .unwrap();
    let home = outer.path().join("openvhost-home");
    assert!(
        !home.exists(),
        "must not exist yet for this test to prove anything"
    );

    provision_home(&home).unwrap();

    let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

/// P1 live-log-viewer design (spec D2/D5), plus security audit L2: `logs`
/// itself — parent of the GLOBAL nginx access+error logs, the aggregate of
/// EVERY site's requests — and `logs/sites`/`logs/services`, the two parent
/// directories `site::apply::commit` creates a per-site or per-major
/// directory under during an Apply, are all seeded here so they exist (and
/// are already the right mode) before the first Apply ever runs. `logs`
/// itself used to come from a plain `create_dir_all` (ambient umask,
/// typically 0755) instead of [`openvhost_core`]'s shared `ensure_log_dir`
/// like its two children — this proves it no longer does.
#[cfg(unix)]
#[test]
fn provisioning_seeds_the_log_parent_directories_at_0700() {
    use std::os::unix::fs::PermissionsExt;
    let home = short_home();

    provision_home(home.path()).unwrap();

    for dir in ["logs", "logs/sites", "logs/services"] {
        let path = home.path().join(dir);
        assert!(path.is_dir(), "{dir} must exist");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{dir} must be 0700 (spec D5), got {mode:o}");
    }
}

/// Mirrors `provisioning_locks_the_home_directory_to_0700_even_when_it_already_existed_looser`:
/// an install that predates this seeding (or one that ran an Apply, which
/// creates a per-site/per-major directory but never re-visits these two
/// PARENTS) may have left one of these at whatever the ambient umask gave
/// it. Provisioning again must tighten it, not leave it alone merely because
/// it already existed.
#[cfg(unix)]
#[test]
fn provisioning_tightens_a_pre_existing_log_parent_directory_to_0700() {
    use std::os::unix::fs::PermissionsExt;
    let home = short_home();
    let sites_dir = home.path().join("logs/sites");
    std::fs::create_dir_all(&sites_dir).unwrap();
    std::fs::set_permissions(&sites_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    provision_home(home.path()).unwrap();

    let mode = std::fs::metadata(&sites_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "logs/sites pre-existed at a looser mode and must be tightened, not left alone"
    );
}
