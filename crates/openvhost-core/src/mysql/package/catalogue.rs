// SPDX-License-Identifier: GPL-3.0-or-later
//! The compiled-in MySQL package catalogue: which prebuilt upstream build this
//! build of OpenVHost will install, from where, and what its bytes must hash
//! to (MySQL-from-tarball design D2).
//!
//! **Compiled in, not fetched.** The public `openvhost/manifests` repository is
//! a later slice, deliberately: its schema should describe packages that work
//! rather than predict them. Until then the pins live here, where they are
//! reviewed, signed off and shipped with the binary — and, critically, where
//! nothing a user types can reach them.
//!
//! SECURITY: the `(url, sha256)` pair handed to the downloader comes only from
//! [`MYSQL_PACKAGES`]. The public entry points take a [`MysqlMajor`] and a
//! [`PackageTarget`] — never a URL, never a hash — so there is no argument a
//! caller can pass that changes which bytes are fetched or what they must hash
//! to. That is golden rule 6 ("runtime download with SHA-256 verification
//! only") expressed as an API shape rather than a convention.
//!
//! PROVENANCE: Oracle publishes an MD5 and a detached PGP signature, no
//! SHA-256 sidecar, so the pin below was computed by us and is only worth
//! anything because the signature was checked first. Verified 2026-08-01: key
//! `BCA43417C3B485DD128EC6D4B7B3B788A8D3785C` (MySQL Release Engineering,
//! valid to 2027-10-23), fingerprint cross-checked against `dev.mysql.com/doc`
//! — a different host from the one the key was fetched from — `gpg --verify`
//! good on the artifact, and the signed bytes hashing to exactly the value
//! below. **Redo that check before changing any entry here.** A pin nobody
//! traced back to a signature certifies "the bytes someone downloaded", not
//! "the bytes Oracle published".

use openvhost_pkg::ArchiveFormat;

use crate::error::CoreError;
use crate::mysql::MysqlMajor;

/// The directory name this runtime occupies in the package tree:
/// `packages/mysql/<major>/<version>/`. A single definition so the installer,
/// the ledger and (later) discovery cannot drift to different spellings.
pub const MYSQL_PACKAGE_NAME: &str = "mysql";

/// The binary exec'd once inside the staging directory so macOS pays its
/// first-execution signature check during the install, behind progress the
/// user is already watching, instead of on their first "Start". Measured on
/// this payload: 809 ms cold, 16 ms warm, and the validation survives
/// `rename(2)`, so paying it in staging covers the installed copy too.
///
/// **`bin/mysqld`, never `bin/mysqld_safe`.** `mysqld_safe` is a shell wrapper
/// carrying a hardcoded `/usr/local/mysql/data`, and it genuinely tries to
/// start a server rather than print a version — warming it would mean spawning
/// a database against a datadir path this project does not own.
pub const MYSQL_WARMUP_BINARY: &str = "bin/mysqld";

/// An OS/architecture pair a prebuilt package can be published for.
///
/// Exhaustive on purpose and matched exhaustively everywhere: adding a target
/// must break compilation at every site that has to make a decision about it,
/// rather than silently falling into a wildcard arm that resolves to the wrong
/// architecture's binaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageTarget {
    /// Apple Silicon macOS.
    MacosArm64,
    /// Intel macOS.
    MacosX86_64,
}

impl PackageTarget {
    /// The stable, user-facing spelling — also what appears in error messages
    /// when this build publishes nothing for a target.
    pub fn as_str(self) -> &'static str {
        match self {
            PackageTarget::MacosArm64 => "macos-arm64",
            PackageTarget::MacosX86_64 => "macos-x86_64",
        }
    }

    /// The target *this binary was compiled for*, or `None` on a host this
    /// programme publishes no packages for.
    ///
    /// Deliberately `cfg`-derived rather than probed at runtime: it answers
    /// "which artifact matches the code that is executing", and an
    /// x86_64 build running under Rosetta on Apple Silicon correctly wants the
    /// x86_64 artifact. It is not a claim about the CPU in the machine.
    pub const fn host() -> Option<PackageTarget> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Some(PackageTarget::MacosArm64)
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Some(PackageTarget::MacosX86_64)
        }
        #[cfg(not(all(
            target_os = "macos",
            any(target_arch = "aarch64", target_arch = "x86_64")
        )))]
        {
            None
        }
    }
}

/// One pinned upstream build: everything the install pipeline needs, and
/// nothing it does not.
///
/// Every field is a `&'static str` baked into the binary. There is no
/// constructor, and the type is only ever obtained by looking one up from
/// [`MYSQL_PACKAGES`] — outside this crate's own tests nothing can mint an
/// entry pointing somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MysqlPackage {
    /// The `major.minor` series, e.g. `"8.4"` — the tree level that shares a
    /// datadir and a configuration.
    pub major: &'static str,
    /// The exact upstream release, e.g. `"8.4.11"`. This is the value recorded
    /// at install time (design D4); it is never recovered by probing.
    pub version: &'static str,
    /// Which host this artifact is for.
    pub target: PackageTarget,
    /// Where the bytes come from. HTTPS-only and re-validated on every
    /// redirect hop by `openvhost-pkg`.
    pub url: &'static str,
    /// What the downloaded bytes must hash to, checked before anything parses
    /// them.
    pub sha256: &'static str,
    /// How to unpack them. Carried explicitly rather than inferred from the
    /// URL suffix so a future entry in another container cannot be handed to
    /// the wrong extractor by a string that merely looks familiar.
    pub format: ArchiveFormat,
}

/// Every MySQL build this version of OpenVHost will install.
///
/// One entry today. **`macos-x86_64` is deliberately absent**: Oracle does
/// publish that artifact, but its bytes have not been through the signature
/// check recorded at the top of this module, and shipping an unverified pin to
/// make a table look symmetrical is exactly the failure golden rule 6 exists to
/// prevent. An Intel host therefore gets an honest
/// [`CoreError::NoPackageForTarget`] rather than arm64 binaries.
///
/// Trap for whoever adds the next entry: the OS tag in the URL is
/// **version-coupled**, not derivable. `macos15` is correct for 8.4.10 and
/// 8.4.11; the same path with `macos14` 404s. It cannot be templated from the
/// MySQL version — pin it per release.
pub const MYSQL_PACKAGES: [MysqlPackage; 1] = [MysqlPackage {
    major: "8.4",
    version: "8.4.11",
    target: PackageTarget::MacosArm64,
    url: "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz",
    sha256: "b96e00493bc3499b9ffd7f08d65c5d64933af0383a8287d9873b64f94c2d6009",
    format: ArchiveFormat::TarGz,
}];

/// The catalogue entry for `major` on `target`, or an error naming what is
/// missing.
///
/// `target` is an `Option` so the "this host has no packages at all" case is
/// an ordinary value rather than a separate code path — and so both branches
/// are reachable from a test on any one machine.
pub fn mysql_package_for_target(
    major: &MysqlMajor,
    target: Option<PackageTarget>,
) -> Result<&'static MysqlPackage, CoreError> {
    let Some(target) = target else {
        return Err(CoreError::NoPackageForTarget {
            name: MYSQL_PACKAGE_NAME,
            version: major.as_str().to_string(),
            target: "this host",
        });
    };
    MYSQL_PACKAGES
        .iter()
        .find(|p| p.major == major.as_str() && p.target == target)
        .ok_or(CoreError::NoPackageForTarget {
            name: MYSQL_PACKAGE_NAME,
            version: major.as_str().to_string(),
            target: target.as_str(),
        })
}

/// The catalogue entry for `major` on the host this binary was built for.
pub fn mysql_package_for_host(major: &MysqlMajor) -> Result<&'static MysqlPackage, CoreError> {
    mysql_package_for_target(major, PackageTarget::host())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mysql::MYSQL_CATALOGUE;

    // ------------------------------------------------------------------
    // Group 1 — the pinned entry, byte for byte.
    //
    // These assertions exist so that changing the pin cannot be a quiet
    // one-character edit: the hash and URL are repeated here as literals, and
    // moving either without redoing the PGP provenance check recorded at the
    // top of this module breaks the build.
    // ------------------------------------------------------------------

    #[test]
    fn the_catalogue_pins_exactly_one_verified_mysql_build_today() {
        assert_eq!(MYSQL_PACKAGES.len(), 1);
        let e = &MYSQL_PACKAGES[0];
        assert_eq!(e.major, "8.4");
        assert_eq!(e.version, "8.4.11");
        assert_eq!(e.target, PackageTarget::MacosArm64);
        assert_eq!(
            e.url,
            "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz"
        );
        assert_eq!(
            e.sha256,
            "b96e00493bc3499b9ffd7f08d65c5d64933af0383a8287d9873b64f94c2d6009"
        );
        assert_eq!(e.format, ArchiveFormat::TarGz);
    }

    #[test]
    fn every_entry_is_https_and_carries_a_well_formed_lowercase_sha256() {
        for e in &MYSQL_PACKAGES {
            assert!(e.url.starts_with("https://"), "{} is not https", e.url);
            assert_eq!(e.sha256.len(), 64, "{} sha is not 64 chars", e.version);
            assert!(
                e.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{} sha is not lowercase hex",
                e.version
            );
        }
    }

    #[test]
    fn every_entry_is_a_version_of_the_major_it_is_filed_under() {
        for e in &MYSQL_PACKAGES {
            assert!(
                e.version == e.major || e.version.starts_with(&format!("{}.", e.major)),
                "{} is filed under major {}",
                e.version,
                e.major
            );
        }
    }

    #[test]
    fn the_urls_os_tag_is_pinned_per_release_and_not_templated_from_the_version() {
        // The trap this guards: `macos15` is correct for 8.4.11 and `macos14`
        // 404s, so a helpful refactor that builds the URL from the version
        // would produce a dead link. Assert the tag is a literal in the URL,
        // not something the version can imply.
        let e = &MYSQL_PACKAGES[0];
        assert!(e.url.contains("macos15"), "got {}", e.url);
        assert!(!e.url.contains(&format!("macos{}", e.version)));
    }

    #[test]
    fn the_warm_up_binary_is_mysqld_and_never_the_mysqld_safe_wrapper() {
        // `mysqld_safe` hardcodes /usr/local/mysql/data and really does start a
        // server; warming it would spawn a database against a path this
        // project does not own.
        assert_eq!(MYSQL_WARMUP_BINARY, "bin/mysqld");
        assert!(!MYSQL_WARMUP_BINARY.contains("mysqld_safe"));
    }

    #[test]
    fn the_package_name_is_a_single_safe_path_component() {
        assert_eq!(MYSQL_PACKAGE_NAME, "mysql");
        assert!(!MYSQL_PACKAGE_NAME.contains('/'));
        assert!(!MYSQL_PACKAGE_NAME.contains('.'));
    }

    /// The brew catalogue ("majors this build offers to install") and the
    /// package catalogue ("majors this build can actually fetch") must agree,
    /// or the UI offers an install that cannot resolve.
    #[test]
    fn every_offered_major_has_an_apple_silicon_package() {
        for major in MYSQL_CATALOGUE {
            let parsed = MysqlMajor::parse(major).unwrap();
            assert!(
                mysql_package_for_target(&parsed, Some(PackageTarget::MacosArm64)).is_ok(),
                "MYSQL_CATALOGUE offers {major} but no arm64 package is pinned for it"
            );
        }
    }

    // ------------------------------------------------------------------
    // Group 2 — target selection.
    // ------------------------------------------------------------------

    #[test]
    fn a_target_renders_as_its_stable_public_spelling() {
        assert_eq!(PackageTarget::MacosArm64.as_str(), "macos-arm64");
        assert_eq!(PackageTarget::MacosX86_64.as_str(), "macos-x86_64");
    }

    #[test]
    fn apple_silicon_resolves_to_the_pinned_arm64_build() {
        let major = MysqlMajor::parse("8.4").unwrap();
        let entry = mysql_package_for_target(&major, Some(PackageTarget::MacosArm64)).unwrap();
        assert_eq!(entry.version, "8.4.11");
        assert_eq!(entry.target, PackageTarget::MacosArm64);
        assert!(entry.url.contains("arm64"));
    }

    /// The catastrophic silent bug this rules out: handing arm64 binaries to
    /// an Intel host because the lookup ignored the target. Intel must get a
    /// refusal, and it must name the target it could not serve.
    #[test]
    fn intel_gets_an_honest_refusal_rather_than_the_arm64_build() {
        let major = MysqlMajor::parse("8.4").unwrap();
        let err = mysql_package_for_target(&major, Some(PackageTarget::MacosX86_64)).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "mysql");
                assert_eq!(version, "8.4");
                assert_eq!(target, "macos-x86_64");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("macos-x86_64"), "got {err}");
    }

    #[test]
    fn an_unsupported_host_is_refused_and_says_so() {
        let major = MysqlMajor::parse("8.4").unwrap();
        let err = mysql_package_for_target(&major, None).unwrap_err();
        match err {
            CoreError::NoPackageForTarget { target, .. } => assert_eq!(target, "this host"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn a_major_with_no_pinned_build_is_refused_on_a_supported_target() {
        // Shape-valid, discoverable, and simply not something we publish.
        let discovered_only = MysqlMajor::from_probe("9.7".to_string()).unwrap();
        let err = mysql_package_for_target(&discovered_only, Some(PackageTarget::MacosArm64))
            .unwrap_err();
        match err {
            CoreError::NoPackageForTarget { ref version, .. } => assert_eq!(version, "9.7"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// On the machines this slice ships for, the host lookup and the explicit
    /// arm64 lookup must be the same answer — otherwise `mysql_package_for_host`
    /// could be resolving through some path the target-explicit tests never
    /// cover.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_lookup_agrees_with_the_explicit_arm64_lookup() {
        let major = MysqlMajor::parse("8.4").unwrap();
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            mysql_package_for_host(&major).unwrap(),
            mysql_package_for_target(&major, Some(PackageTarget::MacosArm64)).unwrap()
        );
    }
}
