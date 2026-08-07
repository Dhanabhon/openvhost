// SPDX-License-Identifier: GPL-3.0-or-later
//! LIVE proof for off-Homebrew slice 5B (PHP discovery), against the real
//! artifact slice 5A's recipe produced and against this machine's real
//! Homebrew PHP — not against fixtures.
//!
//! The packaged half runs against `/opt/openvhost-build/php-8.4.24` (the
//! recipe's staged prefix: `bin/php`, `bin/php-fpm`, `modules/*.so`), copied
//! into a throwaway home. `/opt/openvhost-build` is never written to, and no
//! test here ever reads or writes the user's real `~/.openvhost`: every
//! `PackagesRoot` is minted from a `tempfile::TempDir`.
//!
//! The Homebrew half runs against whatever brew actually has, through the
//! production probe (`openvhost_conf::probe_php_fpm_version`).
//!
//! Skipped, loudly, when the staged prefix is absent — this cannot be part of
//! CI, because CI has no 5A build output.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openvhost_core::{PackagesRoot, PhpRuntimeSource, discover_php};

/// The real staged prefix 5A's recipe leaves behind. Read-only here.
const STAGED_PREFIX: &str = "/opt/openvhost-build/php-8.4.24";

/// This machine's real Homebrew prefixes, in the production order.
fn brew_prefixes() -> Vec<PathBuf> {
    openvhost_core::BREW_PREFIXES
        .iter()
        .map(PathBuf::from)
        .collect()
}

fn staged() -> Option<PathBuf> {
    let p = PathBuf::from(STAGED_PREFIX);
    p.join("bin/php-fpm").is_file().then_some(p)
}

/// Recursive copy preserving permission bits. Deliberately NOT a `cp` child
/// process: the harness must not put process spawns anywhere near the tests
/// that claim discovery spawns nothing.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Reproduce `openvhost_pkg::layout::update_current` exactly — a RELATIVE
/// symlink whose target is the bare version string, published by tmp+rename
/// (`crates/openvhost-pkg/src/platform/unix.rs:38-48`). That function is
/// `pub(crate)`, so an integration test cannot call the real writer; this is
/// the closest reachable equivalent and the shape it produces is identical.
fn link_current(link: &Path, version: &str) {
    let tmp = link.with_file_name(format!(".current.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(version, &tmp).unwrap();
    std::fs::rename(&tmp, link).unwrap();
}

/// Lay the real staged prefix out the way the app expects:
/// `<home>/packages/php/<major>/<version>/` with `current` -> `<version>`.
fn install_packaged(home: &Path, major: &str, version: &str, src: &Path) -> PathBuf {
    let root = PackagesRoot::from_home(home);
    let dir = root.package_dir("php", major, version);
    copy_tree(src, &dir);
    link_current(&root.current_link("php", major), version);
    dir
}

/// A probe that counts every call. Discovery must not reach it for a packaged
/// runtime; for a brew one it may, and the count says whether it did.
fn counting_probe(
    inner: impl Fn(&Path) -> Option<String> + 'static,
) -> (Arc<AtomicUsize>, impl Fn(&Path) -> Option<String>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    (calls, move |bin: &Path| {
        seen.fetch_add(1, Ordering::SeqCst);
        inner(bin)
    })
}

/// The production probe, verbatim: `stack.rs:826` wraps this same call.
fn real_probe(bin: &Path) -> Option<String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(openvhost_conf::probe_php_fpm_version(bin))
}

fn ino(p: &Path) -> u64 {
    std::fs::metadata(p).unwrap().ino()
}

/// Run a binary and return its stdout banner, or the OS error string.
fn run_v(bin: &Path) -> Result<String, String> {
    match std::process::Command::new(bin).arg("-v").output() {
        Ok(out) => Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Proof 1
// ---------------------------------------------------------------------------

#[test]
fn proof_1_the_real_packaged_tree_is_found_and_its_path_never_goes_through_current() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 1: {STAGED_PREFIX} absent");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let dir = install_packaged(home.path(), "8.4", "8.4.24", &src);
    let root = PackagesRoot::from_home(home.path());

    let (calls, probe) = counting_probe(|_| panic!("packaged pass must not probe"));
    let found = discover_php(&root, &[], &probe);

    assert_eq!(found.runtimes.len(), 1, "{:?}", found.runtimes);
    let rt = &found.runtimes[0];
    assert_eq!(rt.major, "8.4");
    assert_eq!(
        rt.source,
        PhpRuntimeSource::Packaged {
            version: "8.4.24".to_string()
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // The handed-out path is INSIDE the version directory, and no component of
    // it is `current`.
    assert_eq!(rt.fpm_bin, dir.join("bin/php-fpm"));
    assert!(rt.fpm_bin.is_file());
    assert!(
        !rt.fpm_bin.components().any(|c| c.as_os_str() == "current"),
        "path goes through the link: {}",
        rt.fpm_bin.display()
    );
    eprintln!(
        "PROOF 1  source={:?}  bin={}",
        rt.source,
        rt.fpm_bin.display()
    );

    // Now swap `current` under the already-resolved runtime.
    let before = ino(&rt.fpm_bin);
    let other = install_packaged(home.path(), "8.4", "8.4.99", &src);
    let link = root.current_link("php", "8.4");
    assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("8.4.99"));

    // Non-vacuity: the swap really took effect — a FRESH discovery follows it.
    let (_, probe2) = counting_probe(|_| panic!("packaged pass must not probe"));
    let after_swap = discover_php(&root, &[], &probe2);
    assert_eq!(
        after_swap.runtimes[0].source,
        PhpRuntimeSource::Packaged {
            version: "8.4.99".to_string()
        },
        "the repoint did not take effect, so this test proves nothing"
    );

    // ...and the ALREADY-RETURNED path is unaffected: same file, same inode,
    // still under 8.4.24.
    assert_eq!(ino(&rt.fpm_bin), before);
    assert_eq!(rt.fpm_bin, dir.join("bin/php-fpm"));
    // The two version dirs are genuinely different files, so "same inode"
    // above is a discriminating assertion and not an artifact of hardlinking.
    assert_ne!(
        before,
        ino(&other.join("bin/php-fpm")),
        "the two version directories share an inode; the check above cannot discriminate"
    );
    // Had the resolver handed out `current/bin/php-fpm`, THIS is the file the
    // caller would now be holding.
    assert_eq!(
        ino(&link.join("bin/php-fpm")),
        ino(&other.join("bin/php-fpm"))
    );
    eprintln!(
        "PROOF 1  after swap: current -> 8.4.99, returned path still inode {before} under 8.4.24"
    );
}

// ---------------------------------------------------------------------------
// Proof 2
// ---------------------------------------------------------------------------

#[test]
fn proof_2_the_version_is_read_from_the_tree_with_nothing_executed() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 2: {STAGED_PREFIX} absent");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    // The version DIRECTORY deliberately disagrees with the binary inside it.
    let dir = install_packaged(home.path(), "8.4", "8.4.99", &src);
    let root = PackagesRoot::from_home(home.path());

    let (calls, probe) = counting_probe(|_| panic!("packaged pass must not probe"));
    let found = discover_php(&root, &[], &probe);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the probe was called");

    // (a) Discovery reports the DIRECTORY name.
    assert_eq!(
        found.runtimes[0].source,
        PhpRuntimeSource::Packaged {
            version: "8.4.99".to_string()
        }
    );
    // (b) The binary itself says something else. If any execution had produced
    //     the version above, it would read 8.4.24.
    let banner = run_v(&dir.join("bin/php-fpm")).unwrap();
    assert!(banner.contains("8.4.24"), "unexpected banner: {banner}");
    assert!(!banner.contains("8.4.99"));
    eprintln!("PROOF 2a reported=8.4.99  binary says: {banner}");

    // (c) Structural: make the binary non-executable and discover again. Any
    //     exec would fail with EACCES; the version still comes back.
    let bin = dir.join("bin/php-fpm");
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
    std::fs::set_permissions(&bin, perms).unwrap();
    // Non-vacuity: it really is unexecutable now.
    let err = run_v(&bin).unwrap_err();
    assert!(err.to_lowercase().contains("permission"), "{err}");

    let (calls2, probe2) = counting_probe(|_| panic!("packaged pass must not probe"));
    let again = discover_php(&root, &[], &probe2);
    assert_eq!(
        again.runtimes[0].source,
        PhpRuntimeSource::Packaged {
            version: "8.4.99".to_string()
        }
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
    eprintln!("PROOF 2c chmod 0444 -> exec fails ({err}), discovery still reports 8.4.99");
}

// ---------------------------------------------------------------------------
// Proof 3
// ---------------------------------------------------------------------------

#[test]
fn proof_3_the_recipes_layout_is_the_layout_the_walk_finds_and_a_brew_shape_is_refused() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 3: {STAGED_PREFIX} absent");
        return;
    };

    // The two constants that have only ever been compared by eye, now compared
    // against each other AND against the real tree, in one place.
    let recipe = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../build/recipes/php.sh")
        .canonicalize()
        .unwrap();
    let text = std::fs::read_to_string(&recipe).unwrap();
    assert!(
        text.contains(r#"RECIPE_SERVER_BIN="bin/php-fpm""#),
        "the recipe no longer declares bin/php-fpm"
    );
    assert!(
        src.join("bin/php-fpm").is_file(),
        "real tree lacks bin/php-fpm"
    );
    assert!(
        !src.join("sbin/php-fpm").exists(),
        "the real tree unexpectedly has a brew-shaped sbin/php-fpm too"
    );

    // The real walk over the real tree.
    let home = tempfile::tempdir().unwrap();
    install_packaged(home.path(), "8.4", "8.4.24", &src);
    let root = PackagesRoot::from_home(home.path());
    let (_, probe) = counting_probe(|_| panic!("packaged pass must not probe"));
    let found = discover_php(&root, &[], &probe);
    assert_eq!(found.runtimes.len(), 1);
    assert!(found.runtimes[0].fpm_bin.ends_with("bin/php-fpm"));
    eprintln!(
        "PROOF 3a recipe={} tree=bin/php-fpm walk={}",
        recipe.display(),
        found.runtimes[0].fpm_bin.display()
    );

    // A Homebrew-SHAPED tree planted in the package tree: same real binary,
    // at brew's `sbin/php-fpm` instead. It must be refused, and reported.
    let brewish = root.package_dir("php", "8.3", "8.3.99");
    std::fs::create_dir_all(brewish.join("sbin")).unwrap();
    std::fs::copy(src.join("bin/php-fpm"), brewish.join("sbin/php-fpm")).unwrap();
    link_current(&root.current_link("php", "8.3"), "8.3.99");
    assert!(brewish.join("sbin/php-fpm").is_file());

    let (_, probe2) = counting_probe(|_| panic!("packaged pass must not probe"));
    let mixed = discover_php(&root, &[], &probe2);
    assert!(
        !mixed.runtimes.iter().any(|r| r.major == "8.3"),
        "a brew-shaped tree was accepted: {:?}",
        mixed.runtimes
    );
    // D4: refused, but NOT silently absent.
    assert!(
        mixed.unidentified.contains(&root.major_dir("php", "8.3")),
        "unidentified = {:?}",
        mixed.unidentified
    );
    assert!(!mixed.is_complete());
    // Non-vacuity: move the identical binary to bin/ and the same tree IS
    // accepted, so the refusal is about the path and nothing else.
    std::fs::create_dir_all(brewish.join("bin")).unwrap();
    std::fs::rename(brewish.join("sbin/php-fpm"), brewish.join("bin/php-fpm")).unwrap();
    let (_, probe3) = counting_probe(|_| panic!("packaged pass must not probe"));
    let fixed = discover_php(&root, &[], &probe3);
    assert!(fixed.runtimes.iter().any(|r| r.major == "8.3"));
    assert!(fixed.is_complete());
    eprintln!("PROOF 3b sbin/php-fpm refused + reported unidentified; bin/php-fpm accepted");
}

// ---------------------------------------------------------------------------
// Proof 4
// ---------------------------------------------------------------------------

#[test]
fn proof_4_the_packaged_tree_coexists_with_this_machines_real_homebrew_php() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 4: {STAGED_PREFIX} absent");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    install_packaged(home.path(), "8.4", "8.4.24", &src);
    let root = PackagesRoot::from_home(home.path());

    let prefixes = brew_prefixes();
    let refs: Vec<&Path> = prefixes.iter().map(PathBuf::as_path).collect();
    let (calls, probe) = counting_probe(real_probe);
    let found = discover_php(&root, &refs, &probe);

    for rt in &found.runtimes {
        eprintln!(
            "PROOF 4  {} {:<9} {}",
            rt.major,
            rt.source.as_str(),
            rt.fpm_bin.display()
        );
    }
    eprintln!(
        "PROOF 4  probe calls={}  unidentified={:?}",
        calls.load(Ordering::SeqCst),
        found.unidentified
    );

    let majors: Vec<&str> = found.runtimes.iter().map(|r| r.major.as_str()).collect();
    assert_eq!(majors, ["8.4", "8.5"], "majors on this machine");

    let ours = &found.runtimes[0];
    assert_eq!(
        ours.source,
        PhpRuntimeSource::Packaged {
            version: "8.4.24".to_string()
        }
    );
    assert!(ours.fpm_bin.starts_with(home.path()));

    let theirs = &found.runtimes[1];
    assert_eq!(theirs.source, PhpRuntimeSource::Homebrew);
    assert_eq!(theirs.source.version(), None);
    assert!(theirs.fpm_bin.starts_with("/opt/homebrew"));
    assert!(theirs.fpm_bin.is_file());
    // Brew preference 2, live: `php@8.5` beats the `php` alias inside one
    // prefix. Both exist on this machine and both point at Cellar/php/8.5.9.
    assert_eq!(
        theirs.fpm_bin,
        Path::new("/opt/homebrew/opt/php@8.5/sbin/php-fpm")
    );
    assert!(Path::new("/opt/homebrew/opt/php/sbin/php-fpm").is_file());
}

// ---------------------------------------------------------------------------
// Proof 5
// ---------------------------------------------------------------------------

#[test]
fn proof_5_the_first_entry_is_the_lowest_major_installed_not_the_packaged_one() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 5: {STAGED_PREFIX} absent");
        return;
    };
    let prefixes = brew_prefixes();
    let refs: Vec<&Path> = prefixes.iter().map(PathBuf::as_path).collect();

    // (a) This machine as it actually is: packaged 8.4 + brew 8.5.
    let low = tempfile::tempdir().unwrap();
    install_packaged(low.path(), "8.4", "8.4.24", &src);
    let (_, p1) = counting_probe(real_probe);
    let a = discover_php(&PackagesRoot::from_home(low.path()), &refs, &p1);
    let first_a = &a.runtimes[0];
    eprintln!(
        "PROOF 5a packaged 8.4 + brew 8.5 -> first = {} ({})",
        first_a.major,
        first_a.source.as_str()
    );
    assert_eq!(first_a.major, "8.4");
    assert_eq!(first_a.source.as_str(), "packaged");

    // (b) The SAME real binaries planted under a HIGHER major, to separate
    //     "packaged wins within a major" from "packaged ranks first overall".
    //     §7 as corrected predicts brew 8.5 takes the first slot here.
    let high = tempfile::tempdir().unwrap();
    install_packaged(high.path(), "8.6", "8.6.0", &src);
    let (_, p2) = counting_probe(real_probe);
    let b = discover_php(&PackagesRoot::from_home(high.path()), &refs, &p2);
    let first_b = &b.runtimes[0];
    eprintln!(
        "PROOF 5b packaged 8.6 + brew 8.5 -> first = {} ({})",
        first_b.major,
        first_b.source.as_str()
    );
    assert_eq!(
        b.runtimes
            .iter()
            .map(|r| r.major.as_str())
            .collect::<Vec<_>>(),
        ["8.5", "8.6"]
    );
    assert_eq!(first_b.major, "8.5");
    assert_eq!(first_b.source, PhpRuntimeSource::Homebrew);
}

// ---------------------------------------------------------------------------
// Proof 6 — a STATED LIMIT, not a pass.
// ---------------------------------------------------------------------------

#[test]
fn proof_6_the_same_major_collision_is_not_reachable_on_this_machine() {
    // Gated on the 5A artifact like every other test here, even though it does
    // not use it: the assertion below is a TRIPWIRE for whoever is running the
    // live proof, not a statement about an arbitrary machine. Ungated it would
    // fail CI on any runner that happened to carry a Homebrew php@8.4.
    if staged().is_none() {
        eprintln!("SKIP proof 6: {STAGED_PREFIX} absent");
        return;
    }
    let mut brew_majors: Vec<String> = Vec::new();
    for prefix in brew_prefixes() {
        let Ok(entries) = std::fs::read_dir(prefix.join("opt")) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if (name == "php" || name.starts_with("php@"))
                && e.path().join("sbin/php-fpm").is_file()
            {
                let v = openvhost_core::resolve_keg(&e.path())
                    .and_then(|k| k.major_minor())
                    .unwrap_or_else(|| "?".into());
                brew_majors.push(format!("{}/{name} -> {v}", prefix.display()));
            }
        }
    }
    eprintln!("PROOF 6  brew PHP present: {brew_majors:?}");
    assert!(
        !Path::new("/opt/homebrew/opt/php@8.4").exists()
            && !Path::new("/usr/local/opt/php@8.4").exists(),
        "a brew 8.4 appeared; the same-major collision IS now reachable live and \
         proof 6 must become a real test instead of a stated limit"
    );
    eprintln!(
        "PROOF 6  LIMIT: no Homebrew 8.4 exists here, so the packaged-beats-brew \
         collision (spec §8.3) was NOT exercised live. Unit fixtures cover it."
    );
}

/// PARTIAL cover for §8.3, and labelled partial on purpose.
///
/// The Homebrew side is entirely real — this machine's `php@8.5`. The packaged
/// side is the real 5A tree planted under a major directory named `8.5`, which
/// is a FICTION: the binary inside it is 8.4.24. Discovery derives `major` from
/// the directory name and from nothing else, so the merge rule under test is
/// exercised exactly as written; what is NOT exercised is a genuinely built PHP
/// 8.5 package. This does not make §8.3 "proven live" — see the test above for
/// the limit that stands.
#[test]
fn proof_6b_packaged_beats_the_real_brew_8_5_when_planted_at_the_same_major() {
    let Some(src) = staged() else {
        eprintln!("SKIP proof 6b: {STAGED_PREFIX} absent");
        return;
    };
    let prefixes = brew_prefixes();
    let refs: Vec<&Path> = prefixes.iter().map(PathBuf::as_path).collect();

    // Baseline: with an EMPTY package tree, brew's 8.5 is present. Without
    // this, "brew's entry was dropped" could just mean it was never there.
    let empty = tempfile::tempdir().unwrap();
    let (_, p0) = counting_probe(real_probe);
    let before = discover_php(&PackagesRoot::from_home(empty.path()), &refs, &p0);
    let brew_8_5: Vec<_> = before
        .runtimes
        .iter()
        .filter(|r| r.major == "8.5")
        .collect();
    assert_eq!(brew_8_5.len(), 1);
    assert_eq!(brew_8_5[0].source, PhpRuntimeSource::Homebrew);
    eprintln!(
        "PROOF 6b baseline: brew 8.5 present at {}",
        brew_8_5[0].fpm_bin.display()
    );

    // Now plant ours at the same major.
    let home = tempfile::tempdir().unwrap();
    install_packaged(home.path(), "8.5", "8.5.0", &src);
    let (_, p1) = counting_probe(real_probe);
    let after = discover_php(&PackagesRoot::from_home(home.path()), &refs, &p1);

    let at_8_5: Vec<_> = after.runtimes.iter().filter(|r| r.major == "8.5").collect();
    assert_eq!(
        at_8_5.len(),
        1,
        "brew's entry was duplicated or appended: {:?}",
        after.runtimes
    );
    assert_eq!(
        at_8_5[0].source,
        PhpRuntimeSource::Packaged {
            version: "8.5.0".to_string()
        }
    );
    assert!(at_8_5[0].fpm_bin.starts_with(home.path()));
    assert_eq!(after.runtimes.len(), 1, "{:?}", after.runtimes);
    assert!(after.unidentified.is_empty());
    eprintln!(
        "PROOF 6b packaged 8.5 (fictional major, real 5A binaries) beat real brew 8.5; \
         1 entry, source=packaged, path={}",
        at_8_5[0].fpm_bin.display()
    );
}
