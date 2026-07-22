# P0-6 — Download → Verify → Extract Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `openvhost-pkg` downloads a pinned archive over HTTPS, verifies its SHA-256 before parsing, extracts it through hardened manual walks, and atomically installs it to `packages/<name>/<major>/<version>/` with a per-major `current` link.

**Architecture:** Six focused modules — `request` (validated inputs + `PackagesRoot` newtype), `download` (async streaming fetch + hash + caps), `extract` (validation primitives + hardened tar.gz and zip walks), `layout` (staging, atomic install, sweep), `platform` (unix symlink swap / windows junction). The extractor is treated as trusted-computing-base: every path/entry check fails closed and rejects the whole archive before any I/O.

**Tech Stack:** Rust 2024, tokio (async), reqwest (native-tls, no auto-decompress), sha2, tar+flate2, zip, tempfile, fd-lock, unicode-normalization, url, hex; junction (windows).

**Spec:** `docs/superpowers/specs/2026-07-22-p06-pkg-pipeline-design.md` — the §5 S-items (S1–S27) are normative. Every S-item traces to a consult finding and is enforced at the security-auditor merge audit. Do NOT weaken any check to make a test pass; if a check seems wrong, report it — it was reviewed by three specialists.

## Global Constraints

- Branch `feat/p06-pkg-pipeline` off current `main`.
- SPDX `// SPDX-License-Identifier: GPL-3.0-or-later` as line 1 of every new `.rs`.
- No `unwrap()`/`expect()` outside `#[cfg(test)]`; library errors are typed via `thiserror` (`PkgError`); no `anyhow` in this lib.
- `openvhost-pkg` must never depend on tauri.
- Every new dependency must pass `cargo deny check licenses advisories`; name the license in the commit body (repo rule).
- Conventional Commits, DCO-signed: always `git commit -s`.
- Gates each task: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh`.
- All extraction/name checks are cross-platform (identical on macOS/Windows/Linux) unless a step says `#[cfg]`.
- Security invariant shorthand: **verify before parse** (S8/S9), **reject-the-archive fail-closed** (S10–S18), **never re-open by path** (S8), **never `remove_dir_all` against `current`** (S22).

---

### Task 1: Crate foundation — deps, errors, validated request types, `PackagesRoot`

**Files:**
- Modify: `crates/openvhost-pkg/Cargo.toml`
- Modify: `crates/openvhost-pkg/src/lib.rs` (replace stub)
- Create: `crates/openvhost-pkg/src/error.rs`
- Create: `crates/openvhost-pkg/src/request.rs`

**Interfaces:**
- Consumes: nothing (leaf task).
- Produces (all later tasks use these):
  - `PkgError` (thiserror enum) with variants used throughout — see code.
  - `ArchiveFormat { TarGz, Zip }`
  - `InstallRequest { name, major, version, url: url::Url, sha256: String, format: ArchiveFormat }` + `InstallRequest::new(name, major, version, url: &str, sha256, format) -> Result<Self, PkgError>` (all boundary validation here).
  - `Progress { Started { total: Option<u64> }, Downloaded { bytes: u64 }, Verified, Extracted, Linked }`
  - `InstalledPackage { dir, current_link, name, major, version }`
  - `PackagesRoot(PathBuf)` with `from_home(&Path)`, `as_path()`, `staging_root()`, `major_dir(name,major)`, `package_dir(name,major,version)`, `current_link(name,major)`.
  - `pub(crate) fn validate_https_url(u: &url::Url) -> Result<(), PkgError>` (S1: scheme https, host present, no userinfo, no IP-literal host).
  - `pub(crate) fn validate_component(s: &str) -> Result<(), PkgError>` (F20: `[a-z0-9._-]{1,64}`, not `.`/`..`, not leading `.`/`-`, not a reserved device name, no trailing dot/space).

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p06-pkg-pipeline
```

- [ ] **Step 2: Dependencies**

Replace `crates/openvhost-pkg/Cargo.toml` with:

```toml
[package]
name = "openvhost-pkg"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
thiserror.workspace = true
tokio.workspace = true
reqwest = { version = "0.12", default-features = false, features = ["native-tls", "stream"] }
sha2 = "0.10"
tar = "0.4"
flate2 = "1"
zip = { version = "2", default-features = false, features = ["deflate"] }
tempfile = "3"
fd-lock = "4"
unicode-normalization = "0.1"
url = "2"
hex = "0.4"
tracing = "0.1"

[target.'cfg(target_os = "macos")'.dependencies]
xattr = "1"

[target.'cfg(windows)'.dependencies]
junction = "1"

[dev-dependencies]
tokio = { workspace = true }

[lints]
workspace = true
```

Rationale for the commit body (repo license rule): reqwest/sha2/tar/flate2/zip/tempfile/fd-lock/unicode-normalization/url/hex/tracing are MIT or MIT-OR-Apache-2.0; `junction` and `xattr` are MIT/Apache-2.0. `reqwest` uses `default-features = false` + `native-tls` (OS trust store) + `stream` ONLY — no gzip/brotli/zstd/deflate/cookies/http2/hickory features, so there is no transport auto-decompression (S3). (`futures-util` is added in Task 5 when the byte stream is first consumed.)

- [ ] **Step 3: Write `error.rs`**

`crates/openvhost-pkg/src/error.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed errors for the package pipeline (thiserror — library crate).

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("invalid path component {value:?}: {reason}")]
    InvalidComponent { value: String, reason: &'static str },
    #[error("invalid url: {0}")]
    InvalidUrl(&'static str),
    #[error("sha256 must be 64 lowercase hex characters")]
    InvalidSha256,
    #[error("network error: {0}")]
    Network(String),
    #[error("download exceeded the {cap}-byte size cap")]
    TooLarge { cap: u64 },
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("archive rejected: {0}")]
    UnsafeArchive(String),
    #[error("package {name} {version} is already installed")]
    AlreadyInstalled { name: String, version: String },
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
```

- [ ] **Step 4: Write the failing tests for `request.rs`**

Create `crates/openvhost-pkg/src/request.rs` starting with the test module (write the production skeleton in Step 5; this step is the RED phase). Put this at the BOTTOM of the file and the skeleton above it so it compiles:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_clean_request() {
        let r = InstallRequest::new(
            "php", "8.4", "8.4.23",
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
        assert!(InstallRequest::new(
            "php", "8.4", "8.4.23",
            "http://www.php.net/x.tar.gz",
            "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
            ArchiveFormat::TarGz,
        )
        .is_err());
    }

    #[test]
    fn rejects_userinfo_url() {
        assert!(InstallRequest::new(
            "php", "8.4", "8.4.23",
            "https://user:pw@evil.com/x.tar.gz",
            "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
            ArchiveFormat::TarGz,
        )
        .is_err());
    }

    #[test]
    fn rejects_ip_literal_host() {
        assert!(InstallRequest::new(
            "php", "8.4", "8.4.23",
            "https://127.0.0.1/x.tar.gz",
            "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573",
            ArchiveFormat::TarGz,
        )
        .is_err());
    }

    #[test]
    fn rejects_bad_sha() {
        for bad in ["", "ABCDEF", &"f".repeat(63), &"F".repeat(64), &"g".repeat(64)] {
            assert!(
                InstallRequest::new("php", "8.4", "8.4.23",
                    "https://x.example/x.tar.gz", bad, ArchiveFormat::TarGz).is_err(),
                "should reject sha {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_dangerous_components() {
        for bad in [".", "..", ".staging", "-rf", "php/evil", "com1", "nul", "a ", "a.", "É", &"a".repeat(65)] {
            assert!(
                InstallRequest::new(bad, "8.4", "8.4.23",
                    "https://x.example/x.tar.gz",
                    &"a".repeat(64), ArchiveFormat::TarGz).is_err(),
                "should reject name {bad:?}"
            );
        }
    }

    #[test]
    fn packages_root_paths() {
        let root = PackagesRoot::from_home(std::path::Path::new("/home/u/.openvhost"));
        assert_eq!(root.as_path(), std::path::Path::new("/home/u/.openvhost/packages"));
        assert_eq!(root.staging_root(), std::path::Path::new("/home/u/.openvhost/packages/.staging"));
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
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p openvhost-pkg request 2>&1 | tail -5`
Expected: compile error (types not yet defined) or FAIL. Proceed to implement.

- [ ] **Step 6: Implement `request.rs` (above the test module)**

```rust
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

#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub name: String,
    pub major: String,
    pub version: String,
    pub url: url::Url,
    pub sha256: String,
    pub format: ArchiveFormat,
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

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7",
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
    if !s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')) {
        return Err(bad("only [a-z0-9._-] allowed"));
    }
    if s == "." || s == ".." {
        return Err(bad("must not be . or .."));
    }
    if s.starts_with('.') || s.starts_with('-') {
        return Err(bad("must not start with . or -"));
    }
    if s.ends_with('.') || s.ends_with(' ') {
        return Err(bad("must not end with . or space"));
    }
    // Reserved Windows device basename (before the first dot), case-insensitive.
    let stem = s.split('.').next().unwrap_or(s);
    if RESERVED.contains(&stem) {
        return Err(bad("reserved device name"));
    }
    Ok(())
}

/// Validate a URL as an acceptable download target (S1): https only, host
/// present, no userinfo, no IP-literal host. Called at request build AND on
/// every redirect hop (download.rs reuses this).
pub(crate) fn validate_https_url(u: &url::Url) -> Result<(), PkgError> {
    if u.scheme() != "https" {
        return Err(PkgError::InvalidUrl("scheme must be https"));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(PkgError::InvalidUrl("url must not contain userinfo"));
    }
    match u.host() {
        None => Err(PkgError::InvalidUrl("url must have a host")),
        Some(url::Host::Domain(_)) => Ok(()),
        Some(_) => Err(PkgError::InvalidUrl("url host must be a domain, not an IP literal")),
    }
}

fn validate_sha256(s: &str) -> Result<(), PkgError> {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
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
        })
    }
}
```

- [ ] **Step 7: Write `lib.rs`**

Replace `crates/openvhost-pkg/src/lib.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-pkg — download → SHA-256 verify → extract → install pipeline.
//!
//! Responsibility (master plan §3.1): fetch a pinned (url, sha256) archive,
//! verify BEFORE parsing, extract through hardened manual walks, install
//! atomically to packages/<name>/<major>/<version>/ with a per-major
//! `current` link. The signed-manifest layer is a separate future slice that
//! produces `InstallRequest`s for this API. Security invariants: see
//! docs/superpowers/specs/2026-07-22-p06-pkg-pipeline-design.md §5.

mod error;
mod request;

pub use error::PkgError;
pub use request::{ArchiveFormat, InstallRequest, InstalledPackage, PackagesRoot, Progress};
```

(Later tasks add `mod download; mod extract; mod layout; mod platform;` and re-export `install_package`.)

- [ ] **Step 8: Run tests, gates, commit**

```bash
cargo test -p openvhost-pkg 2>&1 | tail -5
cargo fmt && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg Cargo.lock && git commit -s -m "feat(pkg): validated request types, PackagesRoot, error enum

New deps (all MIT or MIT-OR-Apache-2.0, pass cargo deny): reqwest(native-tls,stream — no auto-decompress), sha2, tar, flate2, zip, tempfile, fd-lock, unicode-normalization, url, hex, tracing; junction (windows)."
```

Expected: all tests pass; gates green (`cargo deny` covered by the workspace gate you run before PR; if you want to check now: `cargo deny check licenses advisories`).

---

### Task 2: Extraction validation primitives (the TCB core)

**Files:**
- Create: `crates/openvhost-pkg/src/extract/mod.rs` (module decl + shared types)
- Create: `crates/openvhost-pkg/src/extract/validate.rs`
- Modify: `crates/openvhost-pkg/src/lib.rs` (add `mod extract;`)

**Interfaces:**
- Consumes (Task 1): `PkgError`.
- Produces (Tasks 3–4 use these):
  - `pub(crate) enum PlannedKind { Dir, File { mode: u32 }, Symlink { target: String }, Hardlink { target: String } }`
  - `pub(crate) struct PlannedEntry { pub rel: String, pub kind: PlannedKind }`
  - `pub(crate) struct ExtractPlan { pub entries: Vec<PlannedEntry> }`
  - `pub(crate) const MAX_ENTRIES: usize = 100_000;`
  - `pub(crate) const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;`
  - `pub(crate) const MAX_DEPTH: usize = 32;`
  - `pub(crate) const MAX_REL_BYTES: usize = 240;`
  - `pub(crate) fn validate_entry_name(raw: &str) -> Result<String, PkgError>` — returns the cleaned relative path or rejects (S11).
  - `pub(crate) fn collision_key(rel: &str) -> String` — NFC + casefold (S12).
  - `pub(crate) fn strip_single_root(entries: &mut Vec<RawEntry>) -> bool` where `pub(crate) struct RawEntry { pub rel: String, pub is_dir: bool }` — applies S18 rule, returns whether a strip happened.
  - `pub(crate) fn validate_symlink_target(link_rel: &str, target: &str) -> Result<(), PkgError>` — S14 lexical rules (relative, no `.`/`..` components, resolves inside root).

- [ ] **Step 1: Write `extract/mod.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened archive extraction. The validation primitives in `validate` are
//! the trusted-computing-base: every path/entry check fails closed and the
//! whole archive is rejected on any violation, BEFORE a single byte is
//! written (spec §5 S10–S19). Format walks (`targz`, `zip`) build a plan
//! with these primitives, then materialize it.

pub(crate) mod validate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedKind {
    Dir,
    File { mode: u32 },
    Symlink { target: String },
    Hardlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedEntry {
    pub rel: String,
    pub kind: PlannedKind,
}
```

Add `mod extract;` to `lib.rs` (below `mod error;`).

- [ ] **Step 2: Write the failing tests for `validate.rs`**

Put at the bottom of `crates/openvhost-pkg/src/extract/validate.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for ok in ["php-8.4.23/main.c", "a/b/c.txt", "bin/php"] {
            assert!(validate_entry_name(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn rejects_traversal_and_absolute_and_ads() {
        for bad in [
            "../evil", "a/../../evil", "/abs/path", "C:/x", "c:\\x",
            "a/b:stream", "", ".", "a/./b", "a//b", "a/..",
        ] {
            assert!(validate_entry_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_reserved_and_trailing() {
        for bad in ["con", "a/nul.txt", "com1", "a/b./c", "a/b /c", "lpt9.log"] {
            assert!(validate_entry_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_nul_and_bad_utf8_is_caller_job() {
        assert!(validate_entry_name("a\0b").is_err());
    }

    #[test]
    fn rejects_depth_and_length_overflow() {
        let deep = vec!["a"; MAX_DEPTH + 1].join("/");
        assert!(validate_entry_name(&deep).is_err());
        let long = "a/".repeat(200) + "b";
        assert!(validate_entry_name(&long).is_err());
    }

    #[test]
    fn collision_key_folds_case_and_normalization() {
        assert_eq!(collision_key("File.TXT"), collision_key("file.txt"));
        // NFC 'é' vs NFD 'e\u{301}'
        assert_eq!(collision_key("caf\u{e9}.txt"), collision_key("cafe\u{301}.txt"));
        assert_ne!(collision_key("a.txt"), collision_key("b.txt"));
    }

    #[test]
    fn strip_single_root_applies_only_for_one_top_dir() {
        let mut one = vec![
            RawEntry { rel: "php-8.4/".into(), is_dir: true },
            RawEntry { rel: "php-8.4/main.c".into(), is_dir: false },
        ];
        assert!(strip_single_root(&mut one));
        assert_eq!(one[0].rel, "");
        assert_eq!(one[1].rel, "main.c");

        let mut flat = vec![
            RawEntry { rel: "php.exe".into(), is_dir: false },
            RawEntry { rel: "ext/".into(), is_dir: true },
        ];
        assert!(!strip_single_root(&mut flat));
        assert_eq!(flat[0].rel, "php.exe");

        // single top-level entry that is a FILE, not a dir -> no strip
        let mut single_file = vec![RawEntry { rel: "only.txt".into(), is_dir: false }];
        assert!(!strip_single_root(&mut single_file));
    }

    #[test]
    fn symlink_targets() {
        // sibling / descendant relative targets ok
        assert!(validate_symlink_target("lib/libfoo.so", "libfoo.so.1").is_ok());
        assert!(validate_symlink_target("a/b/link", "c/d").is_ok());
        // absolute, parent-escaping, or dot components rejected
        for (link, tgt) in [
            ("a/link", "/abs"),
            ("a/link", "../../etc/passwd"),
            ("a/link", "./x"),
            ("a/link", "b/../x"),
        ] {
            assert!(validate_symlink_target(link, tgt).is_err(), "reject {link} -> {tgt}");
        }
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-pkg extract::validate 2>&1 | tail -5`
Expected: compile errors (functions undefined). Implement next.

- [ ] **Step 4: Implement `validate.rs` (above the tests)**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure path/entry validation — the extractor's trusted core. No I/O.

use unicode_normalization::UnicodeNormalization;

use crate::error::PkgError;

pub(crate) const MAX_ENTRIES: usize = 100_000;
pub(crate) const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_DEPTH: usize = 32;
pub(crate) const MAX_REL_BYTES: usize = 240;

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7",
];

#[derive(Debug, Clone)]
pub(crate) struct RawEntry {
    pub rel: String,
    pub is_dir: bool,
}

fn reject(reason: &str) -> PkgError {
    PkgError::UnsafeArchive(reason.to_string())
}

/// Validate a raw archive entry name and return a normalized relative path
/// using '/' separators. Rejects (S11): non-relative, `..`/`.`/empty
/// components, backslashes, drive/UNC prefixes, `:` (ADS), NUL, reserved
/// device basenames, trailing dot/space, depth > MAX_DEPTH, byte length >
/// MAX_REL_BYTES. Callers pass valid UTF-8 (zip callers convert `name_raw`
/// and reject non-UTF8 before calling).
pub(crate) fn validate_entry_name(raw: &str) -> Result<String, PkgError> {
    if raw.is_empty() {
        return Err(reject("empty entry name"));
    }
    if raw.as_bytes().len() > MAX_REL_BYTES {
        return Err(reject("entry path too long"));
    }
    if raw.contains('\0') {
        return Err(reject("entry name contains NUL"));
    }
    if raw.contains('\\') {
        return Err(reject("entry name contains backslash"));
    }
    if raw.starts_with('/') {
        return Err(reject("absolute entry path"));
    }
    // Drive prefix like "C:" anywhere is caught by the ':' rule below.
    let comps: Vec<&str> = raw.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    // Note: filtering empty and "." would hide "a//b" and "a/./b"; detect them explicitly.
    if raw.split('/').any(|c| c.is_empty()) {
        // leading handled above; this catches "a//b" and trailing "a/"
        // Trailing slash is legitimate for dir entries; strip exactly one trailing '/'.
        let trimmed = raw.strip_suffix('/').unwrap_or(raw);
        if trimmed.split('/').any(|c| c.is_empty()) {
            return Err(reject("empty path component"));
        }
    }
    if raw.split('/').any(|c| c == ".") {
        return Err(reject("'.' path component"));
    }
    if comps.len() > MAX_DEPTH {
        return Err(reject("entry nesting too deep"));
    }
    for c in raw.split('/') {
        let c = c.strip_suffix('/').unwrap_or(c);
        if c.is_empty() {
            continue;
        }
        if c == ".." {
            return Err(reject("'..' path component"));
        }
        if c.contains(':') {
            return Err(reject("':' in path (drive/ADS)"));
        }
        if c.ends_with('.') || c.ends_with(' ') {
            return Err(reject("component ends with dot or space"));
        }
        let stem = c.split('.').next().unwrap_or(c).to_ascii_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(reject("reserved device name component"));
        }
    }
    Ok(raw.strip_suffix('/').unwrap_or(raw).to_string())
}

/// Case-folded + NFC-normalized key for cross-filesystem collision detection
/// (S12). APFS/NTFS are case-insensitive by default and APFS folds NFC/NFD.
pub(crate) fn collision_key(rel: &str) -> String {
    rel.nfc().collect::<String>().to_lowercase()
}

/// Apply the single-top-dir strip rule (S18): if EVERY entry shares one
/// top-level component AND that component appears as an explicit directory
/// entry, remove that leading component from all entries. Returns whether a
/// strip occurred. Entries are already name-validated.
pub(crate) fn strip_single_root(entries: &mut [RawEntry]) -> bool {
    strip_single_root_vec(entries)
}

fn top(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

fn strip_single_root_vec(entries: &mut [RawEntry]) -> bool {
    if entries.is_empty() {
        return false;
    }
    let root = top(&entries[0].rel);
    if root.is_empty() {
        return false;
    }
    let all_share = entries.iter().all(|e| top(&e.rel) == root);
    let root_is_dir_entry = entries
        .iter()
        .any(|e| e.is_dir && e.rel.trim_end_matches('/') == root);
    // Reject the degenerate "single top-level file" case: not a dir → no strip.
    if !all_share || !root_is_dir_entry {
        return false;
    }
    let prefix = format!("{root}/");
    for e in entries.iter_mut() {
        e.rel = e.rel.strip_prefix(&prefix).unwrap_or("").to_string();
    }
    true
}

/// Validate a symlink target (S14): relative, valid UTF-8, no `.`/`..`
/// components (sibling/descendant only), and lexically resolves inside the
/// root. `link_rel` is the already-validated link path.
pub(crate) fn validate_symlink_target(link_rel: &str, target: &str) -> Result<(), PkgError> {
    if target.is_empty() {
        return Err(reject("empty symlink target"));
    }
    if target.starts_with('/') || target.contains('\\') || target.contains(':') {
        return Err(reject("absolute or non-relative symlink target"));
    }
    for c in target.split('/') {
        if c == "." || c == ".." || c.is_empty() {
            return Err(reject("symlink target has '.'/'..'/empty component"));
        }
    }
    // Lexical resolution: link lives in its parent dir; join target; must stay
    // at depth >= 0 relative to root (no component takes it above root). With
    // no '..' allowed this is guaranteed, but keep the depth check for defense.
    let parent_depth = link_rel.split('/').count().saturating_sub(1);
    let _ = parent_depth; // no '..' means it can never escape; explicit for clarity
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p openvhost-pkg extract::validate 2>&1 | tail -5`
Expected: all pass. If `validate_entry_name`'s empty-component logic misbehaves on `"a//b"`/`"a/"`, fix until the `rejects_traversal_and_absolute_and_ads` test is green (trailing slash on a dir entry is allowed; internal `//` is not).

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg && git commit -s -m "feat(pkg): extraction validation primitives (path safety, collision, strip, symlink)"
```

---

### Task 3: Hardened tar.gz extraction

**Files:**
- Create: `crates/openvhost-pkg/src/extract/targz.rs`
- Create: `crates/openvhost-pkg/src/testkit.rs` (`#[cfg(test)]` fixture builders, shared by Tasks 3–4)
- Modify: `crates/openvhost-pkg/src/extract/mod.rs` (add `pub(crate) mod targz;`)
- Modify: `crates/openvhost-pkg/src/lib.rs` (add `#[cfg(test)] mod testkit;`)

**Interfaces:**
- Consumes (Task 2): all of `validate`, `PlannedKind`, `PlannedEntry`.
- Produces:
  - `pub(crate) fn extract_targz(archive: &mut std::fs::File, dest: &std::path::Path) -> Result<(), PkgError>` — two-pass (validate whole archive, then write) hardened extraction to an existing empty `dest`.
  - testkit: `pub(crate) fn targz_bytes(entries: &[TarSpec]) -> Vec<u8>` + `pub(crate) enum TarSpec` (see code).

- [ ] **Step 1: Write the testkit fixture builders**

`crates/openvhost-pkg/src/testkit.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only archive builders. Lets unit tests construct adversarial tar.gz
//! and zip archives in memory so the extractor is exercised against real
//! hostile inputs, not mocks.
#![allow(clippy::unwrap_used)]

use std::io::Write;

pub(crate) enum TarSpec {
    File { path: &'static str, data: &'static [u8], mode: u32 },
    Dir { path: &'static str },
    Symlink { path: &'static str, target: &'static str },
    Hardlink { path: &'static str, target: &'static str },
    Fifo { path: &'static str },
}

pub(crate) fn targz_bytes(entries: &[TarSpec]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    let gz = GzEncoder::new(Vec::new(), Compression::fast());
    let mut ar = tar::Builder::new(gz);
    for e in entries {
        let mut h = tar::Header::new_gnu();
        match e {
            TarSpec::File { path, data, mode } => {
                h.set_size(data.len() as u64);
                h.set_mode(*mode);
                h.set_entry_type(tar::EntryType::Regular);
                h.set_cksum();
                ar.append_data(&mut h, path, &data[..]).unwrap();
            }
            TarSpec::Dir { path } => {
                h.set_size(0);
                h.set_mode(0o755);
                h.set_entry_type(tar::EntryType::Directory);
                h.set_cksum();
                ar.append_data(&mut h, path, std::io::empty()).unwrap();
            }
            TarSpec::Symlink { path, target } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_link_name(target).unwrap();
                h.set_cksum();
                ar.append_data(&mut h, path, std::io::empty()).unwrap();
            }
            TarSpec::Hardlink { path, target } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Link);
                h.set_link_name(target).unwrap();
                h.set_cksum();
                ar.append_data(&mut h, path, std::io::empty()).unwrap();
            }
            TarSpec::Fifo { path } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Fifo);
                h.set_cksum();
                ar.append_data(&mut h, path, std::io::empty()).unwrap();
            }
        }
    }
    let gz = ar.into_inner().unwrap();
    gz.finish().unwrap()
}

/// Write bytes to a NamedTempFile and return it opened read+write, rewound —
/// mirrors the download module handing extraction an open, verified handle.
pub(crate) fn temp_file_with(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
    f
}
```

Add `#[cfg(test)] mod testkit;` to `lib.rs`. Add `pub(crate) mod targz;` to `extract/mod.rs`.

- [ ] **Step 2: Write the failing tests for `targz.rs`**

Bottom of `crates/openvhost-pkg/src/extract/targz.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::{targz_bytes, temp_file_with, TarSpec};
    use std::io::{Seek, SeekFrom};

    fn extract(bytes: &[u8]) -> Result<tempfile::TempDir, PkgError> {
        let mut tf = temp_file_with(bytes);
        tf.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract_targz(tf.as_file_mut(), dest.path())?;
        Ok(dest)
    }

    #[test]
    fn extracts_clean_archive_and_strips_root() {
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "php-8.4.23/" },
            TarSpec::File { path: "php-8.4.23/main.c", data: b"int main;", mode: 0o644 },
            TarSpec::File { path: "php-8.4.23/bin/php", data: b"#!/bin/sh", mode: 0o755 },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("main.c").is_file());
        assert!(dest.path().join("bin/php").is_file());
    }

    #[test]
    fn rejects_zip_slip() {
        let bytes = targz_bytes(&[TarSpec::File { path: "../evil", data: b"x", mode: 0o644 }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        let bytes = targz_bytes(&[TarSpec::File { path: "/etc/evil", data: b"x", mode: 0o644 }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_device_and_fifo() {
        let bytes = targz_bytes(&[TarSpec::Fifo { path: "a/pipe" }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_symlink_escape() {
        let bytes = targz_bytes(&[TarSpec::Symlink { path: "a/link", target: "../../etc" }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_symlink_chain_escape() {
        // S14: d -> . then L -> d/../x escapes at runtime; lexical rules reject
        // any '..'/'.' in a target, so both are refused.
        let bytes = targz_bytes(&[
            TarSpec::Symlink { path: "d", target: "." },
            TarSpec::Symlink { path: "l", target: "d/../x" },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn accepts_internal_relative_symlink() {
        let bytes = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File { path: "p/libfoo.so.1", data: b"x", mode: 0o755 },
            TarSpec::Symlink { path: "p/libfoo.so", target: "libfoo.so.1" },
        ]);
        let dest = extract(&bytes).unwrap();
        let meta = std::fs::symlink_metadata(dest.path().join("libfoo.so")).unwrap();
        assert!(meta.file_type().is_symlink());
    }

    #[test]
    fn rejects_hardlink_escape_and_materializes_internal_as_copy() {
        let bad = targz_bytes(&[TarSpec::Hardlink { path: "p/l", target: "../outside" }]);
        assert!(extract(&bad).is_err());
        let good = targz_bytes(&[
            TarSpec::Dir { path: "p/" },
            TarSpec::File { path: "p/real", data: b"data", mode: 0o644 },
            TarSpec::Hardlink { path: "p/copy", target: "p/real" },
        ]);
        let dest = extract(&good).unwrap();
        assert_eq!(std::fs::read(dest.path().join("copy")).unwrap(), b"data");
    }

    #[test]
    fn rejects_case_collision() {
        let bytes = targz_bytes(&[
            TarSpec::File { path: "a/File.txt", data: b"1", mode: 0o644 },
            TarSpec::File { path: "a/file.txt", data: b"2", mode: 0o644 },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn clamps_modes() {
        use std::os::unix::fs::PermissionsExt;
        let bytes = targz_bytes(&[
            TarSpec::File { path: "s/setuid", data: b"x", mode: 0o4755 },
            TarSpec::File { path: "s/data", data: b"x", mode: 0o666 },
        ]);
        let dest = extract(&bytes).unwrap();
        let ex = std::fs::metadata(dest.path().join("setuid")).unwrap().permissions().mode() & 0o7777;
        let da = std::fs::metadata(dest.path().join("data")).unwrap().permissions().mode() & 0o7777;
        assert_eq!(ex, 0o755, "exec bit kept, setuid stripped");
        assert_eq!(da, 0o644, "no exec bit");
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-pkg extract::targz 2>&1 | tail -5`
Expected: compile error (`extract_targz` undefined).

- [ ] **Step 4: Implement `targz.rs` (above the tests)**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened tar.gz extraction: two passes over a seekable handle. Pass 1
//! validates EVERY entry and rejects the whole archive on any violation;
//! only then does pass 2 write. Never uses tar-rs `unpack` (RUSTSEC-2021-0080
//! link traversal) — a manual walk applying the `validate` primitives.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;

use super::validate::{
    collision_key, strip_single_root, validate_entry_name, validate_symlink_target, RawEntry,
    MAX_ENTRIES, MAX_TOTAL_BYTES,
};
use super::{PlannedEntry, PlannedKind};
use crate::error::PkgError;

fn reject(msg: impl Into<String>) -> PkgError {
    PkgError::UnsafeArchive(msg.into())
}

/// Extract `archive` (a verified, open handle positioned anywhere) into the
/// already-created empty directory `dest`. Pass 1 validates the whole archive
/// and rejects it on any violation; only then does pass 2 write.
pub(crate) fn extract_targz(archive: &mut fs::File, dest: &Path) -> Result<(), PkgError> {
    let plan = plan_targz(archive)?;
    materialize(archive, &plan, dest)?;
    Ok(())
}

/// Pass 1 — read all headers, validate, build a strip-adjusted plan.
fn plan_targz(archive: &mut fs::File) -> Result<Vec<PlannedEntry>, PkgError> {
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|e| io_err("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(GzDecoder::new(&mut *archive));

    // First collect raw (rel, is_dir) for the strip decision + kind metadata.
    struct Staged {
        rel: String,
        kind: PlannedKind,
        is_dir: bool,
    }
    let mut staged: Vec<Staged> = Vec::new();
    let mut count = 0usize;
    let mut declared_total: u64 = 0;

    for entry in ar.entries().map_err(|e| reject(format!("tar read: {e}")))? {
        let entry = entry.map_err(|e| reject(format!("tar entry: {e}")))?;
        let et = entry.header().entry_type();
        // Skip metadata headers tar-rs may surface; reject dangerous types.
        use tar::EntryType as T;
        if matches!(et, T::XHeader | T::XGlobalHeader | T::GNULongName | T::GNULongLink) {
            continue;
        }
        count += 1;
        if count > MAX_ENTRIES {
            return Err(reject("too many entries"));
        }
        let path = entry.path().map_err(|e| reject(format!("bad path: {e}")))?;
        let rel = path
            .to_str()
            .ok_or_else(|| reject("entry path not utf-8"))?
            .replace('\\', "/");
        let rel = validate_entry_name(&rel)?;

        let kind = match et {
            T::Regular | T::Continuous => {
                declared_total = declared_total.saturating_add(entry.size());
                PlannedKind::File {
                    mode: entry.header().mode().unwrap_or(0o644),
                }
            }
            T::Directory => PlannedKind::Dir,
            T::Symlink => {
                let tgt = link_target(&entry)?;
                validate_symlink_target(&rel, &tgt)?;
                PlannedKind::Symlink { target: tgt }
            }
            T::Link => {
                let tgt = link_target(&entry)?;
                let tgt = validate_entry_name(&tgt)?;
                PlannedKind::Hardlink { target: tgt }
            }
            _ => return Err(reject("disallowed entry type (device/fifo/sparse)")),
        };
        let is_dir = matches!(kind, PlannedKind::Dir);
        staged.push(Staged { rel, kind, is_dir });
    }

    if declared_total > MAX_TOTAL_BYTES {
        return Err(reject("declared size exceeds cap"));
    }

    // Strip single root using the raw view, then carry the adjustment back.
    let mut raws: Vec<RawEntry> = staged
        .iter()
        .map(|s| RawEntry { rel: s.rel.clone(), is_dir: s.is_dir })
        .collect();
    let stripped = strip_single_root(&mut raws);

    // Collision check on final paths; also re-validate stripped paths.
    let mut seen: HashSet<String> = HashSet::new();
    let mut plan: Vec<PlannedEntry> = Vec::with_capacity(staged.len());
    for (s, r) in staged.into_iter().zip(raws.into_iter()) {
        if r.rel.is_empty() {
            // the stripped root dir itself — drop it
            continue;
        }
        let rel = if stripped { validate_entry_name(&r.rel)? } else { r.rel };
        if !seen.insert(collision_key(&rel)) {
            return Err(reject(format!("path collision: {rel}")));
        }
        // Hardlink/symlink targets were validated pre-strip; recompute hardlink
        // target against the stripped tree if a strip happened.
        let kind = match s.kind {
            PlannedKind::Hardlink { target } if stripped => {
                let t = target.split_once('/').map(|(_, r)| r.to_string()).unwrap_or(target);
                PlannedKind::Hardlink { target: t }
            }
            other => other,
        };
        plan.push(PlannedEntry { rel, kind });
    }
    Ok(plan)
}

fn link_target(entry: &tar::Entry<'_, impl Read>) -> Result<String, PkgError> {
    let l = entry
        .link_name()
        .map_err(|e| reject(format!("bad link name: {e}")))?
        .ok_or_else(|| reject("link entry without target"))?;
    l.to_str()
        .ok_or_else(|| reject("link target not utf-8"))
        .map(|s| s.replace('\\', "/"))
}

/// Pass 2 — create dirs, stream regular files from the SAME handle (re-seek +
/// fresh decoder, real-bytes cap), then deferred hardlinks (copy) and finally
/// symlinks (S14: last, after the tree exists, so no ancestor is a symlink at
/// creation time), then strip macOS quarantine xattrs (S19).
fn materialize(archive: &mut fs::File, plan: &[PlannedEntry], dest: &Path) -> Result<(), PkgError> {
    // Directories first, shallow→deep.
    let mut dirs: Vec<&PlannedEntry> = plan
        .iter()
        .filter(|e| matches!(e.kind, PlannedKind::Dir))
        .collect();
    dirs.sort_by_key(|e| e.rel.split('/').count());
    for d in dirs {
        let p = dest.join(&d.rel);
        fs::create_dir_all(&p).map_err(|e| io_err("create_dir", &p, e))?;
        set_dir_mode(&p)?;
    }

    // Regular files: clamped-mode lookup keyed by validated rel.
    let file_modes: std::collections::HashMap<&str, u32> = plan
        .iter()
        .filter_map(|e| match &e.kind {
            PlannedKind::File { mode } => Some((e.rel.as_str(), clamp_mode(*mode))),
            _ => None,
        })
        .collect();

    // Re-seek to 0 and re-derive the same stripped rel for each Regular entry.
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|e| io_err("seek", Path::new("<archive>"), e))?;
    let mut ar = tar::Archive::new(GzDecoder::new(&mut *archive));
    let mut written: u64 = 0;
    for entry in ar.entries().map_err(|e| reject(format!("tar reread: {e}")))? {
        let mut entry = entry.map_err(|e| reject(format!("tar entry: {e}")))?;
        use tar::EntryType as T;
        if !matches!(entry.header().entry_type(), T::Regular | T::Continuous) {
            continue;
        }
        let raw = entry
            .path()
            .map_err(|e| reject(format!("bad path: {e}")))?
            .to_str()
            .ok_or_else(|| reject("path not utf-8"))?
            .replace('\\', "/");
        let rel = plan_rel_for(&raw, plan)?;
        let Some(&mode) = file_modes.get(rel.as_str()) else {
            continue; // stripped root dir or non-file
        };
        let out_path = dest.join(&rel);
        ensure_parent(&out_path)?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| io_err("create_new", &out_path, e))?;
        written = copy_capped(&mut entry, &mut out, written)?;
        set_file_mode(&out_path, mode)?;
    }

    // Hardlinks (materialized as copies) — target is an already-extracted file.
    for e in plan {
        if let PlannedKind::Hardlink { target } = &e.kind {
            let src = dest.join(target);
            if !src.is_file() {
                return Err(reject(format!("hardlink target missing: {target}")));
            }
            let dst = dest.join(&e.rel);
            ensure_parent(&dst)?;
            fs::copy(&src, &dst).map_err(|e2| io_err("hardlink copy", &dst, e2))?;
        }
    }

    // Symlinks last.
    for e in plan {
        if let PlannedKind::Symlink { target } = &e.kind {
            let link = dest.join(&e.rel);
            ensure_parent(&link)?;
            create_symlink(target, &link)?;
        }
    }

    strip_quarantine(dest);
    Ok(())
}

/// Map a raw pass-2 entry name to its planned (post-strip) rel by matching
/// against the plan the way pass 1 built it. The plan already applied the
/// single-root strip, so recompute: try the raw name and its once-stripped
/// form, returning whichever the plan contains.
fn plan_rel_for(raw: &str, plan: &[PlannedEntry]) -> Result<String, PkgError> {
    let cleaned = validate_entry_name(raw)?;
    if plan.iter().any(|e| e.rel == cleaned) {
        return Ok(cleaned);
    }
    if let Some((_, after)) = cleaned.split_once('/') {
        if plan.iter().any(|e| e.rel == after) {
            return Ok(after.to_string());
        }
    }
    // Not in plan (e.g. the stripped root dir entry) — return cleaned; caller
    // skips names absent from file_modes.
    Ok(cleaned)
}

/// Copy with a running total cap over REAL decompressed bytes (S17).
fn copy_capped(reader: &mut impl Read, writer: &mut impl io::Write, mut total: u64) -> Result<u64, PkgError> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| reject(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(reject("decompressed size exceeds cap"));
        }
        writer.write_all(&buf[..n]).map_err(|e| reject(format!("write: {e}")))?;
    }
    Ok(total)
}

/// Strip `com.apple.quarantine` (and other non-essential `com.apple.*`) xattrs
/// from the extracted tree before install (S19) — quarantine can ride through
/// archive xattrs (macOS specialist, empirically confirmed). Best-effort;
/// no-op off macOS. Uses the `xattr` crate.
#[cfg(target_os = "macos")]
fn strip_quarantine(dest: &Path) {
    fn walk(p: &Path) {
        if let Ok(names) = xattr::list(p) {
            for n in names {
                if n.to_string_lossy().starts_with("com.apple.") {
                    let _ = xattr::remove(p, &n);
                }
            }
        }
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let cp = e.path();
                if fs::symlink_metadata(&cp).map(|m| !m.file_type().is_symlink()).unwrap_or(false) {
                    walk(&cp);
                }
            }
        }
    }
    walk(dest);
}
#[cfg(not(target_os = "macos"))]
fn strip_quarantine(_dest: &Path) {}

fn set_file_mode(p: &Path, mode: u32) -> Result<(), PkgError> {
    set_file_mode_impl(p, mode)
}

#[cfg(unix)]
fn set_file_mode_impl(p: &Path, mode: u32) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_file_mode_impl(_p: &Path, _mode: u32) -> Result<(), PkgError> { Ok(()) }

fn clamp_mode(mode: u32) -> u32 {
    if mode & 0o111 != 0 { 0o755 } else { 0o644 }
}

fn ensure_parent(p: &Path) -> Result<(), PkgError> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
    }
    Ok(())
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> PkgError {
    PkgError::Io { op, path: path.to_path_buf(), source }
}

#[cfg(unix)]
fn set_dir_mode(p: &Path) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_dir_mode(_p: &Path) -> Result<(), PkgError> { Ok(()) }

#[cfg(unix)]
fn create_symlink(target: &str, link: &Path) -> Result<(), PkgError> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_err("symlink", link, e))
}
#[cfg(windows)]
fn create_symlink(_target: &str, _link: &Path) -> Result<(), PkgError> {
    // Symlink creation needs privilege on Windows; internal package symlinks
    // are rare and out of scope for the v0 Windows runtime (deferred with the
    // matrix). Reject so behavior is explicit, never silently skipped.
    Err(reject("symlink entries not supported on windows v0"))
}
```

Imports for the file's top (the `super::` items are from Task 2's `validate` + `extract/mod.rs`): `std::collections::{HashMap, HashSet}`, `std::fs`, `std::io::{self, Read, Seek, SeekFrom, Write}`, `std::path::Path`, `flate2::read::GzDecoder`, and from `super::validate`: `collision_key, strip_single_root, validate_entry_name, validate_symlink_target, RawEntry, MAX_ENTRIES, MAX_TOTAL_BYTES`; from `super`: `PlannedEntry, PlannedKind`; `crate::error::PkgError`. (`Write` is needed by `copy_capped`.)

- [ ] **Step 5: Run tests to green**

Run: `cargo test -p openvhost-pkg extract::targz -- --nocapture 2>&1 | tail -15`
Expected: all targz tests pass. On unix, `clamps_modes`, `accepts_internal_relative_symlink`, hardlink-copy all pass. If `plan_rel_for` mismaps a stripped path, the `extracts_clean_archive_and_strips_root` test catches it — the plan already carries post-strip rels, so pass-2 matches raw→cleaned→(once-stripped) against them.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg && git commit -s -m "feat(pkg): hardened two-pass tar.gz extraction with adversarial fixtures"
```

---

### Task 4: Hardened zip extraction

**Files:**
- Create: `crates/openvhost-pkg/src/extract/zip.rs`
- Modify: `crates/openvhost-pkg/src/extract/mod.rs` (add `pub(crate) mod zip;`)
- Modify: `crates/openvhost-pkg/src/testkit.rs` (add zip builder)

**Interfaces:**
- Consumes (Task 2): `validate` primitives, `PlannedKind`.
- Produces: `pub(crate) fn extract_zip(archive: &mut std::fs::File, dest: &std::path::Path) -> Result<(), PkgError>`.

- [ ] **Step 1: Add the zip fixture builder to `testkit.rs`**

Append:

```rust
pub(crate) enum ZipSpec {
    File { path: &'static str, data: &'static [u8], mode: u32 },
    Dir { path: &'static str },
    Symlink { path: &'static str, target: &'static str },
}

pub(crate) fn zip_bytes(entries: &[ZipSpec]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        for e in entries {
            match e {
                ZipSpec::File { path, data, mode } => {
                    let opt = SimpleFileOptions::default().unix_permissions(*mode);
                    zw.start_file(*path, opt).unwrap();
                    zw.write_all(data).unwrap();
                }
                ZipSpec::Dir { path } => {
                    zw.add_directory(*path, SimpleFileOptions::default()).unwrap();
                }
                ZipSpec::Symlink { path, target } => {
                    // S_IFLNK | 0777 marks a symlink in the external attrs.
                    let opt = SimpleFileOptions::default().unix_permissions(0o120777);
                    zw.start_file(*path, opt).unwrap();
                    zw.write_all(target.as_bytes()).unwrap();
                }
            }
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}
```

- [ ] **Step 2: Write the failing tests**

Bottom of `crates/openvhost-pkg/src/extract/zip.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::{temp_file_with, zip_bytes, ZipSpec};
    use std::io::{Seek, SeekFrom};

    fn extract(bytes: &[u8]) -> Result<tempfile::TempDir, PkgError> {
        let mut tf = temp_file_with(bytes);
        tf.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract_zip(tf.as_file_mut(), dest.path())?;
        Ok(dest)
    }

    #[test]
    fn extracts_flat_zip_without_stripping() {
        let bytes = zip_bytes(&[
            ZipSpec::File { path: "php.exe", data: b"MZ", mode: 0o755 },
            ZipSpec::Dir { path: "ext/" },
            ZipSpec::File { path: "ext/gd.dll", data: b"dll", mode: 0o644 },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("php.exe").is_file());
        assert!(dest.path().join("ext/gd.dll").is_file());
    }

    #[test]
    fn rejects_zip_slip() {
        let bytes = zip_bytes(&[ZipSpec::File { path: "../evil", data: b"x", mode: 0o644 }]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_duplicate_names() {
        let bytes = zip_bytes(&[
            ZipSpec::File { path: "a.txt", data: b"1", mode: 0o644 },
            ZipSpec::File { path: "a.txt", data: b"2", mode: 0o644 },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn rejects_case_collision() {
        let bytes = zip_bytes(&[
            ZipSpec::File { path: "Read.md", data: b"1", mode: 0o644 },
            ZipSpec::File { path: "read.md", data: b"2", mode: 0o644 },
        ]);
        assert!(extract(&bytes).is_err());
    }

    #[test]
    fn skips_symlink_entries_entirely() {
        // zip symlinks are NOT honored and NOT materialized as files (S14).
        let bytes = zip_bytes(&[
            ZipSpec::File { path: "real", data: b"x", mode: 0o644 },
            ZipSpec::Symlink { path: "link", target: "real" },
        ]);
        let dest = extract(&bytes).unwrap();
        assert!(dest.path().join("real").is_file());
        assert!(!dest.path().join("link").exists(), "symlink entry must be skipped");
    }

    #[cfg(unix)]
    #[test]
    fn clamps_modes() {
        use std::os::unix::fs::PermissionsExt;
        let bytes = zip_bytes(&[ZipSpec::File { path: "s", data: b"x", mode: 0o4777 }]);
        let dest = extract(&bytes).unwrap();
        let m = std::fs::metadata(dest.path().join("s")).unwrap().permissions().mode() & 0o7777;
        assert_eq!(m, 0o755);
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-pkg extract::zip 2>&1 | tail -5`
Expected: compile error (`extract_zip` undefined).

- [ ] **Step 4: Implement `zip.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened zip extraction: iterate the central directory only, validate
//! every entry (reject-the-archive on any violation), then write. Symlink
//! entries (S_IFLNK in external attrs) are skipped entirely — never honored,
//! never materialized as a plain file. Encrypted entries are rejected.
//! Never uses zip-rs `extract` (historic `../`/dup handling) — a manual walk.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::validate::{collision_key, strip_single_root, validate_entry_name, RawEntry, MAX_ENTRIES, MAX_TOTAL_BYTES};
use crate::error::PkgError;

const S_IFLNK: u32 = 0o120000;

fn reject(msg: impl Into<String>) -> PkgError {
    PkgError::UnsafeArchive(msg.into())
}
fn io_err(op: &'static str, path: &Path, source: io::Error) -> PkgError {
    PkgError::Io { op, path: path.to_path_buf(), source }
}
fn clamp_mode(mode: u32) -> u32 {
    if mode & 0o111 != 0 { 0o755 } else { 0o644 }
}

struct PlannedFile {
    index: usize,
    rel: String,
    mode: u32,
}

pub(crate) fn extract_zip(archive: &mut fs::File, dest: &Path) -> Result<(), PkgError> {
    archive.seek(SeekFrom::Start(0)).map_err(|e| io_err("seek", Path::new("<archive>"), e))?;
    let mut zip = zip::ZipArchive::new(&mut *archive).map_err(|e| reject(format!("zip open: {e}")))?;

    if zip.len() > MAX_ENTRIES {
        return Err(reject("too many entries"));
    }

    // Pass 1: validate, collect files + dirs (skip symlinks entirely).
    let mut raws: Vec<RawEntry> = Vec::new();
    let mut files: Vec<PlannedFile> = Vec::new();
    let mut declared_total: u64 = 0;

    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| reject(format!("zip entry {i}: {e}")))?;
        if entry.encrypted() {
            return Err(reject("encrypted zip entry"));
        }
        let raw = entry.name_raw();
        let name = std::str::from_utf8(raw).map_err(|_| reject("zip entry name not utf-8"))?;
        let is_dir = entry.is_dir();
        let mode = entry.unix_mode().unwrap_or(if is_dir { 0o755 } else { 0o644 });
        // Symlink entries: skip entirely (not honored, not written).
        if !is_dir && (mode & S_IFLNK) == S_IFLNK {
            continue;
        }
        let rel = validate_entry_name(&name.replace('\\', "/"))?;
        if is_dir {
            raws.push(RawEntry { rel: rel.clone(), is_dir: true });
        } else {
            declared_total = declared_total.saturating_add(entry.size());
            raws.push(RawEntry { rel: rel.clone(), is_dir: false });
            files.push(PlannedFile { index: i, rel, mode });
        }
    }
    if declared_total > MAX_TOTAL_BYTES {
        return Err(reject("declared size exceeds cap"));
    }

    let stripped = strip_single_root(&mut raws);
    // Collision on final paths.
    let mut seen: HashSet<String> = HashSet::new();
    for r in &raws {
        if r.rel.is_empty() {
            continue;
        }
        if !seen.insert(collision_key(&r.rel)) {
            return Err(reject(format!("path collision: {}", r.rel)));
        }
    }

    // Directories first.
    for r in &raws {
        if r.is_dir && !r.rel.is_empty() {
            let p = dest.join(&r.rel);
            fs::create_dir_all(&p).map_err(|e| io_err("create_dir", &p, e))?;
        }
    }

    // Pass 2: write files with a real-bytes total cap.
    let mut written: u64 = 0;
    for f in &files {
        let rel = if stripped {
            let after = f.rel.split_once('/').map(|(_, r)| r.to_string()).unwrap_or_default();
            if after.is_empty() { continue; }
            validate_entry_name(&after)?
        } else {
            f.rel.clone()
        };
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
        }
        let mut entry = zip.by_index(f.index).map_err(|e| reject(format!("zip reopen: {e}")))?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .map_err(|e| io_err("create_new", &out_path, e))?;
        written = copy_capped(&mut entry, &mut out, written)?;
        set_file_mode(&out_path, clamp_mode(f.mode))?;
    }
    Ok(())
}

/// Copy with a running total cap over REAL decompressed bytes (S17).
fn copy_capped(reader: &mut impl Read, writer: &mut impl io::Write, mut total: u64) -> Result<u64, PkgError> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| reject(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(reject("decompressed size exceeds cap"));
        }
        writer.write_all(&buf[..n]).map_err(|e| reject(format!("write: {e}")))?;
    }
    Ok(total)
}

#[cfg(unix)]
fn set_file_mode(p: &Path, mode: u32) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_file_mode(_p: &Path, _mode: u32) -> Result<(), PkgError> { Ok(()) }
```

- [ ] **Step 5: Run tests to green**

Run: `cargo test -p openvhost-pkg extract::zip -- --nocapture 2>&1 | tail -15`
Expected: all zip tests pass.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg && git commit -s -m "feat(pkg): hardened zip extraction (symlink-skip, encrypted-reject, caps)"
```

---

### Task 5: Download module — streaming fetch, hash, caps, redirect policy

**Files:**
- Create: `crates/openvhost-pkg/src/download.rs`
- Modify: `crates/openvhost-pkg/src/lib.rs` (add `mod download;`)

**Interfaces:**
- Consumes (Task 1): `PkgError`, `request::validate_https_url`, `Progress`.
- Produces: `pub(crate) async fn download_and_verify(url: &url::Url, sha256: &str, staging_dir: &std::path::Path, mut progress: impl FnMut(Progress)) -> Result<std::fs::File, PkgError>` — streams to `staging_dir/archive`, hashes during streaming, enforces the 1 GiB cap, verifies BEFORE returning, and returns the OPEN file rewound to 0 (S8: extraction uses this same handle, never re-opens by path).

- [ ] **Step 1: Write the hermetic-server test harness + failing tests**

Bottom of `crates/openvhost-pkg/src/download.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Minimal one-shot HTTP/1.1 server. `handler` receives the request line
    /// and returns raw response bytes. Returns the bound URL. Loopback + http
    /// is permitted only under debug_assertions (S2).
    fn serve(body: Vec<u8>, extra_headers: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/archive", addr.port());
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}\r\n",
                    body.len(),
                    extra_headers
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.write_all(&body);
            }
        });
        (url, handle)
    }

    fn sha_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let body = b"hello openvhost".to_vec();
        let sha = sha_hex(&body);
        let (url, h) = serve(body.clone(), "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut f = download_and_verify(&u, &sha, dir.path(), |_| {}).await.unwrap();
        let mut got = Vec::new();
        use std::io::{Read, Seek, SeekFrom};
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        h.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_hash_mismatch() {
        let (url, h) = serve(b"tampered".to_vec(), "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_and_verify(&u, &"0".repeat(64), dir.path(), |_| {}).await.unwrap_err();
        assert!(matches!(err, PkgError::HashMismatch { .. }));
        // staging archive must be gone on failure
        assert!(!dir.path().join("archive").exists());
        h.join().unwrap();
    }

    #[tokio::test]
    async fn enforces_size_cap() {
        // 2 MiB body against a tiny cap via the test-only constructor.
        let body = vec![0u8; 2 * 1024 * 1024];
        let (url, h) = serve(body, "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_capped(&u, &"0".repeat(64), dir.path(), 1024, |_| {}).await.unwrap_err();
        assert!(matches!(err, PkgError::TooLarge { .. }));
        h.join().unwrap();
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-pkg download 2>&1 | tail -5`
Expected: compile error (`download_and_verify`/`download_capped` undefined).

- [ ] **Step 3: Implement `download.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming download + SHA-256 verification. The hash is computed over the
//! exact wire bytes (transport auto-decompression is disabled at the reqwest
//! feature level and we send Accept-Encoding: identity — S3). Verification
//! happens BEFORE the handle is returned to the extractor, on the SAME open
//! file (S8); nothing is ever re-opened by path.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::PkgError;
use crate::request::{validate_https_url, Progress};

const SIZE_CAP: u64 = 1024 * 1024 * 1024; // 1 GiB (S4)
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

pub(crate) async fn download_and_verify(
    url: &url::Url,
    sha256: &str,
    staging_dir: &Path,
    progress: impl FnMut(Progress),
) -> Result<fs::File, PkgError> {
    download_capped(url, sha256, staging_dir, SIZE_CAP, progress).await
}

/// Cap-parameterized core (the public wrapper pins 1 GiB; tests pass a tiny
/// cap). In production `url` is already `https`; in debug builds a loopback
/// `http` URL is permitted so hermetic tests need no TLS (S2 — compiled out
/// of release).
pub(crate) async fn download_capped(
    url: &url::Url,
    sha256: &str,
    staging_dir: &Path,
    cap: u64,
    mut progress: impl FnMut(Progress),
) -> Result<fs::File, PkgError> {
    check_scheme(url)?;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error(io_msg("too many redirects"));
            }
            match check_scheme_result(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(_) => attempt.error(io_msg("redirect to disallowed url")),
            }
        }))
        .build()
        .map_err(|e| PkgError::Network(e.to_string()))?;

    let resp = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| PkgError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| PkgError::Network(e.to_string()))?;

    let declared = resp.content_length();
    if let Some(len) = declared {
        if len > cap {
            return Err(PkgError::TooLarge { cap });
        }
    }
    progress(Progress::Started { total: declared });

    let archive_path = staging_dir.join("archive");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&archive_path)
        .map_err(|e| io_err("create_new", &archive_path, e))?;

    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| PkgError::Network(e.to_string()))?;
        total = total.saturating_add(chunk.len() as u64);
        if total > cap {
            drop(file);
            let _ = fs::remove_file(&archive_path);
            return Err(PkgError::TooLarge { cap });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|e| io_err("write", &archive_path, e))?;
        progress(Progress::Downloaded { bytes: total });
    }
    file.sync_all().map_err(|e| io_err("sync", &archive_path, e))?;

    let actual = hex::encode(hasher.finalize());
    if actual != sha256 {
        drop(file);
        let _ = fs::remove_file(&archive_path);
        return Err(PkgError::HashMismatch {
            expected: sha256.to_string(),
            actual,
        });
    }
    progress(Progress::Verified);
    file.seek(SeekFrom::Start(0)).map_err(|e| io_err("seek", &archive_path, e))?;
    Ok(file)
}

fn check_scheme(url: &url::Url) -> Result<(), PkgError> {
    check_scheme_result(url)
}

fn check_scheme_result(url: &url::Url) -> Result<(), PkgError> {
    // Production: https only via validate_https_url. Debug builds also accept
    // http to a loopback host so hermetic tests need no TLS.
    #[cfg(debug_assertions)]
    {
        if url.scheme() == "http" {
            if let Some(host) = url.host_str() {
                if host == "127.0.0.1" || host == "localhost" || host == "[::1]" {
                    return Ok(());
                }
            }
        }
    }
    validate_https_url(url)
}

fn io_err(op: &'static str, path: &Path, source: std::io::Error) -> PkgError {
    PkgError::Io { op, path: path.to_path_buf(), source }
}

fn io_msg(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::other(msg.to_string()))
}
```

Add `mod download;` to `lib.rs`. Add `futures-util = "0.3"` to `[dependencies]` in Cargo.toml (reqwest's `bytes_stream()` yields a `Stream`; `StreamExt::next` needs futures-util — MIT/Apache, passes deny; note it in the commit body).

- [ ] **Step 4: Run tests to green**

Run: `cargo test -p openvhost-pkg download -- --nocapture 2>&1 | tail -15`
Expected: `downloads_and_verifies`, `rejects_hash_mismatch`, `enforces_size_cap` pass.

- [ ] **Step 5: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg Cargo.lock && git commit -s -m "feat(pkg): streaming download+verify with size cap and https-only redirect policy

Adds futures-util (MIT OR Apache-2.0) for the response byte stream."
```

---

### Task 6: Layout — staging, atomic install, sweep, `current` link

**Files:**
- Create: `crates/openvhost-pkg/src/layout.rs`
- Create: `crates/openvhost-pkg/src/platform/mod.rs`
- Create: `crates/openvhost-pkg/src/platform/unix.rs`
- Create: `crates/openvhost-pkg/src/platform/windows.rs`
- Modify: `crates/openvhost-pkg/src/lib.rs` (add `mod layout; mod platform;`)

**Interfaces:**
- Consumes: `PkgError`, `PackagesRoot`.
- Produces:
  - `pub(crate) struct Staging { dir: tempfile::TempDir, _lock: fd_lock::RwLock<std::fs::File> }` with `pub(crate) fn create(root: &PackagesRoot) -> Result<Staging, PkgError>` and `pub(crate) fn path(&self) -> &Path`.
  - `pub(crate) fn install_dir(staging_root: &Path, final_dir: &Path, name: &str, version: &str) -> Result<(), PkgError>` — atomic rename with `AlreadyInstalled` mapping.
  - `pub(crate) fn sweep_stale(root: &PackagesRoot)` — best-effort, lock-guarded, >24h.
  - `pub(crate) fn update_current(link: &Path, version_dir_name: &str) -> Result<(), PkgError>` — dispatches to `platform::update_current` (unix symlink swap / windows junction).

- [ ] **Step 1: Write `platform` link code**

`crates/openvhost-pkg/src/platform/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-OS `current`-link maintenance. Unix: atomic symlink swap with a bare
//! relative target. Windows: NTFS junction (no admin) with a verified
//! remove-then-create — NEVER a recursive delete against `current` (S22).

use std::path::Path;
use crate::error::PkgError;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Point `link` (…/current) at the sibling version directory `version`.
pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    #[cfg(unix)]
    { unix::update_current(link, version) }
    #[cfg(windows)]
    { windows::update_current(link, version) }
}
```

`crates/openvhost-pkg/src/platform/unix.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
use std::fs;
use std::path::Path;
use crate::error::PkgError;

/// Atomic swap: create a temp symlink with a BARE RELATIVE sibling target
/// (survives home relocation/Time Machine — macOS specialist), then rename
/// over `current`. If `current` exists it MUST already be a symlink.
pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    let parent = link.parent().ok_or_else(|| bad("current has no parent"))?;
    if let Ok(meta) = fs::symlink_metadata(link) {
        if !meta.file_type().is_symlink() {
            return Err(bad("existing 'current' is not a symlink; refusing to replace"));
        }
    }
    let tmp = parent.join(".current.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(version, &tmp).map_err(|e| io_err("symlink", &tmp, e))?;
    fs::rename(&tmp, link).map_err(|e| io_err("rename", link, e))
}

fn bad(m: &'static str) -> PkgError { PkgError::UnsafeArchive(m.to_string()) }
fn io_err(op: &'static str, p: &Path, e: std::io::Error) -> PkgError {
    PkgError::Io { op, path: p.to_path_buf(), source: e }
}
```

`crates/openvhost-pkg/src/platform/windows.rs` — **macOS-first v1: explicit stub, NOT the junction implementation** (owner scope decision 2026-07-22). The junction design (verify-reparse-point → `fs::remove_dir` → `junction::create`, never `remove_dir_all` — S22) is preserved in spec §6.2 for the future Windows-enablement phase. For v1 the function returns an explicit error so the seam exists and a Windows build fails LOUDLY at the link step rather than silently no-op'ing:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;
use crate::error::PkgError;

/// macOS-first v1: Windows `current`-link support is deferred to the Windows
/// enablement phase (NTFS junction — design in spec §6.2). Returns an explicit
/// error rather than a silent no-op so nothing pretends a link was created.
pub(crate) fn update_current(_link: &Path, _version: &str) -> Result<(), PkgError> {
    Err(PkgError::UnsafeArchive(
        "current-link on Windows is not implemented in v1 (macOS-first)".to_string(),
    ))
}
```

Also in this task **remove the now-unused `junction` dependency** added in Task 1: delete the `[target.'cfg(windows)'.dependencies] junction = "1"` block from `crates/openvhost-pkg/Cargo.toml` (it is referenced nowhere now). Keep the macOS `xattr` dep.

- [ ] **Step 2: Write failing tests for `layout.rs`**

Bottom of `crates/openvhost-pkg/src/layout.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::request::PackagesRoot;

    fn root() -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(root.as_path()).unwrap();
        (home, root)
    }

    #[test]
    fn staging_is_locked_and_created() {
        let (_h, r) = root();
        let s = Staging::create(&r).unwrap();
        assert!(s.path().is_dir());
        assert!(s.path().starts_with(r.staging_root()));
    }

    #[test]
    fn install_dir_is_atomic_and_rejects_existing() {
        let (_h, r) = root();
        let staging = tempfile::tempdir_in(r.as_path()).unwrap();
        std::fs::write(staging.path().join("marker"), b"x").unwrap();
        let dest = r.package_dir("php", "8.4", "8.4.23");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        install_dir(staging.path(), &dest, "php", "8.4.23").unwrap();
        assert!(dest.join("marker").is_file());

        // second install to same dest -> AlreadyInstalled
        let staging2 = tempfile::tempdir_in(r.as_path()).unwrap();
        let err = install_dir(staging2.path(), &dest, "php", "8.4.23").unwrap_err();
        assert!(matches!(err, PkgError::AlreadyInstalled { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn current_link_swaps_atomically() {
        let (_h, r) = root();
        let major = r.major_dir("php", "8.4");
        std::fs::create_dir_all(major.join("8.4.1")).unwrap();
        std::fs::create_dir_all(major.join("8.4.2")).unwrap();
        let link = r.current_link("php", "8.4");
        update_current(&link, "8.4.1").unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap().to_str().unwrap(), "8.4.1");
        update_current(&link, "8.4.2").unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap().to_str().unwrap(), "8.4.2");
    }

    #[cfg(unix)]
    #[test]
    fn current_refuses_to_replace_real_dir() {
        let (_h, r) = root();
        let major = r.major_dir("php", "8.4");
        let link = r.current_link("php", "8.4");
        std::fs::create_dir_all(&link).unwrap(); // a real dir named "current"
        std::fs::create_dir_all(major.join("8.4.1")).unwrap();
        assert!(update_current(&link, "8.4.1").is_err());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-pkg layout 2>&1 | tail -5`
Expected: compile error.

- [ ] **Step 4: Implement `layout.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Staging, atomic install, stale-staging sweep, and current-link update.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::PkgError;
use crate::platform;
use crate::request::PackagesRoot;

const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A staging directory holding an exclusive advisory lock for its lifetime,
/// so the 24h sweeper never deletes a live (possibly slept-mid-download)
/// install (S20).
pub(crate) struct Staging {
    dir: tempfile::TempDir,
    _lock_file: fs::File,
    _guard: OwnedGuard,
}

// fd_lock's guard borrows the file; keep both alive together via a small
// self-contained holder.
struct OwnedGuard(fd_lock::RwLock<fs::File>);

impl Staging {
    pub(crate) fn create(root: &PackagesRoot) -> Result<Staging, PkgError> {
        let sroot = root.staging_root();
        fs::create_dir_all(&sroot).map_err(|e| io_err("create_dir", &sroot, e))?;
        set_private(&sroot)?;
        let dir = tempfile::Builder::new()
            .prefix("ovh")
            .tempdir_in(&sroot)
            .map_err(|e| io_err("tempdir", &sroot, e))?;
        set_private(dir.path())?;
        let lock_path = dir.path().join(".lock");
        let lf = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| io_err("lockfile", &lock_path, e))?;
        // Hold an exclusive lock for the Staging's lifetime.
        let mut lock = fd_lock::RwLock::new(lf.try_clone().map_err(|e| io_err("clone", &lock_path, e))?);
        // Acquire and leak the guard into the struct via a boxed self-ref.
        std::mem::forget(lock.write().map_err(|e| io_err("flock", &lock_path, e))?);
        Ok(Staging { dir, _lock_file: lf, _guard: OwnedGuard(lock) })
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Atomic install: rename the staged tree onto the final version dir. Same
/// volume by construction. EEXIST/ENOTEMPTY (dest already present, or a
/// concurrent identical install won) maps to AlreadyInstalled (S21).
pub(crate) fn install_dir(
    staged_root: &Path,
    final_dir: &Path,
    name: &str,
    version: &str,
) -> Result<(), PkgError> {
    if final_dir.exists() {
        return Err(PkgError::AlreadyInstalled { name: name.into(), version: version.into() });
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
    }
    match fs::rename(staged_root, final_dir) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(c) if is_exists(c)) => {
            Err(PkgError::AlreadyInstalled { name: name.into(), version: version.into() })
        }
        Err(e) => Err(io_err("rename", final_dir, e)),
    }
}

/// Best-effort sweep of staging dirs older than 24h whose lock is free (S20).
pub(crate) fn sweep_stale(root: &PackagesRoot) {
    let sroot = root.staging_root();
    let Ok(rd) = fs::read_dir(&sroot) else { return };
    let now = SystemTime::now();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| now.duration_since(t).map(|d| d > STALE_AFTER).unwrap_or(false))
            .unwrap_or(false);
        if !stale {
            continue;
        }
        // Only delete if we can take the lock (no live install holds it).
        let lock_path = path.join(".lock");
        if let Ok(lf) = fs::OpenOptions::new().write(true).open(&lock_path) {
            let mut lock = fd_lock::RwLock::new(lf);
            if lock.try_write().is_ok() {
                let _ = fs::remove_dir_all(&path);
            }
        } else {
            // No lockfile (older format / partial) and stale → safe to remove.
            let _ = fs::remove_dir_all(&path);
        }
    }
}

pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    platform::update_current(link, version)
}

fn is_exists(code: i32) -> bool {
    // EEXIST (17) / ENOTEMPTY (66 macOS, 39 linux). Windows ERROR_ALREADY_EXISTS 183.
    matches!(code, 17 | 66 | 39 | 183)
}

#[cfg(unix)]
fn set_private(p: &Path) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).map_err(|e| io_err("chmod", p, e))
}
#[cfg(not(unix))]
fn set_private(_p: &Path) -> Result<(), PkgError> { Ok(()) }

fn io_err(op: &'static str, p: &Path, e: io::Error) -> PkgError {
    PkgError::Io { op, path: p.to_path_buf(), source: e }
}
```

**Implementer note on `Staging` lock lifetime:** `fd_lock`'s write guard borrows the `RwLock`, so a self-referential struct won't compile cleanly. Simplest correct approach: DON'T store the guard. Instead hold the lock by keeping the `RwLock<File>` and re-acquiring is unnecessary — an OS advisory lock is held as long as the underlying fd is open IF you use `flock`-style semantics; but `fd_lock` releases on guard drop. To hold for the struct's lifetime without self-reference, use a raw approach: on unix, `nix`/`libc::flock` on the fd stored in the struct (lock released when the fd closes on drop); on the whole, the SIMPLEST portable solution the implementer should use: keep the `fd_lock::RwLock<File>` in the struct and store the guard via `Box::leak` is wrong. **Prescription:** replace the `_guard`/`OwnedGuard` design with a struct that owns `RwLock<File>` and obtains the guard lazily — but since we only need "sweeper can't delete a live dir," implement the lock as: create `.lock`, take an exclusive `flock(LOCK_EX|LOCK_NB)` via `libc` on unix / `LockFileEx` on windows, and let it release when the fd (stored in `Staging`) closes at drop. Add `libc = "0.2"` (unix) for `flock`. If that expands scope too far, the acceptable v0 fallback: hold the lock by storing `RwLock<File>` and a `'static`-lifetime guard is not possible — so use the fd-close-releases model. Make `staging_is_locked_and_created` pass and ensure a second `Staging::create` in a spawned attempt cannot delete the first via `sweep_stale`. Keep it correct and simple; report if it fights you.

- [ ] **Step 5: Implement, run tests to green**

Run: `cargo test -p openvhost-pkg layout -- --nocapture 2>&1 | tail -15`
Expected: staging, install-atomic/AlreadyInstalled, and (unix) current-swap + refuse-real-dir tests pass.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg Cargo.lock && git commit -s -m "feat(pkg): staging with lock guard, atomic install, sweep, current-link swap"
```

---

### Task 7: Orchestration + hermetic integration + live proof

**Files:**
- Create: `crates/openvhost-pkg/src/install.rs`
- Modify: `crates/openvhost-pkg/src/lib.rs` (add `mod install;`, re-export `install_package`)
- Create: `crates/openvhost-pkg/tests/integration.rs`
- Create: `crates/openvhost-pkg/tests/live_net.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: `pub async fn install_package(req: &InstallRequest, root: &PackagesRoot, progress: impl FnMut(Progress) + Send) -> Result<InstalledPackage, PkgError>`.

- [ ] **Step 1: Implement `install.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Pipeline orchestration: sweep → stage → download+verify → extract →
//! atomic install → current link. Single in-process install at a time (S25).

use std::sync::OnceLock;
use tokio::sync::Semaphore;

use crate::download::download_and_verify;
use crate::error::PkgError;
use crate::extract;
use crate::layout::{self, Staging};
use crate::request::{ArchiveFormat, InstallRequest, InstalledPackage, PackagesRoot, Progress};

fn install_gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(1))
}

pub async fn install_package(
    req: &InstallRequest,
    root: &PackagesRoot,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<InstalledPackage, PkgError> {
    let _permit = install_gate().acquire().await.expect("semaphore not closed");

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
    std::fs::create_dir_all(&extract_root)
        .map_err(|e| PkgError::Io { op: "create_dir", path: extract_root.clone(), source: e })?;

    // Download + verify onto the same handle we extract from (S8).
    let mut file = download_and_verify(&req.url, &req.sha256, &staging_path, &mut progress).await?;

    // Extraction is blocking CPU work.
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
    .map_err(|e| PkgError::Network(format!("extract task panicked: {e}")))??;
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
```

Make `extract::targz` / `extract::zip` reachable: in `extract/mod.rs` they are `pub(crate) mod`. Add `mod install; pub use install::install_package;` to `lib.rs`.

- [ ] **Step 2: Write the hermetic integration test**

`crates/openvhost-pkg/tests/integration.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end pipeline over a hermetic local HTTP server: real tar.gz built
//! in-test, downloaded, verified, extracted, installed, linked.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpListener;

use openvhost_pkg::{install_package, ArchiveFormat, InstallRequest, PackagesRoot, Progress};

fn targz(files: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
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
        ar.append_data(&mut h, format!("pkg/{name}"), *data).unwrap();
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

// dev-build note: http-loopback is accepted only under debug_assertions (S2);
// `cargo test` builds debug, so this hermetic test is valid.
#[tokio::test]
async fn installs_targz_end_to_end() {
    let archive = targz(&[("main.c", b"int main;"), ("bin/php", b"#!/bin/sh")]);
    let sha = sha_hex(&archive);
    let url = serve_once(archive);

    let home = tempfile::Builder::new().prefix("ovh-int").tempdir_in("/tmp").unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();

    let req = InstallRequest::new("php", "8.4", "8.4.99", &url, &sha, ArchiveFormat::TarGz).unwrap();
    let mut events = Vec::new();
    let installed = install_package(&req, &root, |p| events.push(p)).await.unwrap();

    assert!(installed.dir.join("main.c").is_file());
    assert!(installed.dir.join("bin/php").is_file());
    assert_eq!(std::fs::read_link(&installed.current_link).unwrap().to_str().unwrap(), "8.4.99");
    assert!(events.contains(&Progress::Verified));
    assert!(events.contains(&Progress::Extracted));
    assert!(events.contains(&Progress::Linked));
    // staging swept clean
    let mut staging_entries = std::fs::read_dir(root.staging_root()).unwrap();
    assert!(staging_entries.next().is_none(), "staging must be empty after success");
}

#[tokio::test]
async fn second_install_is_already_installed() {
    let archive = targz(&[("x", b"y")]);
    let sha = sha_hex(&archive);
    let url = serve_once(archive.clone());
    let home = tempfile::Builder::new().prefix("ovh-int2").tempdir_in("/tmp").unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let req = InstallRequest::new("php", "8.4", "8.4.98", &url, &sha, ArchiveFormat::TarGz).unwrap();
    install_package(&req, &root, |_| {}).await.unwrap();
    // second attempt (dest exists) — no server needed, pre-check fires
    let req2 = InstallRequest::new("php", "8.4", "8.4.98", &req.url.as_str(), &sha, ArchiveFormat::TarGz).unwrap();
    let err = install_package(&req2, &root, |_| {}).await.unwrap_err();
    assert!(matches!(err, openvhost_pkg::PkgError::AlreadyInstalled { .. }));
}
```

- [ ] **Step 3: Write the env-gated live proof**

`crates/openvhost-pkg/tests/live_net.rs`:

```rust
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

use openvhost_pkg::{install_package, ArchiveFormat, InstallRequest, PackagesRoot};

const PIN_URL: &str = "https://www.php.net/distributions/php-8.4.23.tar.gz";
const PIN_SHA: &str = "f43b69572cabfb91c023356f3ce197c782d8a255bc084c1a6af58c0e86cf7573";

#[tokio::test]
async fn installs_real_php_tarball() {
    if std::env::var("OPENVHOST_NET_TESTS").as_deref() != Ok("1") {
        eprintln!("SKIP live_net: set OPENVHOST_NET_TESTS=1 to run the real php.net download");
        return;
    }
    let home = tempfile::Builder::new().prefix("ovh-live").tempdir_in("/tmp").unwrap();
    let root = PackagesRoot::from_home(home.path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let req = InstallRequest::new("php", "8.4", "8.4.23", PIN_URL, PIN_SHA, ArchiveFormat::TarGz).unwrap();
    let installed = install_package(&req, &root, |_| {}).await.unwrap();
    // php source tarball has configure + main/php_version.h
    assert!(installed.dir.join("configure").is_file(), "expected configure at package root");
    assert!(installed.dir.join("main/php_version.h").is_file());
    assert_eq!(
        std::fs::read_link(&installed.current_link).unwrap().to_str().unwrap(),
        "8.4.23"
    );
    eprintln!("LIVE OK: installed {} at {}", "php-8.4.23", installed.dir.display());
}
```

- [ ] **Step 4: Run hermetic tests to green, then the live proof once**

```bash
cargo test -p openvhost-pkg 2>&1 | tail -8
OPENVHOST_NET_TESTS=1 cargo test -p openvhost-pkg --test live_net -- --nocapture 2>&1 | tail -8
```

Expected: hermetic integration + all unit tests pass; the live test prints `LIVE OK: installed php-8.4.23 ...` (no SKIP line). If php.net rotated the pin, update `PIN_URL`/`PIN_SHA` per the file's header comment and rerun.

- [ ] **Step 5: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-pkg && git commit -s -m "feat(pkg): install_package orchestration + hermetic e2e + gated php.net live proof"
```

---

### Task 8: Windows cross-check, docs, PR (security audit is the controller's gate)

**Files:**
- Modify: `crates/openvhost-pkg/README.md` is not required; instead Modify: `docs/superpowers/specs/2026-07-22-p06-pkg-pipeline-design.md` only if an implementation reality diverged from the spec (record it).
- No production code changes expected.

**Interfaces:** none — verification and delivery.

- [ ] **Step 1: License gate for the new deps**

```bash
cargo deny check licenses advisories 2>&1 | tail -20
```

Expected: exit 0. If a new dep's license is not on the allowlist, STOP and report — do not edit `deny.toml` without confirming GPLv3 compatibility (repo rule); `junction` and `futures-util` are permissive and should pass. Record the outcome for the PR body.

- [ ] **Step 2: (macOS-first — Windows cross-check DROPPED for v1)**

Per the owner's macOS-first scope decision (2026-07-22), this slice does NOT run the `x86_64-pc-windows-msvc` cross-check. The Windows `current`-link is an explicit-error stub (Task 6), the `junction` dep is removed, and Windows build/runtime verification moves to the future Windows-enablement phase. Nothing to do in this step — proceed to Step 3.

- [ ] **Step 3: Full local gate suite (the merge gate while CI is off)**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all green. The hermetic pkg tests run; `live_net` skips (no env var) — that's correct for the default suite.

- [ ] **Step 4: Push + PR**

```bash
git push -u origin feat/p06-pkg-pipeline
gh pr create --title "feat: P0-6 — download/verify/extract pipeline (openvhost-pkg)" --body "Implements docs/superpowers/specs/2026-07-22-p06-pkg-pipeline-design.md: streaming HTTPS download, SHA-256 verify-before-parse on the same handle, hardened two-pass tar.gz + zip extraction (traversal/type/collision/symlink/mode/cap guards, all reject-the-archive fail-closed), atomic install, and a per-major current link (unix symlink swap). No supervisor/app changes.

macOS-first (v1, owner decision 2026-07-22): the per-major current link ships for unix/macOS; Windows update_current is an explicit-error stub and the junction dep + Windows cross-check are deferred to a later Windows-enablement phase (junction design preserved in spec §6.2). Cross-platform extraction hardening (reserved names, ADS, traversal, collisions) stays in full — it is security-required and platform-agnostic.

Verification: full hermetic adversarial unit + integration suite green; env-gated php.net live proof installs php-8.4.23 (configure + main/php_version.h present, current -> 8.4.23).

SECURITY: download-verify code — MERGE-BLOCKED pending security-auditor APPROVE of this diff (CLAUDE.md golden rule 2). Spec §5 S1–S27 are the audit checklist (S22 Windows half deferred with the scope change above)."
```

- [ ] **Step 5: Hand back to controller** — the controller runs the final whole-branch review AND dispatches the **security-auditor to audit the real diff** (merge gate). Then the owner-visible smoke = the `OPENVHOST_NET_TESTS=1` live proof already captured. Do NOT merge from this task.

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** S1 (Task 1 `validate_https_url`), S2 (Task 5 debug-loopback), S3 (Task 1 reqwest features + Task 5 identity header), S4 (Task 5 cap), S5 (Task 5 timeouts), S6 (Task 5 no-resume/no-retry-on-mismatch), S7 (Task 5 client builder), S8 (Task 5 same-handle return + Task 7 threading), S9 (Task 5), S10 (Tasks 3/4 type whitelist), S11 (Task 2 `validate_entry_name`), S12 (Task 2 `collision_key` + Tasks 3/4 sets), S13 (Tasks 3/4 `create_new`), S14 (Task 2 `validate_symlink_target` + Task 3 deferred symlinks + Task 4 skip), S15 (Task 3 hardlink copy), S16 (Tasks 3/4 mode clamp), S17 (Tasks 3/4 byte caps), S18 (Task 2 `strip_single_root`), S19 (Task 3 `strip_quarantine`, macOS `xattr` dep), S20 (Task 6 staging lock + sweep), S21 (Task 6 install_dir), S22 (Task 6 platform links), S23 (doc note in spec), S24 (RECOMMENDED — disk preflight/retry deferred; acceptable), S25 (Task 7 semaphore), S26 (comments in Tasks 3/4), S27 (`tracing` dep present; add spans opportunistically). No unmet REQUIRED S-item.
- **Type consistency:** `PackagesRoot`, `InstallRequest`, `Progress`, `PkgError`, `PlannedKind`, `RawEntry`, `extract_targz`/`extract_zip`, `download_and_verify`, `install_dir`, `update_current`, `Staging` — names are consistent across tasks.
- **Known implementer hazards flagged in-plan (dispatch prompts must carry them):** the `fd_lock` guard-lifetime issue (Task 6 note — the fd-close-releases model is the prescribed fix; may need `libc`/`windows-sys` for a raw `flock`/`LockFileEx`, all license-clean); the debug-loopback test validity (Tasks 5/7 — `cargo test` builds debug so S2's carve-out is active); the zip `unix_permissions`/`SimpleFileOptions` API surface (Task 4 testkit — pin against the `zip = "2"` actually resolved, adjust the builder call if 2.x renamed it); and `reqwest::redirect::Attempt::error` taking `Box<dyn Error + Send + Sync>` (Task 5 — the `io_msg` helper matches that bound).
