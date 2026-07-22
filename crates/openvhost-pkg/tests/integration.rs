// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end pipeline over a hermetic local HTTP server: real tar.gz built
//! in-test, downloaded, verified, extracted, installed, linked.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpListener;

use openvhost_pkg::{ArchiveFormat, InstallRequest, PackagesRoot, Progress, install_package};

fn targz(files: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    let gz = GzEncoder::new(Vec::new(), Compression::fast());
    let mut ar = tar::Builder::new(gz);
    // single top dir "pkg/" so the strip rule is exercised
    let mut d = tar::Header::new_gnu();
    d.set_size(0);
    d.set_entry_type(tar::EntryType::Directory);
    d.set_mode(0o755);
    d.set_cksum();
    ar.append_data(&mut d, "pkg/", std::io::empty()).unwrap();
    for (name, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        ar.append_data(&mut h, format!("pkg/{name}"), *data)
            .unwrap();
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
            let mut b = [0u8; 1024];
            let _ = s.read(&mut b);
            let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(hdr.as_bytes());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}/pkg.tar.gz")
}

/// Build an `InstallRequest` for the loopback `http://` server above without
/// going through `InstallRequest::new`. That constructor enforces
/// https-only (S1) with no debug/loopback carve-out — only `download.rs`'s
/// redirect-hop check has one (S2) — so a real hermetic loopback server can
/// never satisfy it. `InstallRequest`'s fields are all `pub` specifically so
/// callers who already hold a validated/trusted URL can construct one
/// directly; the scheme/userinfo/IP-literal checks this bypasses are
/// independently covered by `request.rs`'s own unit tests
/// (`rejects_http_url`, `rejects_userinfo_url`, `rejects_ip_literal_host`).
/// This test exercises the download→verify→extract→install→link pipeline
/// that runs after that boundary, over a real, unmocked local HTTP server.
fn local_request(
    name: &str,
    major: &str,
    version: &str,
    url: &str,
    sha256: &str,
) -> InstallRequest {
    InstallRequest {
        name: name.to_string(),
        major: major.to_string(),
        version: version.to_string(),
        url: url::Url::parse(url).unwrap(),
        sha256: sha256.to_string(),
        format: ArchiveFormat::TarGz,
    }
}

// dev-build note: `local_request` bypasses `InstallRequest::new`'s https-only
// check for request construction; the download step inside `install_package`
// still runs `download.rs`'s own scheme check, which accepts loopback
// `http://` only under `debug_assertions` (S2). `cargo test` builds debug,
// so this hermetic test exercises a real, unmocked HTTP connection end to end.
#[tokio::test]
async fn installs_targz_end_to_end() {
    let archive = targz(&[("main.c", b"int main;"), ("bin/php", b"#!/bin/sh")]);
    let sha = sha_hex(&archive);
    let url = serve_once(archive);

    let home = tempfile::Builder::new()
        .prefix("ovh-int")
        .tempdir_in("/tmp")
        .unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();

    let req = local_request("php", "8.4", "8.4.99", &url, &sha);
    let mut events = Vec::new();
    let installed = install_package(&req, &root, |p| events.push(p))
        .await
        .unwrap();

    assert!(installed.dir.join("main.c").is_file());
    assert!(installed.dir.join("bin/php").is_file());
    assert_eq!(
        std::fs::read_link(&installed.current_link)
            .unwrap()
            .to_str()
            .unwrap(),
        "8.4.99"
    );
    assert!(events.contains(&Progress::Verified));
    assert!(events.contains(&Progress::Extracted));
    assert!(events.contains(&Progress::Linked));
    // staging swept clean
    let mut staging_entries = std::fs::read_dir(root.staging_root()).unwrap();
    assert!(
        staging_entries.next().is_none(),
        "staging must be empty after success"
    );
}

#[tokio::test]
async fn second_install_is_already_installed() {
    let archive = targz(&[("x", b"y")]);
    let sha = sha_hex(&archive);
    let url = serve_once(archive.clone());
    let home = tempfile::Builder::new()
        .prefix("ovh-int2")
        .tempdir_in("/tmp")
        .unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let req = local_request("php", "8.4", "8.4.98", &url, &sha);
    install_package(&req, &root, |_| {}).await.unwrap();
    // second attempt (dest exists) — no server needed, pre-check fires
    let req2 = local_request("php", "8.4", "8.4.98", req.url.as_str(), &sha);
    let err = install_package(&req2, &root, |_| {}).await.unwrap_err();
    assert!(matches!(
        err,
        openvhost_pkg::PkgError::AlreadyInstalled { .. }
    ));
}
