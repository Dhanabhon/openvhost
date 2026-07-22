// SPDX-License-Identifier: GPL-3.0-or-later
//! Pipeline orchestration: sweep → stage → download+verify → extract →
//! atomic install → current link. Single in-process install at a time (S25).

use std::path::Path;
use std::sync::OnceLock;

use tokio::sync::Semaphore;

use crate::download::download_and_verify;
use crate::error::PkgError;
use crate::extract;
use crate::layout::{self, Staging};
use crate::request::{ArchiveFormat, InstallRequest, InstalledPackage, PackagesRoot, Progress};

/// Process-wide install gate (S25): a single permit, so at most one
/// [`install_package`] call is ever mid-flight in this process at a time —
/// concurrent callers queue here rather than racing the shared staging root
/// and sweeper. Nothing in this crate ever calls `close()` on it, so
/// `acquire` never actually fails in practice; the call site still maps a
/// hypothetical closed-gate error onto [`PkgError::Internal`] instead of
/// `expect()`, per repo rule (no `unwrap()`/`expect()` outside
/// `#[cfg(test)]` — also enforced here by the workspace's
/// `clippy::unwrap_used`/`expect_used` lints under `-D warnings`).
fn install_gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(1))
}

fn io_err(op: &'static str, path: &Path, source: std::io::Error) -> PkgError {
    PkgError::Io {
        op,
        path: path.to_path_buf(),
        source,
    }
}

/// Install `req` under `root`.
///
/// Pipeline: pre-check the destination version directory doesn't already
/// exist, sweep abandoned staging directories older than 24h (S20), stage a
/// fresh private working directory, download the archive and verify its
/// SHA-256 (S8 — the SAME open, verified `File` handle returned by the
/// download stage is threaded straight into extraction on the blocking
/// pool, and handed back out again, never re-opened by path), extract
/// through the hardened per-format walk, atomically rename the extracted
/// tree into its final `packages/<name>/<major>/<version>/` location (S21),
/// then swing the per-major `current` link onto it (S22).
///
/// `progress` receives [`Progress`] events as the pipeline advances
/// (`Started`/`Downloaded` during the fetch, then `Verified`, `Extracted`,
/// `Linked`).
///
/// At most one install runs at a time process-wide (S25); concurrent
/// callers queue on an internal semaphore rather than racing the
/// filesystem. Returns [`PkgError::AlreadyInstalled`] immediately — before
/// any network or filesystem staging work happens for this call — if the
/// destination version directory already exists.
pub async fn install_package(
    req: &InstallRequest,
    root: &PackagesRoot,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<InstalledPackage, PkgError> {
    let _permit = install_gate()
        .acquire()
        .await
        .map_err(|_| PkgError::Internal("install semaphore closed unexpectedly".to_string()))?;

    let final_dir = root.package_dir(&req.name, &req.major, &req.version);
    if final_dir.exists() {
        return Err(PkgError::AlreadyInstalled {
            name: req.name.clone(),
            version: req.version.clone(),
        });
    }
    layout::sweep_stale(root);

    let staging = Staging::create(root)?;
    let staging_path = staging.path().to_path_buf();
    let extract_root = staging_path.join("root");
    std::fs::create_dir_all(&extract_root).map_err(|e| io_err("create_dir", &extract_root, e))?;

    // Download + verify onto the same handle we extract from (S8).
    let mut file = download_and_verify(&req.url, &req.sha256, &staging_path, &mut progress).await?;

    // Extraction is blocking CPU work; run it on the blocking pool and hand
    // the SAME file handle back out — nothing is ever re-opened by path.
    let fmt = req.format;
    let er = extract_root.clone();
    let file = tokio::task::spawn_blocking(move || -> Result<std::fs::File, PkgError> {
        match fmt {
            ArchiveFormat::TarGz => extract::targz::extract_targz(&mut file, &er)?,
            ArchiveFormat::Zip => extract::zip::extract_zip(&mut file, &er)?,
        }
        Ok(file)
    })
    .await
    .map_err(|e| PkgError::Internal(format!("extract task panicked: {e}")))??;
    drop(file);
    progress(Progress::Extracted);

    layout::install_dir(&extract_root, &final_dir, &req.name, &req.version)?;

    let link = root.current_link(&req.name, &req.major);
    layout::update_current(&link, &req.version)?;
    progress(Progress::Linked);

    Ok(InstalledPackage {
        dir: final_dir,
        current_link: link,
        name: req.name.clone(),
        major: req.major.clone(),
        version: req.version.clone(),
    })
}
