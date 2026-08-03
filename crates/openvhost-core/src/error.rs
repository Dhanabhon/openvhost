// SPDX-License-Identifier: GPL-3.0-or-later
//! Core error type (thiserror in library crates — master plan §5).

use std::path::PathBuf;

/// Errors produced by openvhost-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The user home directory could not be determined and no
    /// `OPENVHOST_HOME` override was provided.
    #[error("could not determine the user home directory (set OPENVHOST_HOME to override)")]
    HomeDirUnavailable,
    /// A filesystem operation failed while provisioning.
    #[error("provision: {op} {}: {source}", path.display())]
    ProvisionIo {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The php-fpm unix socket path would exceed Darwin's 104-byte
    /// `sun_path`. php-fpm does NOT reject longer paths — it warns, silently
    /// truncates, and binds the wrong path while nginx 502s forever
    /// (specialist-proven). Refuse early instead.
    #[error("socket path {} is {len} bytes (max 103); use a shorter OPENVHOST_HOME", path.display())]
    SocketPathTooLong { path: PathBuf, len: usize },
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    /// A domain value failed validation at the boundary (parse-don't-validate).
    #[error("invalid {field}: {reason}")]
    Validation { field: &'static str, reason: String },
    /// A filesystem write outside the provisioning path (which already has
    /// its own [`CoreError::ProvisionIo`]) and outside the site-apply plan
    /// (which has its own `site::apply::ApplyError::Io`). Added for the
    /// MySQL init sequence's `my.cnf` write (P1 MySQL lifecycle design, spec
    /// D5: "written with `atomicfile::write_atomic` as a `GeneratedFile`"),
    /// which needs the SAME hardened atomic write `site::apply::commit`
    /// already uses but has no `ApplyPlan` of its own to go through — see
    /// `crate::mysql::write_generated_config`. A second use site joined it
    /// with audit finding H1: `crate::db::Db::open`'s own precreate/chmod
    /// calls that pin `state.db` (and its WAL sidecars) to 0600 are the
    /// identical shape — a filesystem op with no `ApplyPlan`/provisioning
    /// context of its own to report through.
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A path this crate needs to read is not a plain file — refused
    /// rather than followed (a symlink) or silently skipped. Added for
    /// `logs::read::read_window` (P1 live-log-viewer design, spec D5):
    /// mirrors `site::apply::ApplyError::NotAPlainFile`'s reasoning
    /// applied to a log path. A path derived by `logs::LogPaths` is safe
    /// by construction, but the FILE actually sitting there could have
    /// been replaced with a link to something outside `<home>/logs` after
    /// the fact — refusing it here means that swap is caught even though
    /// this crate never calls `canonicalize`.
    #[error("{} is not a plain file (found {found}); refusing to read it", path.display())]
    NotAPlainFile {
        path: PathBuf,
        /// What was actually found there instead — `"a symlink"`,
        /// `"a directory"`, or `"a special file"`.
        found: &'static str,
    },
    /// This build publishes no verified package for the requested runtime on
    /// this host (MySQL-from-tarball design D2).
    ///
    /// Deliberately NOT [`CoreError::Validation`]: the request is well formed
    /// and the version may even be one this build otherwise offers — we simply
    /// have no artifact whose provenance was checked for that architecture,
    /// and shipping an unverified pin so the table looks symmetrical is
    /// exactly what golden rule 6 exists to prevent. The user gets an honest
    /// refusal instead of another architecture's binaries.
    #[error("this build has no verified {name} {version} package for {target}")]
    NoPackageForTarget {
        /// The package tree name, e.g. `"mysql"`.
        name: &'static str,
        /// The version or series that was asked for.
        version: String,
        /// The target that could not be served — a
        /// [`crate::PackageTarget::as_str`] value, or `"this host"` when this
        /// binary was built for a platform the programme publishes nothing
        /// for.
        target: &'static str,
    },
    /// The catalogue pins a package whose release has not been published yet,
    /// so the URL it names does not exist (build-pipeline design D5:
    /// "publishing is owner-gated").
    ///
    /// Deliberately NOT a download failure. The bytes exist and were built and
    /// audited; what is missing is the outward-facing act of publishing them,
    /// which only the owner may perform. Letting the pin reach the downloader
    /// instead would turn a known, stated gap into a 404 that looks like a
    /// network fault — and a user would have no way to tell the two apart.
    /// Refused before any network or filesystem work, and the message names
    /// the release a human has to create.
    #[error(
        "{name} {version} is pinned at a release that does not exist yet ({url}); \
         publish release {tag} before this build can install it"
    )]
    PackageNotPublished {
        /// The package tree name, e.g. `"mariadb"`.
        name: &'static str,
        /// The exact version the catalogue pins.
        version: &'static str,
        /// The release tag that must exist, e.g. `"mariadb-11.4.9"`.
        tag: &'static str,
        /// The URL the release will serve once it is published.
        url: &'static str,
    },
    /// A package download, verification, extraction or install failed.
    ///
    /// Wraps `openvhost-pkg`'s typed error rather than flattening it to a
    /// string: the desktop layer has to be able to tell a SHA-256 mismatch
    /// from a network fault, because a payload that failed verification and a
    /// download that timed out must not look the same to a user.
    #[error("package: {0}")]
    Package(#[from] openvhost_pkg::PkgError),
}

/// Maps the crate-shared hardened atomic write's error (`crate::atomicfile`)
/// into [`CoreError::Io`] — mirrors `site::apply::error`'s identical
/// `From<AtomicWriteError> for ApplyError` (a manual impl, not `#[from]`,
/// because the fields are remapped, not wrapped as-is).
impl From<crate::atomicfile::AtomicWriteError> for CoreError {
    fn from(e: crate::atomicfile::AtomicWriteError) -> Self {
        CoreError::Io {
            op: e.op,
            path: e.path,
            source: e.source,
        }
    }
}
