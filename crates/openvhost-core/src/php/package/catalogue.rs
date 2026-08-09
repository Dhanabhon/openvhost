// SPDX-License-Identifier: GPL-3.0-or-later
//! The compiled-in PHP package catalogue: which build this build of
//! OpenVHost will install, for which host and PHP major, from where, and
//! what its bytes must hash to (php-recipe design D1-D6; build-pipeline
//! design D5).
//!
//! **Compiled in, not fetched.** The public `openvhost/manifests` repository
//! is programme slice 6, deliberately: its schema should describe packages
//! that work rather than predict them. Until then the pins live here, where
//! they are reviewed, signed off and shipped with the binary — and,
//! critically, where nothing a user types can reach them.
//!
//! **Takes a major, unlike [`crate::nginx::NGINX_PACKAGES`] and
//! [`crate::mariadb::MARIADB_PACKAGES`].** Both of those pin a single stable
//! series on purpose (nginx-recipe design D3; MariaDB spec §13.3): adding a
//! second line is a decision with a cost, not a value a caller passes in.
//! PHP is the opposite case — multiple majors installed side by side is this
//! app's headline feature (`brew install php@8.1` through `php@8.5` today),
//! and `build/recipes/_php-pins.sh` already pins verified php-src sources for
//! three of them (8.3.33, 8.4.24, 8.5.9). So [`php_package_for_target`] takes
//! a [`crate::php::PhpMajor`], the same shape
//! [`crate::mysql::mysql_package_for_target`] already uses for the identical
//! reason.
//!
//! **But only one major is pinned here today.** Building and auditing a PHP
//! artifact is per-major work — its own `spc build`, its own audit run — not
//! a URL template, so `_php-pins.sh`'s three pinned SOURCES are not three
//! built ARTIFACTS. 8.4.24 is the only version this slice has a built,
//! audited tarball for, so it is the only entry in [`PHP_PACKAGES`]. A
//! catalogue entry for a version nobody has built would be a hash attached to
//! a claim nobody checked — the same failure golden rule 6 exists to prevent
//! for the missing `macos-x86_64` row below.
//! [`php_package_for_target`] refuses honestly for every other cataloged
//! major, exactly as it refuses for an unsupported target — see
//! `a_cataloged_major_with_no_pinned_package_build_is_refused` in this
//! module's tests, which is also the test proving the lookup does not ignore
//! `major`.
//!
//! SECURITY: the `(url, sha256)` pair handed to the downloader comes only
//! from [`PHP_PACKAGES`]. The public entry points take a
//! [`crate::php::PhpMajor`] and a [`PackageTarget`] — never a URL, never a
//! hash — so there is no argument a caller can pass that changes which bytes
//! are fetched or what they must hash to. That is golden rule 6 ("runtime
//! download with SHA-256 verification only") expressed as an API shape
//! rather than a convention.
//!
//! PROVENANCE — the same in kind as nginx's and MariaDB's: upstream
//! (php.net) publishes signed source, but producing a binary from it is not
//! a `./configure && make` this project drives directly. PHP is built by
//! static-php-cli ("spc"), which resolves and compiles ~35 third-party
//! sources of its own, none of them verified by spc itself (php-recipe
//! design D1). So the digest below is of a tarball *we* produced, on the
//! owner's Mac, from `build/recipes/php.sh`. Four things stand behind it,
//! none of them assumed:
//!
//! 1. Upstream's GPG-signed `php-8.4.24.tar.xz.asc` was verified by the
//!    recipe against key `9D7F99A0CB8F05C8A6958D6256A97AF7600A39A6` — php.net
//!    publishes a signing key PER RELEASE, not per major, so this is
//!    8.4.24's own key, recorded in `build/recipes/_php-pins.sh`'s
//!    `PHP_PINS_PHP_SRC` table rather than a single crate-wide constant. The
//!    fingerprint was cross-checked against `php.net/gpg-keys.php` and the
//!    `php/web-php` GitHub source (which added the 8.4 signing keys in
//!    commit `3814d0ba`, 2024-06-04) — two hosts sharing no infrastructure
//!    with each other or with the download host. Verified 2026-08-07.
//! 2. Every one of the ~35 other sources `spc build` would otherwise have
//!    fetched unverified for itself was instead fetched and SHA-256-verified
//!    by the recipe first (`build/recipes/_php-pins.sh`), and `spc build`
//!    then ran with the network denied outright (`sandbox-exec`,
//!    `(deny network*)`) — measured both directions: the build completes
//!    with the network denied, and fails outright rather than silently
//!    fetching when a pin is removed and the network is available (spc's
//!    build path has no download fallback).
//! 3. The finished tree passed all seven points of the artifact contract
//!    (`build/audit.sh`), including check 6: a real FastCGI request served
//!    through a real nginx for a real `.php`, with opcache and xdebug both
//!    loaded from the relocated tree through `-d` pairs and no `php.ini`
//!    anywhere, the response compared across a restart (php-recipe design
//!    §9, items 3 and 5) — not `php-fpm -t`, and not a version print.
//! 4. The build manifest records all 34 pinned third-party sources — and it
//!    records `resumed_from: "pack"`, which is deliberate rather than a
//!    regression to explain away. **It does not record the 8 `spc build`
//!    flags**: `--from pack` skips `recipe_configure`, the only stage in
//!    this pipeline that calls `bp_record_flags` (defined at
//!    `build/build.sh:540`), so
//!    a repack manifest's `configure_flags` is `[]` — measured on this
//!    build, not assumed. The flags are still pinned, just not here: they
//!    live in `build/recipes/php.sh`'s `_php_spc_build_args`, which is where
//!    to read them until a full rebuild repopulates this field. The tarball
//!    this hash names was **repacked** from the staged prefix
//!    `/opt/openvhost-build/php-8.4.24` by `build/build.sh php 8.4.24 --from
//!    pack`, so that the pin would name bytes the pipeline can *reproduce*
//!    (reproducible-pack design §5.3–§5.4). `--from` is recorded precisely so
//!    that an artifact assembled from partly stale state cannot look like a
//!    clean one, and this one is honest about being a repack.
//!
//!    What the repack did **not** change is the tree. `gunzip -c` of a
//!    pre-repack pack and of this one give the same raw tar,
//!    `df0dfb79c99ad02b6b0abfccdb74167f6ad8e89e08d28239f384a5405c3f63ae` —
//!    same entries, same modes, same mtimes — so the only difference from the
//!    artifact the one complete `spc build` run produced is the four-byte
//!    MTIME field gzip writes into its own header from the clock. The staged
//!    prefix is still what that single complete run installed: no stage was
//!    re-run over it, and the seven contract checks below were re-run against
//!    the repacked tarball, not carried over.
//!
//! **Single-builder trust, accepted explicitly by the owner** (build-pipeline
//! design §13, decision 1 — the same acceptance nginx's and MariaDB's pins
//! rest on). There is no independent reproduction of these bytes. The build
//! manifest published beside the tarball
//! (`php-8.4.24-macos-arm64.manifest.json`) is what makes the inputs
//! auditable, and it is mandatory rather than a nicety. **Redo all four
//! checks before changing any entry here** — a pin nobody traced back to a
//! signature certifies "the bytes someone built", not "the bytes upstream
//! released".
//!
//! # §14 watch list — the security obligation, and its only trigger
//!
//! Leaving Homebrew makes us responsible for security updates: a user's
//! `brew upgrade` no longer reaches anything we ship, and they have no other
//! route to a fix. Build-pipeline design §14 requires the watch list to live
//! next to the catalogue. Check these, and move
//! [`PhpPackage::last_checked_on`] when you do — a stale date is the only
//! signal this mechanism has.
//!
//! | Watch | Where | Note |
//! |---|---|---|
//! | PHP itself | <https://www.php.net/distributions/> | A CVE in PHP is a rebuild against a newly pinned php-src version, never a patch to this file. `build/recipes/_php-pins.sh`'s `PHP_PINS_PHP_SRC` table is where the new pin goes. |
//! | OpenSSL 3.x advisories | <https://openssl-library.org/news/vulnerabilities/> | Linked **statically** into `bin/php` and `bin/php-fpm` (php-recipe design D5), built from the `openssl` entry in `_php-pins.sh`'s `PHP_PINS_LIBS`, so `otool -L` will never show it and no linkage check can find it. 3.6.3 as of the pinned build — the same obligation nginx's and MariaDB's copies already carry. |
//! | xdebug | <https://github.com/xdebug/xdebug/releases> | The one shared extension pinned as a third-party source rather than compiled from php-src itself (`_php-pins.sh`'s `PHP_PINS_LIBS`); a CVE here means moving its own pin, independent of PHP's own version. |
//! | The other ~34 pinned libraries | `build/recipes/_php-pins.sh`'s `PHP_PINS_LIBS` table | Each is its own upstream project with its own advisories; the table is the inventory to walk, not a single link to watch. |
//!
//! A PHP or OpenSSL CVE is a rebuild, not a patch: re-verify upstream's
//! signature, rebuild through the same recipe, re-run the artifact contract,
//! publish, bump this file.

use openvhost_pkg::ArchiveFormat;

use crate::PackageTarget;
use crate::error::CoreError;
use crate::php::PhpMajor;

/// The directory name this runtime occupies in the package tree:
/// `packages/php/<major>/<version>/`. A single definition so the installer,
/// the ledger and (later) discovery cannot drift to different spellings.
pub const PHP_PACKAGE_NAME: &str = "php";

/// The binary exec'd once inside the staging directory so macOS pays its
/// first-execution signature check during the install, behind progress the
/// user is already watching, instead of on their first "Start" — the same
/// mechanism [`crate::nginx::NGINX_WARMUP_BINARY`] and
/// [`crate::mariadb::MARIADB_WARMUP_BINARY`] use.
///
/// **`bin/php-fpm`, not `bin/php`.** Both ship — `RECIPE_REQUIRED_LAYOUT` in
/// `build/recipes/php.sh` carries both `--build-cli` and `--build-fpm` — but
/// php-fpm is the binary the app actually supervises
/// (`RECIPE_SERVER_BIN="bin/php-fpm"` in that same recipe, and the one the
/// artifact contract's own checks 5/6 exercise). Warming the binary the app
/// starts is the rule every package in this fleet already follows:
/// `bin/mysqld` never `bin/mysqld_safe`, `bin/mariadbd` never a `-safe`
/// wrapper, `bin/nginx` (its only binary). Unlike those `-safe` wrappers,
/// `bin/php` is not dangerous to warm — it never listens on anything — it is
/// simply not the one this app runs, so warming it would pay the Gatekeeper
/// cost for the wrong binary and leave php-fpm's first "Start" just as slow
/// as an uninstalled package's.
///
/// **Unlike nginx's warm-up, this one does not merely survive — it
/// succeeds.** `openvhost-pkg`'s pipeline always execs the warm-up target
/// with a fixed literal `--version` (see [`crate::nginx::NGINX_WARMUP_BINARY`]'s
/// doc for why nginx's own `-v`-only parser rejects that flag and the
/// warm-up still pays off regardless). `php-fpm --version` is a real,
/// documented flag: it prints and exits 0, so this warm-up is a genuine
/// successful invocation of the binary the app starts, not merely an exec
/// Gatekeeper charges for before an "invalid option" refusal.
pub const PHP_WARMUP_BINARY: &str = "bin/php-fpm";

/// Whether the release that would serve a pinned artifact actually exists yet.
///
/// A state where a state belongs, rather than a comment nobody reads or a
/// `bool` that says nothing about what to do next. **Publishing is
/// owner-gated** (build-pipeline design D5 and the plan's global
/// constraints): the build pipeline produces artifacts locally, and creating
/// a GitHub Release that hosts binaries is an outward-facing act only the
/// owner may perform. Until they do, the URL below is where the bytes *will*
/// live, not where they are.
///
/// Matched exhaustively at the one place it gates an install
/// (`crate::php::install_php_package`), never through a wildcard arm: a
/// third state would have to be decided about rather than silently treated
/// as installable.
///
/// **Duplicated from [`crate::nginx::Availability`] and
/// [`crate::mariadb::Availability`] on purpose, not shared.** All three
/// catalogues state this explicitly: a shared `Availability` type would be an
/// abstraction over enums that happen to look alike today, and the one thing
/// every recipe in this pipeline has learned so far is that "happens to look
/// alike" is not the same claim as "is the same concept" — MySQL's own
/// catalogue needs no such type at all, because Oracle publishes its
/// binaries directly. A third, textually-identical enum here is the
/// sanctioned shape, not a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The release exists and `url` serves the pinned bytes.
    Published,
    /// The release does not exist yet, so `url` 404s. Carries the tag a human
    /// has to create — the whole point of modelling this is that the refusal
    /// can name the next action instead of surfacing as a network fault.
    AwaitingRelease {
        /// The release tag to publish, e.g. `"php-8.4.24"` (build-pipeline
        /// design D5: one release per `<name>-<version>`, carrying the
        /// tarball, its `.sha256` and the build manifest).
        tag: &'static str,
    },
}

/// One pinned build: everything the install pipeline needs, and nothing it does
/// not.
///
/// Every field is a `&'static str` baked into the binary. There is no
/// constructor, and the type is only ever obtained by looking one up from
/// [`PHP_PACKAGES`] — outside this crate's own tests nothing can mint an
/// entry pointing somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhpPackage {
    /// The `major.minor` version, e.g. `"8.4"` — the value
    /// [`crate::php::PhpMajor::as_str`] returns, and the tree level
    /// side-by-side PHP installs are keyed on (this app's headline feature).
    pub major: &'static str,
    /// The exact release, e.g. `"8.4.24"`. This is the value recorded at
    /// install time (MySQL-from-tarball design D4, reused unchanged); it is
    /// never recovered by probing.
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

/// Every PHP build this version of OpenVHost will install.
///
/// # !!! NOT PUBLISHED YET — this entry cannot be installed from the network !!!
///
/// The tarball exists — built by `build/recipes/php.sh` and audited by
/// `build/audit.sh`, all seven contract checks passing on a real rebuild —
/// but **nothing has been pushed to GitHub Releases**: publishing is
/// owner-gated (build-pipeline design D5) and has not happened. The URL below
/// is the address the release *will* have under design D5's
/// one-release-per-`<name>-<version>` scheme, pinned now so that publishing
/// is a one-line change to [`Availability`] rather than an invitation to
/// invent a URL later. Today it 404s, and [`Availability::AwaitingRelease`]
/// is what stops that 404 ever reaching a user:
/// `crate::php::install_php_package` refuses before any network work, naming
/// the tag to publish.
///
/// **This pin's manifest predates the `dependencies` block** that
/// `build/build.sh` now writes, and it is not being backfilled: a `--from
/// pack` run digests whatever prefix is on disk at manifest time, so a late
/// regeneration would record today's state as though this artifact had been
/// built against it. The `sha256` would come back identical either way — a
/// sidecar cannot change a tarball — so that check would pass while the
/// manifest gained a confident, wrong claim. The driver now refuses to make
/// it, recording `"not_observed"` instead. This entry gains a real dependency
/// record when it is next built from source.
///
/// **What that record would and would not cover is worth knowing before
/// anyone reads its absence as a gap in linkage.** `build/recipes/php.sh`
/// declares exactly one `RECIPE_DEPENDS` entry, `nginx`, and it is not a build
/// input: nothing links it and no byte of it reaches this tarball — it is
/// contract check 6's FastCGI client. PHP's OpenSSL is the one `spc` compiles
/// inside its own closure from `PHP_PINS_LIBS` (see the recipe's D5 note on
/// why a borrowed prefix will not do there), pinned by digest in
/// `_php-pins.sh`. So unlike MariaDB, nothing about *this* artifact's
/// static linkage rides on the missing block; what is missing is a record of
/// which nginx build proved it serves.
///
/// **To publish:**
///
/// 1. Confirm the tarball at `build/out/php-8.4.24-macos-arm64.tar.gz` still
///    hashes to `sha256` below — this module's own test
///    `the_catalogue_pins_exactly_one_php_build_today` pins the same digest
///    as a literal, so a mismatch there is the first signal something moved.
///    Do not reach for `--from pack` to produce that confirmation: it would
///    overwrite the manifest beside the tarball, dropping `configure_flags`
///    (it starts past `configure`) as well as the dependency record.
/// 2. Create release `php-8.4.24` carrying the tarball, its `.sha256`
///    sidecar and `php-8.4.24-macos-arm64.manifest.json`, confirm the served
///    bytes still hash to the pin, then flip `availability` to
///    [`Availability::Published`].
///
/// **Only 8.4 is pinned, even though [`crate::php::CATALOGUE`] offers 8.1
/// through 8.5 for Homebrew install.** See this module's header: a package
/// build is per-major work, and 8.4.24 is the only one with a built, audited
/// artifact today.
///
/// **`macos-x86_64` is deliberately absent** and this slice does not add it:
/// there is no signature-checked x86_64 artifact — `build/recipes/php.sh`
/// builds only what `spc` and this recipe's pinned tools were proven against
/// on this Apple Silicon builder — and shipping an unverified pin to make a
/// table look symmetrical is exactly the failure golden rule 6 exists to
/// prevent. An Intel host gets an honest [`CoreError::NoPackageForTarget`]
/// rather than arm64 binaries.
pub const PHP_PACKAGES: [PhpPackage; 1] = [PhpPackage {
    major: "8.4",
    version: "8.4.24",
    target: PackageTarget::MacosArm64,
    url: "https://github.com/Dhanabhon/openvhost/releases/download/php-8.4.24/php-8.4.24-macos-arm64.tar.gz",
    sha256: "c79b18c372f3f31f91bdefb79da08d81ffcc23e5f894f0a2b40060ffa6bcc2bb",
    format: ArchiveFormat::TarGz,
    availability: Availability::AwaitingRelease { tag: "php-8.4.24" },
    // Mirrors `_php-pins.sh`'s `PHP_PINS_UPSTREAM_RELEASE_DATE` /
    // `PHP_PINS_LAST_CHECKED`, which record the same two dates for the
    // php-src release this artifact was built from.
    // `the_pinned_version_agrees_with_the_php_src_row_the_pin_set_carries`
    // makes a drift between the two a test failure rather than a discrepancy
    // nobody reads.
    upstream_released_on: "2026-07-30",
    last_checked_on: "2026-08-07",
}];

/// The catalogue entry for `major` on `target`, or an error naming what is
/// missing.
///
/// `target` is an `Option` so the "this host has no packages at all" case is
/// an ordinary value rather than a separate code path — and so both branches
/// are reachable from a test on any one machine.
///
/// **Takes a `major`, the same shape
/// [`crate::mysql::mysql_package_for_target`] uses and the one deliberate
/// departure from [`crate::nginx::nginx_package_for_target`]'s and
/// [`crate::mariadb::mariadb_package_for_target`]'s series-only shape** — see
/// this module's header for why PHP is the side-by-side-majors case rather
/// than the single-stable-line case.
///
/// This does **not** report whether the entry can be fetched today; see
/// [`PhpPackage::availability`]. Resolution and availability are separate
/// questions on purpose: a caller that only wants to display what this build
/// pins should not have to care that the release is unpublished.
pub fn php_package_for_target(
    major: &PhpMajor,
    target: Option<PackageTarget>,
) -> Result<&'static PhpPackage, CoreError> {
    let Some(target) = target else {
        return Err(CoreError::NoPackageForTarget {
            name: PHP_PACKAGE_NAME,
            version: major.as_str().to_string(),
            target: "this host",
        });
    };
    PHP_PACKAGES
        .iter()
        .find(|p| p.major == major.as_str() && p.target == target)
        .ok_or(CoreError::NoPackageForTarget {
            name: PHP_PACKAGE_NAME,
            version: major.as_str().to_string(),
            target: target.as_str(),
        })
}

/// The catalogue entry for `major` on the host this binary was built for.
pub fn php_package_for_host(major: &PhpMajor) -> Result<&'static PhpPackage, CoreError> {
    php_package_for_target(major, PackageTarget::host())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The recipe's pin SET for php-src, read at compile time so the two
    /// records of "which php-src release we ship" cannot drift silently.
    /// Test-only: the production build has no dependency on `build/`.
    ///
    /// Unlike [`crate::nginx`]'s and [`crate::mariadb`]'s twin of this
    /// constant, this reads `_php-pins.sh`, not `php.sh`: php.sh has no
    /// literal `RECIPE_UPSTREAM_RELEASE_DATE="..."` of its own to `contains`
    /// against — it assigns `RECIPE_UPSTREAM_RELEASE_DATE="$PHP_PINS_UPSTREAM_RELEASE_DATE"`,
    /// resolving php-src's provenance from this file's `PHP_PINS_PHP_SRC`
    /// table instead (recipe D1's third paragraph), because php.sh pins
    /// THREE php-src releases, not one.
    const PHP_PINS: &str = include_str!("../../../../../build/recipes/_php-pins.sh");

    /// The build manifest the driver wrote beside the tarball this entry pins,
    /// committed under `build/manifests/` and read at compile time. The pin set
    /// above says what a build *would* use; this says what one *did*, and it is
    /// the only thing in the repository that ties `sha256` below to an account
    /// of how those bytes were produced. Test-only, like `PHP_PINS`.
    ///
    /// This one is a repack (`resumed_from: "pack"`, `configure_flags: []`, no
    /// `dependencies` block) — how PR #67 re-cut this pin. Its consequence is
    /// recorded rather than papered over: PHP's `spc build` flags reach **no**
    /// manifest at all and live only in `php.sh`'s `_php_spc_build_args`, so
    /// this file cannot be asked about them. See `build/manifests/README.md`.
    const MANIFEST: &str =
        include_str!("../../../../../build/manifests/php-8.4.24-macos-arm64.manifest.json");

    /// php-src's pinned source digest and upstream's release-signing key
    /// fingerprint, restated here so an edit to EITHER side breaks a test. At
    /// module scope because both the pin-set tripwire and the manifest
    /// agreement test check them: two literals of one fact is the drift this
    /// group exists to catch.
    const PHP_SRC_SHA256: &str = "e127be09a8506f4327c5cfa78a614b00d210714484ec215ce0011b4a03c00731";
    const PHP_SRC_SIGNING_KEY_FPR: &str = "9D7F99A0CB8F05C8A6958D6256A97AF7600A39A6";

    // ------------------------------------------------------------------
    // Group 1 — the pinned entry, byte for byte.
    //
    // These assertions exist so that changing the pin cannot be a quiet
    // one-character edit: the hash and URL are repeated here as literals, and
    // moving either without redoing the provenance work recorded at the top of
    // this module breaks the build.
    //
    // Vacuity: every assertion here is an equality against a literal that
    // appears nowhere else in the test, so any edit to the entry fails it —
    // proven by mutation the same way nginx's and MariaDB's twin groups were:
    // flipping one hex digit of the pinned sha256 turns
    // `the_catalogue_pins_exactly_one_php_build_today` red on its own, and
    // pointing `PHP_PACKAGE_NAME` at "mysql" would redden this group plus
    // every install test.
    // ------------------------------------------------------------------

    #[test]
    fn the_catalogue_pins_exactly_one_php_build_today() {
        assert_eq!(PHP_PACKAGES.len(), 1);
        let e = &PHP_PACKAGES[0];
        assert_eq!(e.major, "8.4");
        assert_eq!(e.version, "8.4.24");
        assert_eq!(e.target, PackageTarget::MacosArm64);
        assert_eq!(
            e.url,
            "https://github.com/Dhanabhon/openvhost/releases/download/php-8.4.24/\
             php-8.4.24-macos-arm64.tar.gz"
        );
        assert_eq!(
            e.sha256,
            "c79b18c372f3f31f91bdefb79da08d81ffcc23e5f894f0a2b40060ffa6bcc2bb"
        );
        assert_eq!(e.format, ArchiveFormat::TarGz);
    }

    #[test]
    fn every_entry_is_https_and_carries_a_well_formed_lowercase_sha256() {
        for e in &PHP_PACKAGES {
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
        for e in &PHP_PACKAGES {
            assert!(
                e.version == e.major || e.version.starts_with(&format!("{}.", e.major)),
                "{} is filed under major {}",
                e.version,
                e.major
            );
        }
    }

    #[test]
    fn the_package_name_is_a_single_safe_path_component() {
        assert_eq!(PHP_PACKAGE_NAME, "php");
        assert!(!PHP_PACKAGE_NAME.contains('/'));
        assert!(!PHP_PACKAGE_NAME.contains('.'));
    }

    /// Pins the warm-up target, and that it is never the CLI binary the
    /// tarball also ships. See [`PHP_WARMUP_BINARY`]'s doc for why `bin/php`
    /// is not dangerous the way a `-safe` wrapper would be — merely wrong.
    #[test]
    fn the_warm_up_binary_is_php_fpm_and_never_the_cli_binary() {
        assert_eq!(PHP_WARMUP_BINARY, "bin/php-fpm");
        assert_ne!(PHP_WARMUP_BINARY, "bin/php");
    }

    // ------------------------------------------------------------------
    // Group 2 — the §14 tripwire, tied to the PIN SET rather than the recipe.
    //
    // Vacuity: `PHP_PINS` is a real file read at compile time and the
    // assertions are `contains` against strings built from the catalogue, so
    // they go red the moment either record moves without the other.
    //
    // Proven by mutation (2026-08-07): changing this entry's
    // `last_checked_on` to `"2026-08-08"` — a plausible day-off typo — turned
    // `the_pinned_version_agrees_with_the_php_src_row_the_pin_set_carries`
    // red on its own with every other test in this module staying green;
    // reverting restored the green.
    // ------------------------------------------------------------------

    #[test]
    fn every_entry_carries_both_dates_the_security_obligation_needs() {
        let is_iso_date = |s: &str| {
            s.len() == 10
                && s.as_bytes()[4] == b'-'
                && s.as_bytes()[7] == b'-'
                && s.bytes().filter(|b| b.is_ascii_digit()).count() == 8
        };
        for e in &PHP_PACKAGES {
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

    /// Ties this catalogue's pinned `version` to the exact row
    /// `build/recipes/_php-pins.sh`'s `PHP_PINS_PHP_SRC` table carries for
    /// it — version, source digest and signing-key fingerprint together, as
    /// one substring — plus the two tripwire dates. Two records of the same
    /// facts drift silently unless something reads them both; this is that
    /// something.
    #[test]
    fn the_pinned_version_agrees_with_the_php_src_row_the_pin_set_carries() {
        let e = &PHP_PACKAGES[0];
        let want_row = format!(
            "\"{} {PHP_SRC_SHA256} {PHP_SRC_SIGNING_KEY_FPR} ",
            e.version
        );
        assert!(
            PHP_PINS.contains(&want_row),
            "_php-pins.sh's PHP_PINS_PHP_SRC has no row starting {want_row:?}; the \
             catalogue pins a php-src version, digest or signing key the pin set does \
             not carry"
        );
        for (field, want) in [
            (
                "PHP_PINS_UPSTREAM_RELEASE_DATE",
                format!(
                    "PHP_PINS_UPSTREAM_RELEASE_DATE=\"{}\"",
                    e.upstream_released_on
                ),
            ),
            (
                "PHP_PINS_LAST_CHECKED",
                format!("PHP_PINS_LAST_CHECKED=\"{}\"", e.last_checked_on),
            ),
        ] {
            assert!(
                PHP_PINS.contains(&want),
                "_php-pins.sh does not carry {want:?}; the catalogue and the pin set \
                 disagree about {field}"
            );
        }
    }

    /// Non-vacuity twin for the test above: it must be reading the real
    /// pin-set file, not an empty string that makes `contains` trivially
    /// satisfiable.
    #[test]
    fn the_pin_set_the_dates_are_checked_against_is_the_real_file() {
        assert!(
            PHP_PINS.len() > 1000,
            "pin set read as {} bytes",
            PHP_PINS.len()
        );
        assert!(!PHP_PINS.contains("PHP_PINS_UPSTREAM_RELEASE_DATE=\"1970-01-01\""));
        assert!(
            PHP_PINS.contains("PHP_PINS_PHP_SRC"),
            "that is not the PHP pin set"
        );
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
        let e = &PHP_PACKAGES[0];
        let m: serde_json::Value = serde_json::from_str(MANIFEST)
            .expect("build/manifests/php-8.4.24-macos-arm64.manifest.json is not valid JSON");
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
            ("name", m["name"].as_str(), PHP_PACKAGE_NAME),
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
                PHP_SRC_SIGNING_KEY_FPR,
            ),
            (
                "upstream.sha256",
                m["upstream"]["sha256"].as_str(),
                PHP_SRC_SHA256,
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
    // turns exactly this group red and leaves the rest of the catalogue green
    // (mirrors the mutation already proven against nginx's and MariaDB's
    // identical test).
    // ------------------------------------------------------------------

    /// Publishing is owner-gated and has not happened. When it does, this test
    /// is the checklist: create the release, re-verify the served bytes against
    /// the pin, then change this test and the entry together.
    #[test]
    fn the_pinned_release_is_marked_as_not_yet_published() {
        let e = &PHP_PACKAGES[0];
        match e.availability {
            Availability::AwaitingRelease { tag } => {
                assert_eq!(tag, "php-8.4.24");
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
    // Group 4 — target AND major selection.
    //
    // Vacuity: each case asserts a distinct outcome for a distinct input, and
    // the refusals assert the reason text, not merely `is_err()`.
    //
    // `a_cataloged_major_with_no_pinned_package_build_is_refused` is the one
    // that matters most for a ONE-ROW table: "returns the first entry when
    // asked for a major it does not carry" is precisely the bug a table with
    // one row cannot catch by asking for the row's OWN major (a mutated
    // lookup that ignores `major` entirely still answers correctly when the
    // request happens to match the only row). Asking for a DIFFERENT,
    // still-cataloged major is what discriminates: the sole entry is filed
    // under "8.4", so a request for "8.3" must be refused, and a lookup that
    // ignores `major` would instead hand back the "8.4" entry.
    //
    // Proven by mutation (2026-08-07): changing the filter in
    // `php_package_for_target` from `p.major == major.as_str() && p.target
    // == target` to `p.target == target` (major ignored) reddened exactly
    // `a_cataloged_major_with_no_pinned_package_build_is_refused` — it got
    // `Ok(PhpPackage { major: "8.4", version: "8.4.24", .. })` where it
    // expected `Err(NoPackageForTarget { version: "8.3", .. })`. Every other
    // test in this module, including
    // `apple_silicon_resolves_to_the_pinned_arm64_build` (which asks for the
    // matching major "8.4"), stayed green — exactly the "cannot catch it by
    // asking for the one row's own major" blind spot this test exists to
    // close. Reverting the filter restored the green. No second catalogue
    // row was needed.
    // ------------------------------------------------------------------

    #[test]
    fn apple_silicon_resolves_to_the_pinned_arm64_build() {
        let major = PhpMajor::parse("8.4").unwrap();
        let entry = php_package_for_target(&major, Some(PackageTarget::MacosArm64)).unwrap();
        assert_eq!(entry.version, "8.4.24");
        assert_eq!(entry.target, PackageTarget::MacosArm64);
        assert!(entry.url.contains("arm64"));
    }

    /// The discriminating case — see the group comment above.
    #[test]
    fn a_cataloged_major_with_no_pinned_package_build_is_refused() {
        let major = PhpMajor::parse("8.3").unwrap();
        assert!(
            major.is_cataloged(),
            "8.3 must still be a brew-offered major for this test to mean anything"
        );
        let err = php_package_for_target(&major, Some(PackageTarget::MacosArm64)).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "php");
                assert_eq!(version, "8.3");
                assert_eq!(target, "macos-arm64");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("8.3"), "got {err}");
    }

    /// The catastrophic silent bug this rules out: handing arm64 binaries to an
    /// Intel host because the lookup ignored the target. Intel must get a
    /// refusal, and it must name the target it could not serve.
    #[test]
    fn intel_gets_an_honest_refusal_rather_than_the_arm64_build() {
        let major = PhpMajor::parse("8.4").unwrap();
        let err = php_package_for_target(&major, Some(PackageTarget::MacosX86_64)).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "php");
                assert_eq!(version, "8.4");
                assert_eq!(target, "macos-x86_64");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("macos-x86_64"), "got {err}");
        assert!(err.to_string().contains("php"), "got {err}");
    }

    #[test]
    fn an_unsupported_host_is_refused_and_says_so() {
        let major = PhpMajor::parse("8.4").unwrap();
        let err = php_package_for_target(&major, None).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "php");
                assert_eq!(version, "8.4");
                assert_eq!(target, "this host");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("this host"), "got {err}");
    }

    /// On the machines this slice ships for, the host lookup and the explicit
    /// arm64 lookup must be the same answer — otherwise [`php_package_for_host`]
    /// could be resolving through some path the target-explicit tests never
    /// cover.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_lookup_agrees_with_the_explicit_arm64_lookup() {
        let major = PhpMajor::parse("8.4").unwrap();
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            php_package_for_host(&major).unwrap(),
            php_package_for_target(&major, Some(PackageTarget::MacosArm64)).unwrap()
        );
    }
}
