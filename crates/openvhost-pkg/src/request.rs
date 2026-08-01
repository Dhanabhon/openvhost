// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated pipeline inputs. ALL boundary validation lives here so a future
//! (untrusted) manifest layer cannot smuggle a traversal or a non-https URL
//! past `InstallRequest::new` / `validate_component` (spec F20, S1).

use std::path::{Path, PathBuf};

use crate::error::PkgError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

/// A validated request to install one package version. Constructed ONLY via
/// [`InstallRequest::new`], which enforces every boundary check (safe path
/// components, https-only/no-userinfo/no-IP-literal URL, well-formed
/// SHA-256) — fields are `pub(crate)` specifically so nothing outside this
/// crate can construct one by struct literal and bypass `::new` (S1/F20).
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub(crate) name: String,
    pub(crate) major: String,
    pub(crate) version: String,
    pub(crate) url: url::Url,
    pub(crate) sha256: String,
    pub(crate) format: ArchiveFormat,
    /// Optional: a binary inside the package tree to exec once in staging so
    /// macOS pays its first-execution signature check during the install
    /// rather than on the user's first "Start" (see
    /// [`InstallRequest::with_warmup_binary`]). `None` — the default — skips
    /// the step entirely. Package-specific by design: this pipeline carries
    /// nginx and PHP as well as MySQL, and each names its own (or none).
    pub(crate) warmup_bin: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Started { total: Option<u64> },
    Downloaded { bytes: u64 },
    Verified,
    Extracted,
    Linked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub dir: PathBuf,
    pub current_link: PathBuf,
    pub name: String,
    pub major: String,
    pub version: String,
}

/// Filesystem root for installed packages. Minted ONLY from core's home
/// resolution — never from IPC/webview input (S/F23): a future Tauri command
/// physically cannot hand this constructor an arbitrary path.
#[derive(Debug, Clone)]
pub struct PackagesRoot(PathBuf);

impl PackagesRoot {
    pub fn from_home(home: &Path) -> Self {
        Self(home.join("packages"))
    }
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub fn staging_root(&self) -> PathBuf {
        self.0.join(".staging")
    }
    pub fn major_dir(&self, name: &str, major: &str) -> PathBuf {
        self.0.join(name).join(major)
    }
    pub fn package_dir(&self, name: &str, major: &str, version: &str) -> PathBuf {
        self.major_dir(name, major).join(version)
    }
    pub fn current_link(&self, name: &str, major: &str) -> PathBuf {
        self.major_dir(name, major).join("current")
    }
}

/// Reserved Windows device basenames (case-insensitive), checked without a
/// trailing extension. Shared crate-wide: `validate_component` (below) uses
/// it for name/major/version components, and `extract::validate` imports it
/// for archive entry-name validation (S11) — ONE list, no duplication.
pub(crate) const RESERVED: [&str; 24] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Validate a name/major/version as a safe SINGLE path component (F20).
pub(crate) fn validate_component(s: &str) -> Result<(), PkgError> {
    let bad = |reason: &'static str| PkgError::InvalidComponent {
        value: s.to_string(),
        reason,
    };
    if s.is_empty() || s.len() > 64 {
        return Err(bad("length must be 1..=64"));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(bad("only [a-z0-9._-] allowed"));
    }
    if s == "." || s == ".." {
        return Err(bad("must not be . or .."));
    }
    if s.starts_with('.') || s.starts_with('-') {
        return Err(bad("must not start with . or -"));
    }
    // A trailing space is already impossible here: the charset check above
    // only allows `[a-z0-9._-]`, which excludes ' ' outright.
    if s.ends_with('.') {
        return Err(bad("must not end with a dot"));
    }
    // Reserved Windows device basename (before the first dot), case-insensitive.
    let stem = s.split('.').next().unwrap_or(s);
    if RESERVED.contains(&stem) {
        return Err(bad("reserved device name"));
    }
    Ok(())
}

/// Validate a URL as an acceptable download target (S1): https only, host
/// present, no userinfo, no IP-literal host. Called at request build time
/// (`InstallRequest::new`) AND on every redirect hop (`download.rs` reuses
/// this) — ONE validator, ONE set of rules, for both trust-boundary
/// crossings.
///
/// Debug builds additionally accept plain `http` to a loopback host (S2),
/// so hermetic tests need no TLS; `#[cfg(debug_assertions)]` compiles this
/// carve-out out entirely in release, so production builds are https-only
/// with no exception, regardless of caller.
pub(crate) fn validate_https_url(u: &url::Url) -> Result<(), PkgError> {
    #[cfg(debug_assertions)]
    {
        if u.scheme() == "http"
            && let Some(host) = u.host_str()
            && (host == "127.0.0.1" || host == "localhost" || host == "[::1]")
        {
            return Ok(());
        }
    }
    if u.scheme() != "https" {
        return Err(PkgError::InvalidUrl("scheme must be https"));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(PkgError::InvalidUrl("url must not contain userinfo"));
    }
    match u.host() {
        None => Err(PkgError::InvalidUrl("url must have a host")),
        Some(url::Host::Domain(_)) => Ok(()),
        Some(_) => Err(PkgError::InvalidUrl(
            "url host must be a domain, not an IP literal",
        )),
    }
}

/// Longest accepted warm-up path, in bytes. Matches the extractor's own
/// per-entry cap (`extract::validate::MAX_REL_BYTES`) in value but is
/// deliberately an independent constant: a path this crate will hand to
/// `exec` is a different trust question from a path it will `create`, and
/// coupling them would mean one relaxing silently relaxes the other.
const MAX_WARMUP_REL_BYTES: usize = 240;
/// Deepest accepted warm-up path, in components. `bin/mysqld` is 2;
/// `Contents/MacOS/foo` is 3. Nothing legitimate needs 8.
const MAX_WARMUP_DEPTH: usize = 8;

/// Validate a warm-up binary path: a `/`-separated RELATIVE path inside the
/// installed package tree, e.g. `bin/mysqld`.
///
/// This is a parse-only charset+shape guard on a value that later becomes
/// `argv[0]` of a real `exec`, so it is deliberately narrow: ASCII
/// `[A-Za-z0-9._-]` per component plus `/` separators, no `.`/`..`/empty
/// component, no absolute path, no backslash, no NUL, no leading `-` on any
/// component (so a component can never be mistaken for a flag by anything
/// downstream that re-splits the path). Containment is re-checked against
/// the real, symlink-resolved staging tree at warm-up time — this check is
/// the cheap first gate, not the only one.
fn validate_warmup_rel(s: &str) -> Result<PathBuf, PkgError> {
    let bad = |reason: &'static str| PkgError::InvalidWarmupPath {
        value: s.to_string(),
        reason,
    };
    if s.is_empty() {
        return Err(bad("must not be empty"));
    }
    if s.len() > MAX_WARMUP_REL_BYTES {
        return Err(bad("path too long"));
    }
    if s.contains('\0') {
        return Err(bad("must not contain NUL"));
    }
    if s.contains('\\') {
        return Err(bad("must not contain a backslash"));
    }
    if s.starts_with('/') {
        return Err(bad("must be relative, not absolute"));
    }
    let comps: Vec<&str> = s.split('/').collect();
    if comps.len() > MAX_WARMUP_DEPTH {
        return Err(bad("too many path components"));
    }
    for c in &comps {
        if c.is_empty() {
            return Err(bad("empty path component"));
        }
        if *c == "." || *c == ".." {
            return Err(bad("must not contain a . or .. component"));
        }
        if c.starts_with('-') {
            return Err(bad("path component must not start with '-'"));
        }
        if !c
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(bad("only [A-Za-z0-9._-] allowed in path components"));
        }
    }
    // Built component-by-component rather than `PathBuf::from(s)` so the
    // separator is the platform's, and so nothing but the components this
    // loop just validated can end up in the result.
    Ok(comps.iter().collect())
}

fn validate_sha256(s: &str) -> Result<(), PkgError> {
    if s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(PkgError::InvalidSha256)
    }
}

impl InstallRequest {
    pub fn new(
        name: &str,
        major: &str,
        version: &str,
        url: &str,
        sha256: &str,
        format: ArchiveFormat,
    ) -> Result<Self, PkgError> {
        validate_component(name)?;
        validate_component(major)?;
        validate_component(version)?;
        validate_sha256(sha256)?;
        let parsed = url::Url::parse(url).map_err(|_| PkgError::InvalidUrl("unparseable url"))?;
        validate_https_url(&parsed)?;
        Ok(Self {
            name: name.to_string(),
            major: major.to_string(),
            version: version.to_string(),
            url: parsed,
            sha256: sha256.to_string(),
            format,
            warmup_bin: None,
        })
    }

    /// Name a binary inside the package tree — a relative, `/`-separated
    /// path such as `bin/mysqld` — to exec once, in staging, immediately
    /// before the atomic rename.
    ///
    /// **Why:** macOS charges a Gatekeeper/XProtect signature check on the
    /// FIRST execution of a new binary. Measured on a freshly extracted
    /// 67 MB `mysqld`: 1.877 s cold, 13.7 ms warm (137x) — and the
    /// validation survives `rename(2)`, so paying it in staging (749 ms)
    /// makes the first exec after the rename warm (13.6 ms). Spending it
    /// here puts it behind the progress the user is already watching instead
    /// of turning their first "Start MySQL" into an unexplained pause.
    ///
    /// **This is an optimization and only an optimization.** A warm-up that
    /// is missing, not executable, exits non-zero, rejects `--version`,
    /// hangs, or is killed for outliving its budget is logged and ignored —
    /// it can never fail an install. An [`InstallRequest`] that never calls
    /// this skips the step entirely.
    ///
    /// Returns [`PkgError::InvalidWarmupPath`] if `rel_path` is not a safe
    /// relative path (see the boundary rules at the top of this module —
    /// this value ends up as `argv[0]` of a real `exec`).
    pub fn with_warmup_binary(mut self, rel_path: &str) -> Result<Self, PkgError> {
        self.warmup_bin = Some(validate_warmup_rel(rel_path)?);
        Ok(self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_clean_request() {
        let r = InstallRequest::new(
            "php",
            "8.4",
            "8.4.23",
            "https://www.php.net/distributions/php-8.4.23.tar.gz",
            "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
            ArchiveFormat::TarGz,
        )
        .unwrap();
        assert_eq!(r.name, "php");
        assert_eq!(r.version, "8.4.23");
    }

    #[test]
    fn rejects_http_url() {
        assert!(
            InstallRequest::new(
                "php",
                "8.4",
                "8.4.23",
                "http://www.php.net/x.tar.gz",
                "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
                ArchiveFormat::TarGz,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_userinfo_url() {
        assert!(
            InstallRequest::new(
                "php",
                "8.4",
                "8.4.23",
                "https://user:pw@evil.com/x.tar.gz",
                "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
                ArchiveFormat::TarGz,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_ip_literal_host() {
        assert!(
            InstallRequest::new(
                "php",
                "8.4",
                "8.4.23",
                "https://127.0.0.1/x.tar.gz",
                "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
                ArchiveFormat::TarGz,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_bad_sha() {
        for bad in [
            "",
            "ABCDEF",
            &"f".repeat(63),
            &"F".repeat(64),
            &"g".repeat(64),
        ] {
            assert!(
                InstallRequest::new(
                    "php",
                    "8.4",
                    "8.4.23",
                    "https://x.example/x.tar.gz",
                    bad,
                    ArchiveFormat::TarGz
                )
                .is_err(),
                "should reject sha {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_dangerous_components() {
        for bad in [
            ".",
            "..",
            ".staging",
            "-rf",
            "php/evil",
            "com1",
            "nul",
            "lpt8",
            "lpt9",
            "lpt8.log",
            "a ",
            "a.",
            "É",
            &"a".repeat(65),
        ] {
            assert!(
                InstallRequest::new(
                    bad,
                    "8.4",
                    "8.4.23",
                    "https://x.example/x.tar.gz",
                    &"a".repeat(64),
                    ArchiveFormat::TarGz
                )
                .is_err(),
                "should reject name {bad:?}"
            );
        }
    }

    fn req() -> InstallRequest {
        InstallRequest::new(
            "mysql",
            "8.4",
            "8.4.11",
            "https://cdn.mysql.example/mysql.tar.gz",
            &"a".repeat(64),
            ArchiveFormat::TarGz,
        )
        .unwrap()
    }

    #[test]
    fn a_request_names_no_warmup_binary_by_default() {
        assert_eq!(req().warmup_bin, None);
    }

    #[test]
    fn accepts_real_warmup_binary_paths() {
        for good in [
            "bin/mysqld",
            "sbin/nginx",
            "bin/php-fpm",
            "mysqld",
            "Contents/MacOS/Some_App.helper",
            "bin/php8.4",
        ] {
            let r = req()
                .with_warmup_binary(good)
                .unwrap_or_else(|e| panic!("should accept {good:?}: {e}"));
            assert_eq!(
                r.warmup_bin,
                Some(good.split('/').collect::<PathBuf>()),
                "{good:?} must round-trip component-for-component"
            );
        }
    }

    #[test]
    fn rejects_dangerous_warmup_binary_paths() {
        for bad in [
            "",
            "/bin/sh",
            "../../../bin/sh",
            "bin/../../sh",
            "bin/./sh",
            "bin//sh",
            "bin/",
            "bin\\sh",
            "bin/sh\0",
            "bin/-rf",
            "-rf",
            "bin/my sqld",
            "bin/mysqld;rm",
            "bin/$(whoami)",
            "bin/mysqld\n--defaults-file=/etc/shadow",
            "a/b/c/d/e/f/g/h/i",
            &"a".repeat(241),
        ] {
            let err = req()
                .with_warmup_binary(bad)
                .err()
                .unwrap_or_else(|| panic!("should reject warm-up path {bad:?}"));
            assert!(
                matches!(err, PkgError::InvalidWarmupPath { .. }),
                "wrong variant for {bad:?}: {err:?}"
            );
        }
    }

    #[test]
    fn packages_root_paths() {
        let root = PackagesRoot::from_home(std::path::Path::new("/home/u/.openvhost"));
        assert_eq!(
            root.as_path(),
            std::path::Path::new("/home/u/.openvhost/packages")
        );
        assert_eq!(
            root.staging_root(),
            std::path::Path::new("/home/u/.openvhost/packages/.staging")
        );
        assert_eq!(
            root.package_dir("php", "8.4", "8.4.23"),
            std::path::Path::new("/home/u/.openvhost/packages/php/8.4/8.4.23")
        );
        assert_eq!(
            root.current_link("php", "8.4"),
            std::path::Path::new("/home/u/.openvhost/packages/php/8.4/current")
        );
    }
}
