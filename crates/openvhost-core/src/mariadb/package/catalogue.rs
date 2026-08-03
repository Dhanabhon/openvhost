// SPDX-License-Identifier: GPL-3.0-or-later
//! The compiled-in MariaDB package catalogue: which build this build of
//! OpenVHost will install, from where, and what its bytes must hash to
//! (build-pipeline design D5).
//!
//! **Compiled in, not fetched.** The public `openvhost/manifests` repository is
//! programme slice 6, deliberately: its schema should describe packages that
//! work rather than predict them. Until then the pins live here, where they are
//! reviewed, signed off and shipped with the binary — and, critically, where
//! nothing a user types can reach them. Identical in shape to
//! [`crate::mysql::MYSQL_PACKAGES`] on purpose (design D5, "consistency beats
//! novelty"): a second shape would be a finding, not a design.
//!
//! SECURITY: the `(url, sha256)` pair handed to the downloader comes only from
//! [`MARIADB_PACKAGES`]. The public entry points take a [`PackageTarget`] —
//! never a URL, never a hash — so there is no argument a caller can pass that
//! changes which bytes are fetched or what they must hash to. That is golden
//! rule 6 ("runtime download with SHA-256 verification only") expressed as an
//! API shape rather than a convention.
//!
//! PROVENANCE — and it differs from MySQL's in kind, not just in detail.
//! Oracle publishes macOS binaries, so the MySQL pin certifies bytes *upstream*
//! produced. **MariaDB publishes no macOS build at all**, so the digest below
//! is of a tarball *we* produced, on the owner's Mac, from
//! `build/recipes/mariadb.sh`. Three things stand behind it, none of them
//! assumed:
//!
//! 1. Upstream's GPG-signed `sha256sums.txt` was verified by the recipe against
//!    key `177F4010FE56CA3336300305F1656F24C74CD1D8` (MariaDB release signing,
//!    no expiry), the fingerprint cross-checked against `keyserver.ubuntu.com`
//!    — a different host from the one that serves the source. Verified
//!    2026-08-02.
//! 2. Every input MariaDB's own build system would otherwise have fetched for
//!    itself (`WITH_PCRE=bundled`, `WITH_LIBFMT=bundled`, checked upstream by
//!    `URL_MD5` and nothing else) was fetched and verified by the recipe first
//!    — pcre2 by GPG signature, fmt by digest — and the compile then ran with
//!    the network taken away, so an unverified fetch fails loudly instead of
//!    succeeding quietly.
//! 3. The finished tree passed all six points of the artifact contract
//!    (`build/audit.sh`, spec D6), including running from two different paths
//!    and serving SQL across a restart.
//!
//! **Single-builder trust, accepted explicitly by the owner (spec §13.1).**
//! There is no independent reproduction of these bytes. The build manifest
//! published beside the tarball is what makes the inputs auditable, and it is
//! mandatory rather than a nicety. **Redo all three checks before changing any
//! entry here** — a pin nobody traced back to a signature certifies "the bytes
//! someone built", not "the bytes upstream released".
//!
//! # §14 watch list — the security obligation, and its only trigger
//!
//! Leaving Homebrew makes us responsible for security updates: a user's
//! `brew upgrade` no longer reaches anything we ship, and they have no other
//! route to a fix. Spec §14 requires the watch list to live next to the
//! catalogue, so here it is. Check these, and move
//! [`MariadbPackage::last_checked_on`] when you do — a stale date is the only
//! signal this mechanism has.
//!
//! | Watch | Where | Note |
//! |---|---|---|
//! | MariaDB 11.4 releases | <https://mariadb.org/download/> | 11.4 LTS only (spec §13.3). A new major is a decision with a cost, not a configuration change. |
//! | OpenSSL 3.x advisories | <https://openssl-library.org/news/vulnerabilities/> | Linked **statically** into `bin/mariadbd` (spec §13.4), so `otool -L` will never show it and no linkage check can find it. 3.5.7 as of the pinned build. |
//! | pcre2 | <https://github.com/PCRE2Project/pcre2/releases> | Compiled in. **Its version is MariaDB's choice, not ours** — cmake insists on its own `URL_MD5` — so the answer to a pcre2 CVE is a MariaDB release that bumps it, never a number edited here. |
//! | fmt | <https://github.com/fmtlib/fmt/releases> | Compiled in; same qualification as pcre2. |
//!
//! A CVE in any of them is a rebuild, not a patch: re-verify upstream's
//! signature, rebuild through the same recipe, re-run the artifact contract,
//! publish, bump this file.

use openvhost_pkg::ArchiveFormat;

use crate::PackageTarget;
use crate::error::CoreError;

/// The directory name this runtime occupies in the package tree:
/// `packages/mariadb/<major>/<version>/`. A single definition so the installer,
/// the ledger and (later) discovery cannot drift to different spellings.
pub const MARIADB_PACKAGE_NAME: &str = "mariadb";

/// The one series this build publishes (spec §13.3: **11.4 LTS only** — no
/// 10.x, no 11.7). Every extra major is another tree to build, verify and
/// patch, and the §14 obligation above scales with that count.
pub const MARIADB_SERIES: &str = "11.4";

/// The binary exec'd once inside the staging directory so macOS pays its
/// first-execution signature check during the install, behind progress the user
/// is already watching, instead of on their first "Start".
///
/// **`bin/mariadbd`, never a `-safe` wrapper.** The tarball also ships
/// `bin/mariadbd-safe` and its `bin/mysqld_safe` alias, which are shell scripts
/// that genuinely try to start a server rather than print a version — warming
/// one would mean spawning a database against whatever datadir it resolved.
/// Inherited verbatim from the MySQL slice, where `mysqld_safe` carried a
/// hardcoded `/usr/local/mysql/data`; spec D8 makes that rule load-bearing,
/// because it is what keeps the residual embedded build paths inert.
pub const MARIADB_WARMUP_BINARY: &str = "bin/mariadbd";

/// Whether the release that would serve a pinned artifact actually exists yet.
///
/// A state where a state belongs, rather than a comment nobody reads or a
/// `bool` that says nothing about what to do next. **Publishing is owner-gated**
/// (spec D5 and the plan's global constraints): the build pipeline produces
/// artifacts locally, and creating a GitHub Release that hosts binaries is an
/// outward-facing act only the owner may perform. Until they do, the URL below
/// is where the bytes *will* live, not where they are.
///
/// Matched exhaustively at the one place it gates an install
/// (`crate::mariadb::install_mariadb_package`), never through a wildcard arm:
/// a third state would have to be decided about rather than silently treated
/// as installable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The release exists and `url` serves the pinned bytes.
    Published,
    /// The release does not exist yet, so `url` 404s. Carries the tag a human
    /// has to create — the whole point of modelling this is that the refusal
    /// can name the next action instead of surfacing as a network fault.
    AwaitingRelease {
        /// The release tag to publish, e.g. `"mariadb-11.4.9"` (design D5: one
        /// release per `<name>-<version>`, carrying the tarball, its `.sha256`
        /// and the build manifest).
        tag: &'static str,
    },
}

/// One pinned build: everything the install pipeline needs, and nothing it does
/// not.
///
/// Every field is a `&'static str` baked into the binary. There is no
/// constructor, and the type is only ever obtained by looking one up from
/// [`MARIADB_PACKAGES`] — outside this crate's own tests nothing can mint an
/// entry pointing somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MariadbPackage {
    /// The `major.minor` series, e.g. `"11.4"` — the tree level that shares a
    /// datadir and a configuration.
    pub major: &'static str,
    /// The exact release, e.g. `"11.4.9"`. This is the value recorded at
    /// install time (MySQL-from-tarball design D4); it is never recovered by
    /// probing.
    pub version: &'static str,
    /// Which host this artifact is for.
    pub target: PackageTarget,
    /// Where the bytes come from. HTTPS-only and re-validated on every redirect
    /// hop by `openvhost-pkg`. Meaningful only once [`Self::availability`] says
    /// [`Availability::Published`].
    pub url: &'static str,
    /// What the downloaded bytes must hash to, checked before anything parses
    /// them. This is the digest of **our** tarball — see this module's
    /// PROVENANCE note for what stands behind it.
    pub sha256: &'static str,
    /// How to unpack them. Carried explicitly rather than inferred from the URL
    /// suffix so a future entry in another container cannot be handed to the
    /// wrong extractor by a string that merely looks familiar.
    pub format: ArchiveFormat,
    /// Whether [`Self::url`] is live yet.
    pub availability: Availability,
    /// The date **upstream** released the source this artifact was built from,
    /// `YYYY-MM-DD`. Spec §14's first tripwire field: how old what we ship
    /// actually is, rather than how recently we happened to rebuild it.
    pub upstream_released_on: &'static str,
    /// The date a human last checked whether upstream has published something
    /// newer, `YYYY-MM-DD`. Spec §14's second tripwire field, and the one that
    /// does the work: **an accepted obligation with no trigger is an
    /// intention**, so the check becomes visible in source rather than
    /// remembered. If this date is more than a quarter old, the next action is
    /// to walk the watch list in this module's header and either move this date
    /// or open a rebuild slice.
    pub last_checked_on: &'static str,
}

/// Every MariaDB build this version of OpenVHost will install.
///
/// # !!! NOT PUBLISHED YET — this entry cannot be installed from the network !!!
///
/// The tarball exists, was audited and passes the artifact contract, but
/// **nothing has been pushed to GitHub Releases**: publishing is owner-gated
/// and has not happened. The URL below is the address the release *will* have
/// under design D5's one-release-per-`<name>-<version>` scheme, pinned now so
/// that publishing is a one-line change to [`Availability`] rather than an
/// invitation to invent a URL later. Today it 404s, and
/// [`Availability::AwaitingRelease`] is what stops that 404 ever reaching a
/// user: `crate::mariadb::install_mariadb_package` refuses before any network
/// work, naming the tag to publish.
///
/// # The pin below is STALE and the bytes it names must not be published
///
/// A security audit BLOCKed the artifact this hash was taken from, on
/// 2026-08-03. Its `mariadbd` resolves `basedir`, `plugin_dir` and
/// `character-sets-dir` out of `/private/tmp/openvhost-build/...`, and
/// `/private/tmp` is mode 1777 — so on a user's machine anything could create
/// that tree and plant a plugin dylib or a charset index for the server to load
/// (CWE-426 / CWE-427). Nothing was ever published, and
/// [`Availability::AwaitingRelease`] is why. `build/build.sh` now refuses to
/// build under a root with a world-writable ancestor, and contract check 7
/// rejects the artifact even if one somehow appeared.
///
/// **To publish** — step 1 is not optional, and the rest cannot be reached
/// without it, because the hash will not match until it is done:
///
/// 1. Prepare the build root once, then rebuild:
///    ```text
///    sudo mkdir -p /opt/openvhost-build
///    sudo chown "$(id -u):$(id -g)" /opt/openvhost-build
///    build/build.sh mariadb 11.4.9
///    ```
///    All seven contract checks must pass, twice — once on the staged tree and
///    once on the packed tarball. The driver runs both.
/// 2. Replace `sha256` below with the hash the `pack` stage printed, and run
///    `the_real_artifact_installs_and_runs_from_the_package_tree` (it is
///    `#[ignore]`d; set `OPENVHOST_MARIADB_TARBALL`) — it fails unless the
///    tarball on disk is the one this pin names.
/// 3. Create release `mariadb-11.4.9` carrying the tarball, its `.sha256` and
///    the build manifest, confirm the served bytes still hash to the new pin,
///    then flip `availability` to [`Availability::Published`].
///
/// **`macos-x86_64` is deliberately absent** and this slice does not add it:
/// there is no signature-checked x86_64 artifact, and shipping an unverified
/// pin to make a table look symmetrical is exactly the failure golden rule 6
/// exists to prevent. An Intel host gets an honest
/// [`CoreError::NoPackageForTarget`] rather than arm64 binaries.
pub const MARIADB_PACKAGES: [MariadbPackage; 1] = [MariadbPackage {
    major: MARIADB_SERIES,
    version: "11.4.9",
    target: PackageTarget::MacosArm64,
    url: "https://github.com/Dhanabhon/openvhost/releases/download/mariadb-11.4.9/mariadb-11.4.9-macos-arm64.tar.gz",
    sha256: "76ea96a4089e56953693d1af14e3ddd8da03cab291eada1fd1cf4e2c1df18304",
    format: ArchiveFormat::TarGz,
    availability: Availability::AwaitingRelease {
        tag: "mariadb-11.4.9",
    },
    // Mirrors `RECIPE_UPSTREAM_RELEASE_DATE` / `RECIPE_LAST_CHECKED` in
    // `build/recipes/mariadb.sh`, which records the same two dates in the build
    // manifest. `the_tripwire_dates_agree_with_the_recipe_that_built_the_bytes`
    // makes a drift between the two a test failure rather than a discrepancy
    // nobody reads.
    upstream_released_on: "2025-11-05",
    last_checked_on: "2026-08-02",
}];

/// The catalogue entry for `target`, or an error naming what is missing.
///
/// `target` is an `Option` so the "this host has no packages at all" case is an
/// ordinary value rather than a separate code path — and so both branches are
/// reachable from a test on any one machine.
///
/// **Takes no series argument, unlike [`crate::mysql::mysql_package_for_target`]
/// — the one deliberate departure from that function's shape.** Spec §13.3
/// settled MariaDB at 11.4 LTS only, and §13.3's point is that adding a major
/// is a decision with a cost rather than a value someone passes in. A parameter
/// whose only legal argument is [`MARIADB_SERIES`] would suggest otherwise.
/// When a second series is decided on, this grows the parameter and the callers
/// have to be revisited — which is the intended friction.
///
/// This does **not** report whether the entry can be fetched today; see
/// [`MariadbPackage::availability`]. Resolution and availability are separate
/// questions on purpose: a caller that only wants to display what this build
/// pins should not have to care that the release is unpublished.
pub fn mariadb_package_for_target(
    target: Option<PackageTarget>,
) -> Result<&'static MariadbPackage, CoreError> {
    let Some(target) = target else {
        return Err(CoreError::NoPackageForTarget {
            name: MARIADB_PACKAGE_NAME,
            version: MARIADB_SERIES.to_string(),
            target: "this host",
        });
    };
    MARIADB_PACKAGES
        .iter()
        .find(|p| p.target == target)
        .ok_or(CoreError::NoPackageForTarget {
            name: MARIADB_PACKAGE_NAME,
            version: MARIADB_SERIES.to_string(),
            target: target.as_str(),
        })
}

/// The catalogue entry for the host this binary was built for.
pub fn mariadb_package_for_host() -> Result<&'static MariadbPackage, CoreError> {
    mariadb_package_for_target(PackageTarget::host())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The recipe that produced the pinned bytes, read at compile time so the
    /// two records of the same facts cannot drift silently. Test-only: the
    /// production build has no dependency on `build/`.
    const RECIPE: &str = include_str!("../../../../../build/recipes/mariadb.sh");

    // ------------------------------------------------------------------
    // Group 1 — the pinned entry, byte for byte.
    //
    // These assertions exist so that changing the pin cannot be a quiet
    // one-character edit: the hash and URL are repeated here as literals, and
    // moving either without redoing the provenance work recorded at the top of
    // this module breaks the build.
    //
    // Vacuity: every assertion here is an equality against a literal that
    // appears nowhere else in the test, so any edit to the entry fails it.
    // Proven by mutation — flipping one hex digit of the pinned sha256 turned
    // `the_catalogue_pins_exactly_one_mariadb_build_today` red on its own, and
    // pointing `MARIADB_PACKAGE_NAME` at "mysql" reddened this group plus every
    // install test.
    // ------------------------------------------------------------------

    #[test]
    fn the_catalogue_pins_exactly_one_mariadb_build_today() {
        assert_eq!(MARIADB_PACKAGES.len(), 1);
        let e = &MARIADB_PACKAGES[0];
        assert_eq!(e.major, "11.4");
        assert_eq!(e.version, "11.4.9");
        assert_eq!(e.target, PackageTarget::MacosArm64);
        assert_eq!(
            e.url,
            "https://github.com/Dhanabhon/openvhost/releases/download/mariadb-11.4.9/\
             mariadb-11.4.9-macos-arm64.tar.gz"
        );
        assert_eq!(
            e.sha256,
            "76ea96a4089e56953693d1af14e3ddd8da03cab291eada1fd1cf4e2c1df18304"
        );
        assert_eq!(e.format, ArchiveFormat::TarGz);
    }

    #[test]
    fn every_entry_is_https_and_carries_a_well_formed_lowercase_sha256() {
        for e in &MARIADB_PACKAGES {
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
    fn every_entry_is_a_version_of_the_series_it_is_filed_under() {
        for e in &MARIADB_PACKAGES {
            assert_eq!(e.major, MARIADB_SERIES, "spec §13.3 pins 11.4 LTS only");
            assert!(
                e.version.starts_with(&format!("{}.", e.major)),
                "{} is filed under major {}",
                e.version,
                e.major
            );
        }
    }

    #[test]
    fn the_package_name_is_a_single_safe_path_component() {
        assert_eq!(MARIADB_PACKAGE_NAME, "mariadb");
        assert!(!MARIADB_PACKAGE_NAME.contains('/'));
        assert!(!MARIADB_PACKAGE_NAME.contains('.'));
    }

    /// The trap inherited from the MySQL slice: the tarball ships
    /// `bin/mariadbd-safe` and `bin/mysqld_safe` beside `bin/mariadbd`, and
    /// both wrappers start a real server rather than printing a version.
    #[test]
    fn the_warm_up_binary_is_mariadbd_and_never_a_safe_wrapper() {
        assert_eq!(MARIADB_WARMUP_BINARY, "bin/mariadbd");
        assert!(!MARIADB_WARMUP_BINARY.contains("safe"));
    }

    // ------------------------------------------------------------------
    // Group 2 — the §14 tripwire.
    //
    // Vacuity: `RECIPE` is a real file read at compile time and the assertions
    // are `contains` against strings built from the catalogue, so they go red
    // the moment either record moves without the other. Proven by mutation —
    // changing `last_checked_on` to "2026-08-03" (a plausible typo, since the
    // recipe's *vendored* check really is that date) failed the agreement test
    // while every other test stayed green.
    // ------------------------------------------------------------------

    #[test]
    fn every_entry_carries_both_dates_the_security_obligation_needs() {
        let is_iso_date = |s: &str| {
            s.len() == 10
                && s.as_bytes()[4] == b'-'
                && s.as_bytes()[7] == b'-'
                && s.bytes().filter(|b| b.is_ascii_digit()).count() == 8
        };
        for e in &MARIADB_PACKAGES {
            assert!(
                is_iso_date(e.upstream_released_on),
                "{} upstream_released_on {:?} is not YYYY-MM-DD",
                e.version,
                e.upstream_released_on
            );
            assert!(
                is_iso_date(e.last_checked_on),
                "{} last_checked_on {:?} is not YYYY-MM-DD",
                e.version,
                e.last_checked_on
            );
            // A check that predates the release it is checking is not a check.
            // ISO dates sort lexicographically, which is the whole reason the
            // format is pinned above.
            assert!(
                e.last_checked_on >= e.upstream_released_on,
                "{}: last checked {} but upstream released {} — the dates are \
                 transposed, or the pin was never re-checked after the bump",
                e.version,
                e.last_checked_on,
                e.upstream_released_on
            );
        }
    }

    /// Spec §14's tripwire lives in two files — this catalogue and the recipe
    /// that produced the bytes — and two records of one fact drift. Reading the
    /// recipe makes that drift a test failure instead of a discrepancy nobody
    /// notices during a CVE response.
    #[test]
    fn the_tripwire_dates_agree_with_the_recipe_that_built_the_bytes() {
        let e = &MARIADB_PACKAGES[0];
        for (field, want) in [
            (
                "RECIPE_UPSTREAM_RELEASE_DATE",
                format!(
                    "RECIPE_UPSTREAM_RELEASE_DATE=\"{}\"",
                    e.upstream_released_on
                ),
            ),
            (
                "RECIPE_LAST_CHECKED",
                format!("RECIPE_LAST_CHECKED=\"{}\"", e.last_checked_on),
            ),
        ] {
            assert!(
                RECIPE.contains(&want),
                "build/recipes/mariadb.sh does not carry {want:?}; the catalogue and \
                 the recipe disagree about {field}"
            );
        }
    }

    /// Non-vacuity twin for the test above: it must be reading a real recipe
    /// with real content, not an empty string that makes `contains` trivially
    /// satisfiable — and it must be the recipe for the version we pin.
    #[test]
    fn the_recipe_the_dates_are_checked_against_is_the_one_that_built_this_pin() {
        assert!(RECIPE.len() > 1000, "recipe read as {} bytes", RECIPE.len());
        assert!(!RECIPE.contains("RECIPE_UPSTREAM_RELEASE_DATE=\"1970-01-01\""));
        assert!(RECIPE.contains("mariadb"), "that is not the MariaDB recipe");
    }

    // ------------------------------------------------------------------
    // Group 3 — the entry is honestly marked unpublished.
    //
    // Vacuity: the assertions are on `Availability`, matched exhaustively, so a
    // flip to `Published` fails them. That is intended — publishing is supposed
    // to be a reviewed change, and these tests are the review's checklist.
    // Proven by mutation: setting `availability: Availability::Published`
    // turned exactly this group red and left the rest of the catalogue green.
    // ------------------------------------------------------------------

    /// Publishing is owner-gated and has not happened. When it does, this test
    /// is the checklist: create the release, re-verify the served bytes against
    /// the pin, then change this test and the entry together.
    #[test]
    fn the_pinned_release_is_marked_as_not_yet_published() {
        let e = &MARIADB_PACKAGES[0];
        match e.availability {
            Availability::AwaitingRelease { tag } => {
                assert_eq!(tag, "mariadb-11.4.9");
                assert!(
                    e.url.contains(tag),
                    "the pinned url {} does not name the release tag {tag} it is \
                     waiting on",
                    e.url
                );
            }
            Availability::Published => panic!(
                "the catalogue now claims {} is published. Confirm the release really \
                 exists and serves bytes hashing to {}, then update this test.",
                e.version, e.sha256
            ),
        }
    }

    // ------------------------------------------------------------------
    // Group 4 — target selection.
    //
    // Vacuity: each case asserts a distinct outcome for a distinct input, and
    // the refusals assert the reason text, not merely `is_err()`. Proven by
    // mutation — deleting the `p.target == target` filter made Intel resolve to
    // the arm64 build and reddened
    // `intel_gets_an_honest_refusal_rather_than_the_arm64_build` alone. The
    // unsupported-host case stayed green, correctly: it returns before the
    // filter is reached, which is why the two refusals are separate tests.
    // ------------------------------------------------------------------

    #[test]
    fn apple_silicon_resolves_to_the_pinned_arm64_build() {
        let entry = mariadb_package_for_target(Some(PackageTarget::MacosArm64)).unwrap();
        assert_eq!(entry.version, "11.4.9");
        assert_eq!(entry.target, PackageTarget::MacosArm64);
        assert!(entry.url.contains("arm64"));
    }

    /// The catastrophic silent bug this rules out: handing arm64 binaries to an
    /// Intel host because the lookup ignored the target. Intel must get a
    /// refusal, and it must name the target it could not serve.
    #[test]
    fn intel_gets_an_honest_refusal_rather_than_the_arm64_build() {
        let err = mariadb_package_for_target(Some(PackageTarget::MacosX86_64)).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "mariadb");
                assert_eq!(version, "11.4");
                assert_eq!(target, "macos-x86_64");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("macos-x86_64"), "got {err}");
        assert!(err.to_string().contains("mariadb"), "got {err}");
    }

    #[test]
    fn an_unsupported_host_is_refused_and_says_so() {
        let err = mariadb_package_for_target(None).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "mariadb");
                assert_eq!(version, "11.4");
                assert_eq!(target, "this host");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("this host"), "got {err}");
    }

    /// On the machines this slice ships for, the host lookup and the explicit
    /// arm64 lookup must be the same answer — otherwise
    /// [`mariadb_package_for_host`] could be resolving through some path the
    /// target-explicit tests never cover.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_lookup_agrees_with_the_explicit_arm64_lookup() {
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            mariadb_package_for_host().unwrap(),
            mariadb_package_for_target(Some(PackageTarget::MacosArm64)).unwrap()
        );
    }
}
