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
//! 3. The finished tree passed all seven points of the artifact contract
//!    (`build/audit.sh`, spec D6), including running from two different paths
//!    and serving SQL across a restart.
//!
//! **The digest below names a REPACKED tarball** (reproducible-pack design
//! §5.3). `build/build.sh mariadb 11.4.9 --from pack` re-cut it from the staged
//! prefix `/opt/openvhost-build/mariadb-11.4.9` so that the pin would name bytes
//! the pipeline can *reproduce*. The three checks above stand unchanged, and not
//! by assumption: every one of the 16 882 files in the previously pinned tarball
//! has the same SHA-256 as the file at the same path in the prefix that was
//! repacked, and `gunzip -c` of both tarballs gives the same raw tar,
//! `1d55a367c1d519a9a525fb8439dc54b4d07f524e104deb38a4ef69a1cebe6232` — same
//! entries, same modes, same mtimes. The whole difference is the four-byte MTIME
//! field gzip writes into its own header from the clock. All seven contract
//! checks were re-run against the repacked tarball rather than carried over.
//!
//! **Three costs, recorded rather than hidden — not one.** The repack's
//! manifest carries `resumed_from: "pack"`, and three fields degraded
//! between the manifest beside the previous pin and this one:
//! `configure_flags` went from 24 entries — the entire plugin-disable set
//! among them (`-DPLUGIN_CONNECT=NO`, `-DPLUGIN_OQGRAPH=NO`,
//! `-DPLUGIN_S3=NO`, …) — to `[]`, because `--from pack` skips
//! `recipe_configure`, the only stage that calls `bp_record_flags`.
//! `recipe.vendored_on_disk` is `[]` for the same reason: that block records
//! the digests of the pcre2 and fmt archives *as the build read them*, and
//! the work tree holding them was cleaned up long ago, so there is nothing
//! left to hash. **Read that as a fact about this one frozen manifest, not as
//! a rule** — `[]` was ambiguous precisely because it is also what a walk that
//! looked and found nothing prints, and the recipe no longer emits it for a
//! directory it never opened. A `--from pack` run cut today writes
//! `"vendored_on_disk": null` with a `vendored_on_disk_not_observed` sentence
//! beside it; the `[]` above predates that and still needs this paragraph.
//! And `recipe.bison.path` / `.version` — which bison actually
//! built the parser — are both `""`, because bison is discovered inside
//! `recipe_configure` too. **Which plugins exist in the shipped server is
//! security-relevant configuration**, so none of this is cosmetic.
//! `recipe.vendored` — what was pinned and verified — is unchanged, and the
//! manifest cut beside the 2026-08-03 build still carries a populated
//! `configure_flags`, both on-disk digests, and the bison path and version.
//! Only a full rebuild puts any of the three back in a current manifest.
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
/// # History: the pin this one replaced, and why it was unpublishable
///
/// **This section is past tense on purpose.** It described a live hazard until
/// PR #52 rebuilt under `/opt`, and it kept saying so afterwards — a security
/// statement that outlived its subject and then, when the pin moved again for
/// reproducibility, was warning about bytes that no longer existed. A stale
/// warning about a hash is worse than none: the next reader either believes a
/// false claim or learns to skim the warnings.
///
/// A security audit BLOCKed an *earlier* artifact on 2026-08-03. Its
/// `mariadbd` resolved `basedir`, `plugin_dir` and `character-sets-dir` out of
/// `/private/tmp/openvhost-build/...`, and `/private/tmp` is mode 1777 — so on
/// a user's machine anything could create that tree and plant a plugin dylib or
/// a charset index for the server to load (CWE-426 / CWE-427).
///
/// Nothing was ever published, and [`Availability::AwaitingRelease`] is why.
/// Two durable guards came out of it and both still hold: `build/build.sh`
/// refuses to build under a root with a world-writable ancestor, and contract
/// check 7 rejects an artifact carrying such a path even if one appeared.
/// **"A neutral prefix is not an inert one"** is the sentence worth keeping.
///
/// # What this pin's manifest does not say
///
/// Its manifest **predates** the `dependencies` block `build/build.sh` now
/// writes, so for its one `RECIPE_DEPENDS` entry it records `"version":
/// "3.5.7"` and nothing else — and a version string cannot tell one build of
/// OpenSSL 3.5.7 from another. nginx drifted 611 bytes from its own pin
/// exactly that way, invisibly.
///
/// What is honestly known here: the staged prefix these bytes are packed from
/// dates to 2026-08-03 and nothing has rebuilt it since, which is why
/// repacking it still reproduces this `sha256`. What is **not** known is which
/// build of OpenSSL is statically inside it. That shared `openssl-3.5.7`
/// prefix has been rebuilt at least twice since — once on 2026-08-07,
/// silently, and once deliberately for nginx's re-pin — so the prefix standing
/// on the builder today is certainly not the one these bytes link against.
///
/// **Regenerating this manifest would not answer that; it would guess.** A
/// `--from pack` run digests whatever prefix is on disk at manifest time, so
/// it would write today's OpenSSL into a record of an August 3 build — and
/// because a sidecar cannot change a tarball, the `sha256` would come back
/// identical and the check would pass while the manifest acquired a precise,
/// confident, wrong claim. The driver now refuses to make it, recording
/// `"not_observed"` instead. This entry gains a real dependency digest when it
/// is next built from source, and not before.
///
/// # To publish
///
/// 1. Confirm the pin below still reproduces: `build/build.sh mariadb 11.4.9`
///    (or `--from pack --keep-work` against an existing staged prefix) must
///    print this exact hash. Since the pack stage stopped writing a timestamp
///    into the gzip header, that is a real check rather than a formality.
/// 2. **All seven contract checks must pass.** A full `build/build.sh` run
///    (no `--from`) gets both runs for free: its `audit` stage runs them
///    against the staged tree, and `verify-artifact` runs them again against
///    the packed tarball. The `--from pack` shortcut in step 1 does not — it
///    starts past `audit`, so it exercises the contract once, against the
///    tarball only. Taking that shortcut means confirming the staged tree
///    already has a passing, current audit behind it, not assuming one.
/// 3. Run `the_real_artifact_installs_and_runs_from_the_package_tree` (it is
///    `#[ignore]`d; set `OPENVHOST_MARIADB_TARBALL`) — it fails unless the
///    tarball on disk is the one this pin names.
/// 4. Create release `mariadb-11.4.9` carrying the tarball, its `.sha256` and
///    the build manifest, confirm the served bytes still hash to this pin, then
///    flip `availability` to [`Availability::Published`].
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
    sha256: "854c34dcafef29dc72af2bcbd6d66271ae2e6167ab45e33c4f744d163675aeb0",
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

    /// The build manifest the driver wrote beside the tarball this entry pins,
    /// committed under `build/manifests/` and read at compile time. The recipe
    /// above says what a build *would* do; this says what one *did*, and it is
    /// the only thing in the repository that ties `sha256` below to an account
    /// of how those bytes were produced. Test-only, like `RECIPE`.
    ///
    /// This one is a repack (`resumed_from: "pack"`, `configure_flags: []`) —
    /// how PR #67 re-cut this pin, and a true account of that run rather than a
    /// degraded copy of a better file. It also carries no `dependencies` block,
    /// which is a **separate fact with a separate cause**: being a repack does
    /// not lose that block — the driver emits it unconditionally, with
    /// `"tree_sha256": null`, for any recipe declaring `RECIPE_DEPENDS`. This
    /// manifest simply **predates** it, exactly as the PROVENANCE note above
    /// says. See `build/manifests/README.md`.
    const MANIFEST: &str =
        include_str!("../../../../../build/manifests/mariadb-11.4.9-macos-arm64.manifest.json");

    /// MariaDB's release-signing key primary fingerprint and the pinned digest
    /// of upstream's source tarball, restated here so they are checked rather
    /// than merely stated in the PROVENANCE prose above. Both are facts the
    /// recipe and the manifest each record independently, and until now this
    /// catalogue bound neither — only the two dates — while nginx's twin bound
    /// its key and PHP's bound version, digest and key together.
    const SIGNING_KEY_FPR: &str = "177F4010FE56CA3336300305F1656F24C74CD1D8";
    const SOURCE_SHA256: &str = "8e481ca29b5a740444d45451c8ea2d93711cf525d6fa5d27bc9512cf8973b075";

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
            "854c34dcafef29dc72af2bcbd6d66271ae2e6167ab45e33c4f744d163675aeb0"
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
    ///
    /// Levelled 2026-08-09 to bind what the other two catalogues bind. It
    /// checked the two dates and nothing else, while nginx's twin also bound
    /// its signing-key fingerprint and PHP's bound version, source digest and
    /// key fingerprint together — so a MariaDB key rotation or a re-pinned
    /// source tarball could move in the recipe with this test staying green,
    /// which is precisely the drift it exists to refuse. The fingerprint
    /// matters more than either date: it is what `bp_gpg_verify_signature`
    /// actually verified the source against.
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
            (
                "RECIPE_SIGNING_KEY_FPR",
                format!("RECIPE_SIGNING_KEY_FPR=\"{SIGNING_KEY_FPR}\""),
            ),
            (
                "RECIPE_SOURCE_SHA256",
                format!("RECIPE_SOURCE_SHA256=\"{SOURCE_SHA256}\""),
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

    /// The committed manifest is the record of how the pinned bytes were made,
    /// and `output.sha256` is what ties that record to them — every other
    /// assertion here is secondary to it. Until `build/manifests/` existed, no
    /// file in this repository named this digest except the entry below, so the
    /// account of its provenance was prose. This makes a pin bumped without its
    /// manifest, or a manifest swapped for another build's, a test failure.
    ///
    /// `upstream.sha256` is checked too: this manifest is a repack and so
    /// carries no `configure_flags`, which leaves the source digest as the
    /// strongest statement it makes about what went *into* the artifact.
    ///
    /// Vacuity: `serde_json::from_str` is a real parse, so a truncated or
    /// malformed manifest fails here rather than quietly satisfying a substring
    /// search, and a missing key deserialises to `Value::Null`, whose `as_str()`
    /// is `None` and never equals the `Some(_)` expected. No separate
    /// non-vacuity twin is written: there is no way for this test to pass
    /// without a real manifest for this artifact, so a twin asserting that
    /// would be a check whose passing is guaranteed by the same fact that makes
    /// its failure invisible.
    #[test]
    fn the_committed_manifest_describes_the_bytes_this_entry_pins() {
        let e = &MARIADB_PACKAGES[0];
        let m: serde_json::Value = serde_json::from_str(MANIFEST)
            .expect("build/manifests/mariadb-11.4.9-macos-arm64.manifest.json is not valid JSON");
        assert_eq!(
            m["output"]["sha256"].as_str(),
            Some(e.sha256),
            "the committed manifest records output.sha256 {:?}, but this entry pins {:?}; \
             either the pin moved without its manifest, or the manifest is another \
             build's",
            m["output"]["sha256"].as_str(),
            e.sha256
        );
        for (field, got, want) in [
            ("name", m["name"].as_str(), MARIADB_PACKAGE_NAME),
            ("version", m["version"].as_str(), e.version),
            (
                "upstream.release_date",
                m["upstream"]["release_date"].as_str(),
                e.upstream_released_on,
            ),
            (
                "upstream.last_checked",
                m["upstream"]["last_checked"].as_str(),
                e.last_checked_on,
            ),
            (
                "upstream.signing_key_fingerprint",
                m["upstream"]["signing_key_fingerprint"].as_str(),
                SIGNING_KEY_FPR,
            ),
            (
                "upstream.sha256",
                m["upstream"]["sha256"].as_str(),
                SOURCE_SHA256,
            ),
        ] {
            assert_eq!(
                got,
                Some(want),
                "the committed manifest and this catalogue disagree about {field}: \
                 manifest {got:?}, catalogue {want:?}"
            );
        }
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
