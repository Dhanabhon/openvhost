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
//!
//! ## Slice 0 — "prove the payload" (2026-08-01)
//!
//! Everything below `installs_real_php_tarball` is THROWAWAY INVESTIGATION,
//! not a regression suite: it answers whether this crate can ingest a real
//! ~160 MB database tarball, and whether MariaDB publishes an arm64 macOS
//! tarball at all. It changes no product code. Run it with:
//!
//! ```text
//! OPENVHOST_NET_TESTS=1 cargo test -p openvhost-pkg --test live_net \
//!     -- --nocapture --test-threads=1
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openvhost_pkg::{ArchiveFormat, InstallRequest, PackagesRoot, Progress, install_package};

const PIN_URL: &str = "https://www.php.net/distributions/php-8.4.23.tar.gz";
const PIN_SHA: &str = "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573";

#[tokio::test]
async fn installs_real_php_tarball() {
    if !gated() {
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

// ===========================================================================
// Slice 0 investigation
// ===========================================================================

/// Pinned by hand on 2026-08-01. `curl -I` on this URL reported
/// Content-Length: 167977240, Last-Modified: Tue, 30 Jun 2026 16:19:42 GMT.
const MYSQL_URL: &str =
    "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz";
/// Locally computed with `shasum -a 256`. Oracle publishes only an MD5
/// sidecar (`...tar.gz.md5` = 6e89113f04f2af85d0a164573493db3a, which the
/// same download matched) plus a detached PGP `.asc` — no SHA-256 sidecar.
const MYSQL_SHA: &str = "b96e00493bc3499b9ffd7f08d65c5d64933af0383a8287d9873b64f94c2d6009";
const MYSQL_VER: &str = "8.4.11";
/// The tarball's own top-level directory name — the one `strip_single_root`
/// is supposed to remove.
const MYSQL_TOP: &str = "mysql-8.4.11-macos15-arm64";

/// Shared cache so a re-run does not re-pull 160 MB for the *structural*
/// analysis. The pipeline test always fetches from the CDN regardless — that
/// download IS the measurement.
const CACHE_DIR: &str = "/tmp/openvhost-slice0-cache";

fn gated() -> bool {
    if std::env::var("OPENVHOST_NET_TESTS").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("SKIP live_net: set OPENVHOST_NET_TESTS=1 to run the real network tests");
    false
}

fn banner(s: &str) {
    eprintln!("\n=====================================================================");
    eprintln!("{s}");
    eprintln!("=====================================================================");
}

async fn http_text(url: &str) -> Option<String> {
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .ok()?;
    let r = c.get(url).send().await.ok()?;
    let status = r.status();
    let b = r.bytes().await.ok()?;
    eprintln!("  GET {url} -> {status} ({} bytes)", b.len());
    Some(String::from_utf8_lossy(&b).into_owned())
}

// ---------------------------------------------------------------------------
// Q1 — does MariaDB publish an arm64 macOS tarball at all?  And does MySQL?
// ---------------------------------------------------------------------------

#[tokio::test]
async fn q1_macos_arm64_tarball_availability() {
    if !gated() {
        return;
    }

    banner("Q1a — MariaDB: any macOS / arm64 binary tarball?");
    // The archive is the authoritative file tree; the REST API is the
    // authoritative *metadata* (it carries an explicit `os` field per file).
    for ver in ["11.4.12", "11.8.8", "10.11.11", "10.4.13"] {
        let url = format!("https://archive.mariadb.org/mariadb-{ver}/");
        if let Some(body) = http_text(&url).await {
            let mut dirs: Vec<String> = body
                .split("href=\"")
                .skip(1)
                .filter_map(|s| s.split('"').next())
                .filter(|s| s.ends_with('/') && !s.starts_with('/') && !s.starts_with(".."))
                .map(|s| s.trim_end_matches('/').to_string())
                .collect();
            dirs.sort();
            dirs.dedup();
            eprintln!("  mariadb-{ver}/ subdirs: {dirs:?}");
            let mac: Vec<&String> = dirs
                .iter()
                .filter(|d| {
                    let d = d.to_lowercase();
                    d.contains("osx")
                        || d.contains("macos")
                        || d.contains("darwin")
                        || d.contains("arm64")
                        || d.contains("aarch64")
                })
                .collect();
            eprintln!("  -> macOS/arm64 candidates: {mac:?}");
        }
        let api = format!("https://downloads.mariadb.org/rest-api/mariadb/{ver}/");
        if let Some(body) = http_text(&api).await {
            // Crude but dependency-free: pull every "file_name" value and
            // every "os" value out of the JSON.
            let names = json_string_values(&body, "file_name");
            let oss = {
                let mut v = json_string_values(&body, "os");
                v.sort();
                v.dedup();
                v
            };
            eprintln!("  REST os values: {oss:?}");
            let hits: Vec<&String> = names
                .iter()
                .filter(|n| {
                    let n = n.to_lowercase();
                    n.contains("osx")
                        || n.contains("macos")
                        || n.contains("darwin")
                        || n.contains("arm64")
                        || n.contains("aarch64")
                })
                .collect();
            eprintln!(
                "  REST filenames matching macos/arm: {hits:?}  (of {})",
                names.len()
            );
        }
    }

    banner("Q1b — MySQL 8.4.x macos arm64 tarball + sidecars");
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .unwrap();
    for candidate in [
        "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz",
        "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos14-arm64.tar.gz",
        "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-x86_64.tar.gz",
        "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-macos15-arm64.tar.gz",
    ] {
        match c.head(candidate).send().await {
            Ok(r) => eprintln!(
                "  HEAD {} -> {} content-length={:?}",
                candidate,
                r.status(),
                r.headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
            ),
            Err(e) => eprintln!("  HEAD {candidate} -> transport error: {e}"),
        }
    }
    for sidecar in [".asc", ".md5", ".sha256", ".sha1"] {
        let u = format!("{MYSQL_URL}{sidecar}");
        match c.get(&u).send().await {
            Ok(r) => {
                let st = r.status();
                let b = r.bytes().await.unwrap_or_default();
                let head = String::from_utf8_lossy(&b[..b.len().min(60)]).replace('\n', "\\n");
                eprintln!("  {sidecar}: {st} ({} bytes) first60={head:?}", b.len());
            }
            Err(e) => eprintln!("  {sidecar}: transport error: {e}"),
        }
    }
}

/// Pull every value of `"key": "value"` out of a JSON blob without adding a
/// serde dependency to this crate. Good enough for a throwaway probe.
fn json_string_values(body: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\":");
    let mut out = Vec::new();
    for seg in body.split(&pat).skip(1) {
        let seg = seg.trim_start();
        if let Some(rest) = seg.strip_prefix('"') {
            if let Some(v) = rest.split('"').next() {
                out.push(v.to_string());
            }
        } else if seg.starts_with("null") {
            out.push("null".into());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Q2 — does our extractor accept the real MySQL tarball?
// ---------------------------------------------------------------------------

/// One tar entry, exactly as tar-rs (hence our extractor's pass 1) sees it.
#[derive(Debug, Clone)]
struct Ent {
    rel: String,
    etype: tar::EntryType,
    size: u64,
    link: Option<String>,
}

fn read_archive(bytes: &[u8]) -> Vec<Ent> {
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut ar = tar::Archive::new(gz);
    let mut out = Vec::new();
    for e in ar.entries().unwrap() {
        let e = e.unwrap();
        let h = e.header();
        out.push(Ent {
            rel: e.path().unwrap().to_string_lossy().into_owned(),
            etype: h.entry_type(),
            size: e.size(),
            link: e
                .link_name()
                .ok()
                .flatten()
                .map(|p| p.to_string_lossy().into_owned()),
        });
    }
    out
}

/// Mirrors `extract/validate.rs`'s reserved-name rule so we can report which
/// entries WOULD trip it, independently of which check fires first.
fn reserved_hit(rel: &str) -> Option<String> {
    const RESERVED: [&str; 24] = [
        "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
        "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
        "lpt9",
    ];
    for c in rel.split('/') {
        if c.is_empty() {
            continue;
        }
        let stem = c.split('.').next().unwrap_or(c).to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Some(c.to_string());
        }
    }
    None
}

async fn cached_mysql_tarball() -> PathBuf {
    std::fs::create_dir_all(CACHE_DIR).unwrap();
    let p = Path::new(CACHE_DIR).join(format!("{MYSQL_TOP}.tar.gz"));
    if p.is_file() {
        eprintln!(
            "  cache hit: {} ({} bytes)",
            p.display(),
            p.metadata().unwrap().len()
        );
        return p;
    }
    eprintln!("  cache miss — fetching {MYSQL_URL}");
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .unwrap();
    let t0 = Instant::now();
    let b = c
        .get(MYSQL_URL)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    eprintln!("  fetched {} bytes in {:?}", b.len(), t0.elapsed());
    std::fs::write(&p, &b).unwrap();
    p
}

#[tokio::test]
async fn q2_real_mysql_tarball_through_our_pipeline() {
    if !gated() {
        return;
    }

    banner("Q2a — structural census of the REAL MySQL 8.4.11 arm64 tarball");
    let cache = cached_mysql_tarball().await;
    let raw = std::fs::read(&cache).unwrap();
    eprintln!("  compressed size: {} bytes", raw.len());
    let t0 = Instant::now();
    let ents = read_archive(&raw);
    eprintln!("  parsed {} tar entries in {:?}", ents.len(), t0.elapsed());
    census(&ents);

    banner("Q2b — install_package() against the LIVE CDN url");
    let home = tempfile::Builder::new()
        .prefix("ovh-slice0-mysql")
        .tempdir_in("/tmp")
        .unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let req = InstallRequest::new(
        "mysql",
        "8.4",
        MYSQL_VER,
        MYSQL_URL,
        MYSQL_SHA,
        ArchiveFormat::TarGz,
    )
    .expect("InstallRequest::new must accept the pinned mysql url");

    let start = Instant::now();
    let mut declared_total: Option<u64> = None;
    let mut bytes_seen = 0u64;
    let mut t_verified: Option<Duration> = None;
    let res = install_package(&req, &root, |p| match p {
        Progress::Started { total } => {
            declared_total = total;
            eprintln!(
                "  Progress::Started total={total:?} at {:?}",
                start.elapsed()
            );
        }
        Progress::Downloaded { bytes } => bytes_seen = bytes,
        Progress::Verified => {
            t_verified = Some(start.elapsed());
            eprintln!("  Progress::Verified at {:?}", start.elapsed());
        }
        Progress::Extracted => eprintln!("  Progress::Extracted at {:?}", start.elapsed()),
        Progress::Linked => eprintln!("  Progress::Linked at {:?}", start.elapsed()),
    })
    .await;
    eprintln!(
        "  download+verify wall clock: {:?} for {bytes_seen} bytes (declared {declared_total:?}) \
         => {:.1} MB/s",
        t_verified,
        bytes_seen as f64
            / 1_048_576.0
            / t_verified.unwrap_or(Duration::from_secs(1)).as_secs_f64()
    );
    eprintln!("  TOTAL_TIMEOUT is 900s; SIZE_CAP is 1 GiB (download.rs:17,20)");
    match &res {
        Ok(inst) => {
            eprintln!("  install_package -> Ok({})", inst.dir.display());
            dump_tree(&inst.dir, 2);
        }
        Err(e) => eprintln!("  install_package -> Err({e})"),
    }

    banner("Q2c — which check fires, and what is behind it (repack variants)");
    // The pipeline stops at the FIRST violation, so one live run can only
    // ever show one blocker. These variants replay the REAL archive's entry
    // list (real names, real types, real symlink targets) with contents
    // emptied, through the REAL install_package over loopback http, peeling
    // one blocker off at a time.
    let variants: Vec<(&str, Vec<Ent>)> = vec![
        ("A: verbatim entry list (contents emptied)", ents.clone()),
        ("B: symlinks -> regular files", drop_symlinks(&ents)),
        (
            "C: symlinks -> files, duplicate dir entries removed",
            dedup_dirs(&drop_symlinks(&ents)),
        ),
        (
            "D: C + an explicit top-level dir entry (the control)",
            with_root_dir_entry(&dedup_dirs(&drop_symlinks(&ents))),
        ),
    ];
    for (label, list) in variants {
        eprintln!("\n  --- variant {label} ({} entries) ---", list.len());
        let body = repack(&list);
        let sha = sha_hex(&body);
        let url = serve_once(body);
        let h = tempfile::Builder::new()
            .prefix("ovh-slice0-var")
            .tempdir_in("/tmp")
            .unwrap();
        let r = PackagesRoot::from_home(h.path());
        std::fs::create_dir_all(r.as_path()).unwrap();
        let rq = InstallRequest::new("mysql", "8.4", MYSQL_VER, &url, &sha, ArchiveFormat::TarGz)
            .unwrap();
        match install_package(&rq, &r, |_| {}).await {
            Ok(inst) => {
                eprintln!("  -> Ok({})", inst.dir.display());
                eprintln!(
                    "     bin/mysqld at package root?          {}",
                    inst.dir.join("bin/mysqld").exists()
                );
                eprintln!(
                    "     bin/mysqld one level too deep ({MYSQL_TOP}/)? {}",
                    inst.dir.join(MYSQL_TOP).join("bin/mysqld").exists()
                );
                dump_tree(&inst.dir, 1);
            }
            Err(e) => eprintln!("  -> Err({e})"),
        }
    }
}

fn census(ents: &[Ent]) {
    use tar::EntryType as T;
    let (mut dirs, mut files, mut syms, mut hards, mut meta) = (0, 0, 0, 0, 0);
    let mut max_bytes = (0usize, String::new());
    let mut max_depth = (0usize, String::new());
    let mut declared = 0u64;
    let mut dotdot_syms: Vec<(String, String)> = Vec::new();
    let mut abs_syms: Vec<(String, String)> = Vec::new();
    let mut reserved: Vec<(String, String)> = Vec::new();
    let mut tops: Vec<String> = Vec::new();
    let mut root_dir_entry = false;
    let mut seen_keys: std::collections::HashMap<String, usize> = Default::default();

    for e in ents {
        match e.etype {
            T::Directory => {
                dirs += 1;
                if e.rel.trim_end_matches('/') == MYSQL_TOP {
                    root_dir_entry = true;
                }
            }
            T::Regular | T::Continuous => {
                files += 1;
                declared += e.size;
            }
            T::Symlink => {
                syms += 1;
                if let Some(t) = &e.link {
                    if t.starts_with('/') {
                        abs_syms.push((e.rel.clone(), t.clone()));
                    } else if t.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
                        dotdot_syms.push((e.rel.clone(), t.clone()));
                    }
                }
            }
            T::Link => hards += 1,
            _ => meta += 1,
        }
        let b = e.rel.len();
        if b > max_bytes.0 {
            max_bytes = (b, e.rel.clone());
        }
        let d = e
            .rel
            .split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .count();
        if d > max_depth.0 {
            max_depth = (d, e.rel.clone());
        }
        if let Some(c) = reserved_hit(&e.rel) {
            reserved.push((e.rel.clone(), c));
        }
        let top = e.rel.split('/').next().unwrap_or("").to_string();
        if !tops.contains(&top) {
            tops.push(top);
        }
        *seen_keys
            .entry(e.rel.trim_end_matches('/').to_lowercase())
            .or_insert(0) += 1;
    }

    eprintln!("  entries as tar-rs yields them: {}", ents.len());
    eprintln!(
        "    dirs={dirs} files={files} symlinks={syms} hardlinks={hards} metadata-hdrs={meta}"
    );
    eprintln!(
        "  MAX_ENTRIES  = 100000   actual = {:<7} -> {}",
        ents.len(),
        verdict(ents.len() as u64 <= 100_000)
    );
    eprintln!(
        "  MAX_REL_BYTES=    240   actual = {:<7} -> {}   ({})",
        max_bytes.0,
        verdict(max_bytes.0 <= 240),
        max_bytes.1
    );
    eprintln!(
        "  MAX_DEPTH    =     32   actual = {:<7} -> {}   ({})",
        max_depth.0,
        verdict(max_depth.0 <= 32),
        max_depth.1
    );
    eprintln!(
        "  MAX_TOTAL_BYTES=4 GiB   actual = {} bytes ({:.2} GiB) -> {}",
        declared,
        declared as f64 / (1024.0 * 1024.0 * 1024.0),
        verdict(declared <= 4 * 1024 * 1024 * 1024)
    );
    eprintln!(
        "  reserved device names (aux/con/nul/prn/com0-9/lpt0-9): {} hits -> {}",
        reserved.len(),
        verdict(reserved.is_empty())
    );
    for (r, c) in reserved.iter().take(20) {
        eprintln!("      HIT component {c:?} in {r}");
    }
    eprintln!(
        "  symlink targets with '..'/'.'/empty component: {} -> {}",
        dotdot_syms.len(),
        verdict(dotdot_syms.is_empty())
    );
    for (l, t) in dotdot_syms.iter().take(6) {
        eprintln!("      {l} -> {t}");
    }
    if dotdot_syms.len() > 6 {
        eprintln!("      ... and {} more", dotdot_syms.len() - 6);
    }
    eprintln!("  absolute symlink targets: {}", abs_syms.len());
    eprintln!("  top-level components: {tops:?}");
    eprintln!(
        "  explicit top-level DIRECTORY entry ({MYSQL_TOP}/) present: {root_dir_entry} -> {}",
        verdict(root_dir_entry)
    );
    eprintln!("    (strip_single_root only strips when this is true — validate.rs:122-127)");
    let dupes: Vec<(&String, &usize)> = seen_keys.iter().filter(|(_, v)| **v > 1).collect();
    eprintln!(
        "  case-folded duplicate final paths: {} -> {}",
        dupes.len(),
        verdict(dupes.is_empty())
    );
    for (k, n) in dupes.iter().take(10) {
        eprintln!("      {k} x{n}");
    }
}

fn verdict(ok: bool) -> &'static str {
    if ok { "OK" } else { "*** VIOLATION ***" }
}

/// Variants A-D above emptied every file so the *validation* verdict came
/// back fast. That leaves the headline UX number unmeasured: how long does
/// our two-pass extractor take on the REAL 669 MB payload? Repack the real
/// archive with real content, the three blockers removed, and run it through
/// the real `install_package` over loopback.
#[tokio::test]
async fn q2d_full_payload_install_wall_clock() {
    if !gated() {
        return;
    }
    banner("Q2d — full-payload install wall clock through OUR extractor");
    let cache = cached_mysql_tarball().await;

    let t0 = Instant::now();
    let body = {
        use flate2::{Compression, write::GzEncoder};
        let f = std::fs::File::open(&cache).unwrap();
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        let gz = GzEncoder::new(Vec::new(), Compression::fast());
        let mut out = tar::Builder::new(gz);
        // Blocker 3: supply the explicit top-level dir entry upstream omits.
        let mut root = tar::Header::new_gnu();
        root.set_size(0);
        root.set_entry_type(tar::EntryType::Directory);
        root.set_mode(0o755);
        root.set_cksum();
        out.append_data(&mut root, format!("{MYSQL_TOP}/"), std::io::empty())
            .unwrap();
        let mut seen = std::collections::HashSet::new();
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let rel = e.path().unwrap().to_string_lossy().into_owned();
            let et = e.header().entry_type();
            // Blocker 1: drop the 22 `..`-target symlinks outright.
            if et == tar::EntryType::Symlink {
                continue;
            }
            // Blocker 2: drop the 12 duplicate directory headers.
            if !seen.insert(rel.trim_end_matches('/').to_lowercase()) {
                continue;
            }
            let mut h = tar::Header::new_gnu();
            h.set_size(e.size());
            h.set_mode(e.header().mode().unwrap_or(0o644));
            h.set_entry_type(et);
            h.set_cksum();
            let name = if et == tar::EntryType::Directory {
                format!("{}/", rel.trim_end_matches('/'))
            } else {
                rel
            };
            out.append_data(&mut h, name, &mut e).unwrap();
        }
        out.into_inner().unwrap().finish().unwrap()
    };
    eprintln!(
        "  repacked real-content archive: {} bytes in {:?}",
        body.len(),
        t0.elapsed()
    );

    let sha = sha_hex(&body);
    let url = serve_once(body);
    let h = tempfile::Builder::new()
        .prefix("ovh-slice0-full")
        .tempdir_in("/tmp")
        .unwrap();
    let root = PackagesRoot::from_home(h.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let rq =
        InstallRequest::new("mysql", "8.4", MYSQL_VER, &url, &sha, ArchiveFormat::TarGz).unwrap();

    let start = Instant::now();
    let mut t_ver = Duration::ZERO;
    let mut t_ext = Duration::ZERO;
    let res = install_package(&rq, &root, |p| match p {
        Progress::Verified => {
            t_ver = start.elapsed();
            eprintln!("  Verified  at {t_ver:?}");
        }
        Progress::Extracted => {
            t_ext = start.elapsed();
            eprintln!("  Extracted at {t_ext:?}");
        }
        Progress::Linked => eprintln!("  Linked    at {:?}", start.elapsed()),
        _ => {}
    })
    .await;
    match res {
        Ok(inst) => {
            eprintln!("  install_package -> Ok({})", inst.dir.display());
            let mysqld = inst.dir.join("bin/mysqld");
            eprintln!("  bin/mysqld at package root: {}", mysqld.is_file());
            eprintln!(
                "  EXTRACT-ONLY time (Extracted - Verified): {:?}",
                t_ext.saturating_sub(t_ver)
            );
            eprintln!("  TOTAL install wall clock: {:?}", start.elapsed());
            let (_, du, _) = sh("/usr/bin/du", &["-sh", inst.dir.to_str().unwrap()]);
            eprintln!("  installed size: {}", du.trim());
            // Does it run, extracted by OUR code this time?
            let (s1, o1, e1) = sh(mysqld.to_str().unwrap(), &["--version"]);
            eprintln!("  1st `mysqld --version`: {s1} in {e1:?} -> {}", o1.trim());
            let (s2, _, e2) = sh(mysqld.to_str().unwrap(), &["--version"]);
            eprintln!("  2nd `mysqld --version`: {s2} in {e2:?}");
            let (_, x, _) = sh("/usr/bin/xattr", &["-l", mysqld.to_str().unwrap()]);
            eprintln!("  xattr after OUR strip_quarantine: {:?}", x.trim());
            let (_, cs, _) = sh(
                "/usr/bin/codesign",
                &["-v", "--verbose=2", mysqld.to_str().unwrap()],
            );
            eprintln!("  codesign -v after OUR extraction: {}", cs.trim());
            eprintln!(
                "  NOTE the 12 dropped symlinks are absent: lib/libcrypto.dylib exists = {}",
                inst.dir.join("lib/libcrypto.dylib").exists()
            );
        }
        Err(e) => eprintln!("  install_package -> Err({e})"),
    }
}

fn drop_symlinks(ents: &[Ent]) -> Vec<Ent> {
    ents.iter()
        .map(|e| {
            if e.etype == tar::EntryType::Symlink {
                Ent {
                    rel: e.rel.clone(),
                    etype: tar::EntryType::Regular,
                    size: 0,
                    link: None,
                }
            } else {
                e.clone()
            }
        })
        .collect()
}

fn dedup_dirs(ents: &[Ent]) -> Vec<Ent> {
    let mut seen = std::collections::HashSet::new();
    ents.iter()
        .filter(|e| seen.insert(e.rel.trim_end_matches('/').to_lowercase()))
        .cloned()
        .collect()
}

fn with_root_dir_entry(ents: &[Ent]) -> Vec<Ent> {
    let mut v = vec![Ent {
        rel: format!("{MYSQL_TOP}/"),
        etype: tar::EntryType::Directory,
        size: 0,
        link: None,
    }];
    v.extend(ents.iter().cloned());
    v
}

fn repack(ents: &[Ent]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    let gz = GzEncoder::new(Vec::new(), Compression::fast());
    let mut ar = tar::Builder::new(gz);
    for e in ents {
        let mut h = tar::Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o755);
        h.set_entry_type(e.etype);
        if let Some(t) = &e.link {
            h.set_link_name(t).unwrap();
        }
        h.set_cksum();
        ar.append_data(&mut h, &e.rel, std::io::empty()).unwrap();
    }
    ar.into_inner().unwrap().finish().unwrap()
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}

fn serve_once(body: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut b = [0u8; 2048];
            let _ = s.read(&mut b);
            let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(hdr.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}/pkg.tar.gz")
}

fn dump_tree(dir: &Path, depth: usize) {
    fn walk(p: &Path, d: usize, max: usize, pad: usize) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items.iter().take(12) {
            let md = std::fs::symlink_metadata(e.path());
            let kind = md
                .as_ref()
                .map(|m| {
                    if m.file_type().is_symlink() {
                        "link"
                    } else if m.is_dir() {
                        "dir "
                    } else {
                        "file"
                    }
                })
                .unwrap_or("????");
            eprintln!(
                "     {:pad$}{kind} {}",
                "",
                e.file_name().to_string_lossy(),
                pad = pad
            );
            if md.map(|m| m.is_dir()).unwrap_or(false) && d < max {
                walk(&e.path(), d + 1, max, pad + 2);
            }
        }
        if items.len() > 12 {
            eprintln!("     {:pad$}... {} more", "", items.len() - 12, pad = pad);
        }
    }
    walk(dir, 0, depth, 0);
}

// ---------------------------------------------------------------------------
// Q3 — is the extracted MySQL usable?
// ---------------------------------------------------------------------------

fn sh(prog: &str, args: &[&str]) -> (std::process::ExitStatus, String, Duration) {
    let t0 = Instant::now();
    let out = std::process::Command::new(prog)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn {prog}: {e}"));
    let el = t0.elapsed();
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status, s, el)
}

#[tokio::test]
async fn q3_extracted_mysql_is_usable() {
    if !gated() {
        return;
    }
    banner("Q3 — is the extracted MySQL 8.4.11 arm64 tree usable?");
    let cache = cached_mysql_tarball().await;

    // Our own extractor refuses this archive (Q2). To answer Q3 at all we
    // must materialize the tree some other way — system bsdtar — while
    // reproducing the shape our pipeline WOULD produce: extract into a
    // staging dir, then rename it into its final packages/<n>/<M>/<v>/ home,
    // exactly as layout::install_dir does. That keeps the Gatekeeper
    // question ("does the first-exec cost land before or after the atomic
    // rename?") honest.
    let home = tempfile::Builder::new()
        .prefix("ovh-slice0-q3")
        .tempdir_in("/tmp")
        .unwrap();
    let staging = home.path().join("packages/.staging/x/root");
    std::fs::create_dir_all(&staging).unwrap();
    let (st, out, el) = sh(
        "/usr/bin/tar",
        &[
            "-xzf",
            cache.to_str().unwrap(),
            "-C",
            staging.to_str().unwrap(),
        ],
    );
    eprintln!("  bsdtar -xzf: {st} in {el:?} {}", out.trim());
    assert!(st.success(), "system tar failed");
    let src = staging.join(MYSQL_TOP);

    // xattrs as they land from the archive, BEFORE anything strips them.
    let mysqld_staged = src.join("bin/mysqld");
    let (_, x, _) = sh("/usr/bin/xattr", &["-l", mysqld_staged.to_str().unwrap()]);
    eprintln!("  xattr -l on staged bin/mysqld: {:?}", x.trim());
    let (_, xr, _) = sh(
        "/usr/bin/xattr",
        &["-r", "-l", src.join("bin").to_str().unwrap()],
    );
    eprintln!(
        "  xattr -r -l on staged bin/ -> {} line(s): {:?}",
        xr.lines().count(),
        xr.lines().take(5).collect::<Vec<_>>()
    );

    // The atomic rename our pipeline performs (layout::install_dir).
    let final_dir = home.path().join("packages/mysql/8.4/8.4.11");
    std::fs::create_dir_all(final_dir.parent().unwrap()).unwrap();
    std::fs::rename(&src, &final_dir).unwrap();
    let mysqld = final_dir.join("bin/mysqld");
    eprintln!("\n  after atomic rename: {}", final_dir.display());

    use std::os::unix::fs::PermissionsExt;
    let md = std::fs::metadata(&mysqld).unwrap();
    eprintln!(
        "  bin/mysqld present={} size={} mode={:o}",
        mysqld.is_file(),
        md.len(),
        md.permissions().mode() & 0o7777
    );

    let (_, otool, _) = sh("/usr/bin/otool", &["-L", mysqld.to_str().unwrap()]);
    eprintln!("\n  --- otool -L bin/mysqld ---\n{otool}");
    let foreign: Vec<&str> = otool
        .lines()
        .skip(1)
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| {
            !(l.starts_with("/usr/lib/")
                || l.starts_with("/System/")
                || l.starts_with("@loader_path")
                || l.starts_with("@rpath")
                || l.starts_with("@executable_path"))
        })
        .collect();
    eprintln!("  non-system / non-relative load commands: {foreign:?}");
    let (_, rpaths, _) = sh("/usr/bin/otool", &["-l", mysqld.to_str().unwrap()]);
    let rp: Vec<&str> = rpaths
        .lines()
        .filter(|l| l.contains("path ") && l.contains("(offset"))
        .map(|l| l.trim())
        .collect();
    eprintln!("  LC_RPATH entries: {rp:?}");

    let (cs_st, cs, _) = sh("/usr/bin/codesign", &["-dv", mysqld.to_str().unwrap()]);
    eprintln!("\n  --- codesign -dv bin/mysqld (status {cs_st}) ---\n{cs}");
    let (_, csv, _) = sh(
        "/usr/bin/codesign",
        &["-v", "--verbose=2", mysqld.to_str().unwrap()],
    );
    eprintln!("  codesign -v: {}", csv.trim());

    let (_, x2, _) = sh("/usr/bin/xattr", &["-l", mysqld.to_str().unwrap()]);
    eprintln!("  xattr -l after rename: {:?}", x2.trim());
    let quarantined = x2.contains("com.apple.quarantine");
    eprintln!("  com.apple.quarantine present: {quarantined}");

    banner("Q3 — first vs second exec (Gatekeeper / notarization scan cost)");
    let (s1, o1, e1) = sh(mysqld.to_str().unwrap(), &["--version"]);
    eprintln!("  1st `mysqld --version`: {s1} in {e1:?}  -> {}", o1.trim());
    let (s2, o2, e2) = sh(mysqld.to_str().unwrap(), &["--version"]);
    eprintln!("  2nd `mysqld --version`: {s2} in {e2:?}  -> {}", o2.trim());
    let (s3, _, e3) = sh(mysqld.to_str().unwrap(), &["--version"]);
    eprintln!("  3rd `mysqld --version`: {s3} in {e3:?}");
    eprintln!(
        "  first-exec penalty: {:?} ({:.0}x the warm run)",
        e1.saturating_sub(e2),
        e1.as_secs_f64() / e2.as_secs_f64().max(0.000_001)
    );

    let (sp_st, sp, _) = sh("/usr/sbin/spctl", &["-a", "-vvv", mysqld.to_str().unwrap()]);
    eprintln!("  spctl -a -vvv: {sp_st} {}", sp.trim());

    // A SECOND, independent copy: does the rename itself matter, or is the
    // scan keyed to the file content regardless of path?
    banner("Q3 — control: a second fresh extraction at a different path");
    let staging2 = home.path().join("packages/.staging/y/root");
    std::fs::create_dir_all(&staging2).unwrap();
    let (st2, _, el2) = sh(
        "/usr/bin/tar",
        &[
            "-xzf",
            cache.to_str().unwrap(),
            "-C",
            staging2.to_str().unwrap(),
        ],
    );
    eprintln!("  bsdtar again: {st2} in {el2:?}");
    let mysqld2 = staging2.join(MYSQL_TOP).join("bin/mysqld");
    let (sa, _, ea) = sh(mysqld2.to_str().unwrap(), &["--version"]);
    eprintln!("  1st exec of the SECOND copy (no rename): {sa} in {ea:?}");
    let (sb, _, eb) = sh(mysqld2.to_str().unwrap(), &["--version"]);
    eprintln!("  2nd exec of the SECOND copy: {sb} in {eb:?}");

    // THE design question: can we pre-pay the first-exec cost DURING install,
    // in staging, so the user never eats it on "Start MySQL"? Exec once in
    // staging, then perform the atomic rename, then exec again. If the
    // post-rename exec is warm, the scan survives the rename and the cost is
    // pre-payable behind the install progress bar.
    banner("Q3 — can the first-exec cost be pre-paid in staging, before the rename?");
    let staging3 = home.path().join("packages/.staging/z/root");
    std::fs::create_dir_all(&staging3).unwrap();
    let (st3, _, el3) = sh(
        "/usr/bin/tar",
        &[
            "-xzf",
            cache.to_str().unwrap(),
            "-C",
            staging3.to_str().unwrap(),
        ],
    );
    eprintln!("  bsdtar third copy: {st3} in {el3:?}");
    let src3 = staging3.join(MYSQL_TOP);
    let warm = src3.join("bin/mysqld");
    let (wa_s, _, wa) = sh(warm.to_str().unwrap(), &["--version"]);
    eprintln!("  warm-up exec IN STAGING:      {wa_s} in {wa:?}");
    let final3 = home.path().join("packages/mysql/8.4/8.4.11-warm");
    std::fs::rename(&src3, &final3).unwrap();
    let after = final3.join("bin/mysqld");
    let (af_s, _, af) = sh(after.to_str().unwrap(), &["--version"]);
    eprintln!("  1st exec AFTER the atomic rename: {af_s} in {af:?}");
    let (af2_s, _, af2) = sh(after.to_str().unwrap(), &["--version"]);
    eprintln!("  2nd exec after the rename:        {af2_s} in {af2:?}");
    eprintln!(
        "  => pre-paying in staging {} the post-rename first exec",
        if af.as_millis() < 100 {
            "DOES cover"
        } else {
            "does NOT cover"
        }
    );

    banner("Q3 — decompression throughput: our flate2 pass vs system gzip/bsdtar");
    let raw = std::fs::read(&cache).unwrap();
    let t = Instant::now();
    let n = read_archive(&raw).len();
    let ours = t.elapsed();
    eprintln!(
        "  flate2 (miniz_oxide) header walk of {n} entries over 669647849 decompressed bytes: \
         {ours:?} => {:.0} MB/s",
        669_647_849.0 / 1_048_576.0 / ours.as_secs_f64()
    );
    eprintln!("  NOTE: our extractor does TWO such passes (plan + materialize), so budget ~2x.");
    let (gz_s, _, gz) = sh(
        "/bin/sh",
        &[
            "-c",
            &format!("/usr/bin/gzip -dc {} > /dev/null", cache.display()),
        ],
    );
    eprintln!(
        "  system `gzip -dc > /dev/null`: {gz_s} in {gz:?} => {:.0} MB/s",
        669_647_849.0 / 1_048_576.0 / gz.as_secs_f64()
    );

    banner("Q3 — our my.cnf template against this binary");
    // openvhost-conf is not a dependency of openvhost-pkg, so render the real
    // template file with a naive substitution rather than adding a dep.
    let tpl_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../openvhost-conf/templates/mysql/my.cnf.tera");
    let tpl = std::fs::read_to_string(&tpl_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", tpl_path.display()));
    let datadir = home.path().join("data/mysql/8.4");
    let confd = home.path().join("config/custom/mysql/8.4/conf.d");
    let rundir = home.path().join("run");
    std::fs::create_dir_all(&datadir).unwrap();
    std::fs::create_dir_all(&confd).unwrap();
    std::fs::create_dir_all(&rundir).unwrap();
    let rendered = tpl
        .replace("{{ datadir }}", datadir.to_str().unwrap())
        .replace(
            "{{ socket }}",
            rundir.join("mysql-8.4.sock").to_str().unwrap(),
        )
        .replace(
            "{{ pid_file }}",
            rundir.join("mysql-8.4.pid").to_str().unwrap(),
        )
        .replace("{{ custom_confd }}", confd.to_str().unwrap());
    let cnf = home.path().join("my.cnf");
    std::fs::write(&cnf, &rendered).unwrap();
    eprintln!("  --- rendered my.cnf ---\n{rendered}");
    eprintln!(
        "  socket path bytes: {} (sun_path ceiling is 103)",
        rundir.join("mysql-8.4.sock").to_str().unwrap().len()
    );
    let (vs, vo, ve) = sh(
        mysqld.to_str().unwrap(),
        &[
            &format!("--defaults-file={}", cnf.display()),
            "--validate-config",
        ],
    );
    eprintln!("  mysqld --validate-config: {vs} in {ve:?}\n{vo}");

    banner("Q3 — a few sibling binaries (do they all run?)");
    for b in ["mysql", "mysqladmin", "mysqld_safe", "mysql_config"] {
        let p = final_dir.join("bin").join(b);
        if !p.exists() {
            eprintln!("  {b}: ABSENT");
            continue;
        }
        let (s, o, e) = sh(p.to_str().unwrap(), &["--version"]);
        eprintln!("  {b}: {s} in {e:?} -> {}", o.lines().next().unwrap_or(""));
    }

    eprintln!("\n  disk footprint of the installed tree:");
    let (_, du, _) = sh("/usr/bin/du", &["-sh", final_dir.to_str().unwrap()]);
    eprintln!("  {}", du.trim());
}

// ---------------------------------------------------------------------------
// Q4 — MariaDB battery. Only meaningful if Q1 found an arm64 macOS tarball.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn q4_mariadb_battery() {
    if !gated() {
        return;
    }
    banner("Q4 — MariaDB battery");
    eprintln!(
        "  SKIPPED BY CONSTRUCTION: Q1 must first produce a macOS arm64 MariaDB\n  \
         tarball URL to point this at. See q1_macos_arm64_tarball_availability."
    );
}
