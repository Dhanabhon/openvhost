// SPDX-License-Identifier: GPL-3.0-or-later
//! The compiled-in nginx package catalogue: which build this build of
//! OpenVHost will install, from where, and what its bytes must hash to
//! (nginx-recipe design D2, D3, D5; build-pipeline design D5).
//!
//! **Compiled in, not fetched.** The public `openvhost/manifests` repository is
//! programme slice 6, deliberately: its schema should describe packages that
//! work rather than predict them. Until then the pins live here, where they are
//! reviewed, signed off and shipped with the binary — and, critically, where
//! nothing a user types can reach them. Identical in shape to
//! [`crate::mariadb::MariadbPackage`] on purpose (build-pipeline design D5,
//! "consistency beats novelty"): a second shape would be a finding, not a
//! design. [`Availability`] is the one deliberate exception — re-declared here
//! rather than shared, the same way MariaDB's own copy is not shared with
//! MySQL's catalogue (which needs no such type at all, because Oracle
//! publishes MySQL's binaries directly).
//!
//! SECURITY: the `(url, sha256)` pair handed to the downloader comes only from
//! [`NGINX_PACKAGES`]. The public entry points take a [`PackageTarget`] —
//! never a URL, never a hash — so there is no argument a caller can pass that
//! changes which bytes are fetched or what they must hash to. That is golden
//! rule 6 ("runtime download with SHA-256 verification only") expressed as an
//! API shape rather than a convention.
//!
//! PROVENANCE — the same in kind as MariaDB's: nginx.org publishes signed
//! upstream *source*, but no macOS binary at all, so the digest below is of a
//! tarball *we* produced, on the owner's Mac, from `build/recipes/nginx.sh`.
//! Three things stand behind it, none of them assumed:
//!
//! 1. Upstream's GPG-signed `nginx-1.30.4.tar.gz.asc` was verified by the
//!    recipe against key `43387825DDB1BB97EC36BA5D007C8D7C15D87369` (nginx
//!    release signing — Roman Arutyunyan's primary key, no expiry), the
//!    fingerprint cross-checked against `keys.openpgp.org` and
//!    `keyserver.ubuntu.com` — two hosts that share no infrastructure with
//!    each other or with `nginx.org`. Verified 2026-08-06.
//! 2. PCRE2's own release archive was independently GPG-verified (key
//!    `A95536204A3BB489715231282A98E77EB6F24CA8`, cross-checked the same way)
//!    before exactly one file — `src/pcre2.h.generic` — was extracted from it,
//!    for a **header only**: the compiled, linked, shipped PCRE2 is Apple's own
//!    `/usr/lib/libpcre2-8.dylib`, never anything this recipe builds or ships.
//!    zlib needed no fetch at all — the Xcode/CLT SDK already ships `zlib.h`
//!    and a matching stub, so nginx's own "system zlib" probe succeeded
//!    unassisted.
//! 3. The finished tree passed all seven points of the artifact contract
//!    (`build/audit.sh`), run twice — once on the staged tree, once on the
//!    packed tarball — including a real HTTP GET compared byte-for-byte
//!    across a restart (nginx-recipe design D4).
//!
//! **Single-builder trust, accepted explicitly by the owner** (build-pipeline
//! design §13, decision 1 — the same acceptance MariaDB's pin rests on).
//! There is no independent reproduction of these bytes. The build manifest
//! published beside the tarball
//! (`nginx-1.30.4-macos-arm64.manifest.json`) is what makes the inputs
//! auditable, and it is mandatory rather than a nicety. **Redo all three
//! checks before changing any entry here** — a pin nobody traced back to a
//! signature certifies "the bytes someone built", not "the bytes upstream
//! released".
//!
//! # §14 watch list — the security obligation, and its only trigger
//!
//! Leaving Homebrew makes us responsible for security updates: a user's
//! `brew upgrade` no longer reaches anything we ship, and they have no other
//! route to a fix. Build-pipeline design §14 requires the watch list to live
//! next to the catalogue, and names nginx explicitly as the next package to
//! grow it — so here it is. Check these, and move
//! [`NginxPackage::last_checked_on`] when you do — a stale date is the only
//! signal this mechanism has.
//!
//! | Watch | Where | Note |
//! |---|---|---|
//! | nginx security advisories | <https://nginx.org/en/security_advisories.html> | A CVE in nginx itself is a rebuild against a newly pinned version, never a patch to this file. |
//! | OpenSSL 3.x advisories | <https://openssl-library.org/news/vulnerabilities/> | Linked **statically** into `bin/nginx` (nginx-recipe design D6), so `otool -L` will never show it and no linkage check can find it. 3.5.7 as of the pinned build — same obligation MariaDB's copy already carries. |
//! | PCRE2 | <https://github.com/PCRE2Project/pcre2/releases> | **Header only.** `src/pcre2.h.generic` is extracted and never compiled; the code that actually runs is Apple's own `/usr/lib/libpcre2-8.dylib`. A PCRE2 CVE is therefore a macOS/Xcode SDK update, not a rebuild here — unless the CVE changes the header's own declared API shape, which is worth a look before assuming it does not apply. |
//! | zlib | n/a | System library only (`/usr/lib/libz.dylib` via the SDK); nothing was fetched, so there is nothing here to watch. |
//!
//! An nginx or OpenSSL CVE is a rebuild, not a patch: re-verify upstream's
//! signature, rebuild through the same recipe, re-run the artifact contract,
//! publish, bump this file.

use openvhost_pkg::ArchiveFormat;

use crate::PackageTarget;
use crate::error::CoreError;

/// The directory name this runtime occupies in the package tree:
/// `packages/nginx/<major>/<version>/`. A single definition so the installer,
/// the ledger and (later) discovery cannot drift to different spellings.
pub const NGINX_PACKAGE_NAME: &str = "nginx";

/// The one series this build publishes. nginx has no "major.minor" series the
/// way MariaDB does, but its *minor* line carries the same meaning
/// (nginx-recipe design D2): `1.30.3` and `1.30.4` are drop-in for each
/// other, `1.28` and `1.30` are not necessarily — so `<major>` here is that
/// minor line, and the tree is `packages/nginx/1.30/1.30.4/`.
///
/// **Stable, not mainline** (nginx-recipe design D3): every version shipped
/// here is a maintenance obligation this project has signed up for, and
/// "what nginx.org recommends" is an argument aimed at operators who can
/// upgrade on their own schedule — a user of this app cannot, they get
/// whatever we pinned. Every extra line published is another tree to build,
/// verify and patch, and the §14 obligation above scales with that count, the
/// same reasoning that pins MariaDB to 11.4 LTS only.
pub const NGINX_SERIES: &str = "1.30";

/// The binary exec'd once inside the staging directory so macOS pays its
/// first-execution signature check during the install, behind progress the
/// user is already watching, instead of on their first "Start" (same
/// mechanism [`crate::mariadb::MARIADB_WARMUP_BINARY`] uses).
///
/// **`bin/nginx`, and there is nothing else it could be.** Unlike MariaDB's
/// tarball, which also ships `bin/mariadbd-safe` and `bin/mysqld_safe` —
/// wrapper scripts that genuinely start a server rather than print a version
/// — nginx's tarball ships exactly one executable under `bin/`
/// (`build/recipes/nginx.sh`'s `RECIPE_REQUIRED_LAYOUT=(bin)`, and its
/// `recipe_install` stage produces nothing else there). So there is no unsafe
/// alternative this constant could accidentally name, and no "never a safe
/// wrapper" caveat to carry. It is still worth warming: nginx's binary pays
/// the same first-execution Gatekeeper/XProtect cost any freshly extracted
/// Mach-O does, regardless of what kind of program it is. See
/// `crate::nginx::install` for what actually happens when this is warmed —
/// nginx has no `--version`, only `-v`, so the fixed `--version` argument the
/// pipeline always passes is rejected as an unrecognized option. That is
/// harmless: nginx's own option parser (`ngx_get_options`) prints "invalid
/// option" to stderr and returns before touching any config, socket, pidfile
/// or listener, so the exec still happens — which is the only thing the
/// signature check cares about — and still exits promptly with no side
/// effects.
pub const NGINX_WARMUP_BINARY: &str = "bin/nginx";

/// Whether the release that would serve a pinned artifact actually exists yet.
///
/// A state where a state belongs, rather than a comment nobody reads or a
/// `bool` that says nothing about what to do next. **Publishing is
/// owner-gated** (nginx-recipe design, scope §2, and the plan's global
/// constraints): the build pipeline produces artifacts locally, and creating
/// a GitHub Release that hosts binaries is an outward-facing act only the
/// owner may perform. Until they do, the URL below is where the bytes *will*
/// live, not where they are.
///
/// Matched exhaustively at the one place it gates an install
/// (`crate::nginx::install_nginx_package`), never through a wildcard arm: a
/// third state would have to be decided about rather than silently treated as
/// installable.
///
/// **Duplicated from [`crate::mariadb::Availability`] on purpose, not
/// shared.** Both catalogues state this explicitly: a shared `Availability`
/// type would be an abstraction over two enums that happen to look alike
/// today, and the one thing every recipe in this pipeline has learned so far
/// is that "happens to look alike" is not the same claim as "is the same
/// concept" — MySQL's own catalogue needs no such type at all, because Oracle
/// publishes its binaries directly. A second, textually-identical enum here
/// is the sanctioned shape, not a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The release exists and `url` serves the pinned bytes.
    Published,
    /// The release does not exist yet, so `url` 404s. Carries the tag a human
    /// has to create — the whole point of modelling this is that the refusal
    /// can name the next action instead of surfacing as a network fault.
    AwaitingRelease {
        /// The release tag to publish, e.g. `"nginx-1.30.4"` (build-pipeline
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
/// [`NGINX_PACKAGES`] — outside this crate's own tests nothing can mint an
/// entry pointing somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NginxPackage {
    /// The minor line, e.g. `"1.30"` — the tree level nginx-recipe design D2
    /// treats as the drop-in-compatible unit, standing in for MariaDB's
    /// `major.minor` series.
    pub major: &'static str,
    /// The exact release, e.g. `"1.30.4"`. This is the value recorded at
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

/// Every nginx build this version of OpenVHost will install.
///
/// # !!! NOT PUBLISHED YET — this entry cannot be installed from the network !!!
///
/// The tarball exists — built by `build/recipes/nginx.sh` and audited by
/// `build/audit.sh`, all seven contract checks passing on a real rebuild —
/// but **nothing has been pushed to GitHub Releases**: publishing is
/// owner-gated (build-pipeline design D5) and has not happened. The URL below
/// is the address the release *will* have under design D5's
/// one-release-per-`<name>-<version>` scheme, pinned now so that publishing
/// is a one-line change to [`Availability`] rather than an invitation to
/// invent a URL later. Today it 404s, and [`Availability::AwaitingRelease`]
/// is what stops that 404 ever reaching a user:
/// `crate::nginx::install_nginx_package` refuses before any network work,
/// naming the tag to publish.
///
/// **Cut from an observed rebuild of both OpenSSL and nginx on 2026-08-08,
/// and reproducible as a repack from the prefix that rebuild staged.** The
/// pin this replaced (`a29e7d61…`, PR #57) had neither property, and the two
/// findings that produced it are worth keeping, because a future reader of
/// this field will be asking exactly the questions they answer.
///
/// **The durable one: a repack of the drifted prefix passed the artifact
/// contract 7/7** — measured live, zero checks skipped — so swapping the pin
/// to it would have replaced an audited artifact with an unexplained one
/// while every gate stayed green. *Passing the contract is not having
/// provenance.* That is why the repack was refused and this rebuild happened
/// instead.
///
/// **What had drifted, and why nothing noticed.** `/opt/openvhost-build`'s
/// `openssl-3.5.7` carried mtime 2026-08-07 09:52:10 and its
/// `nginx-1.30.4/bin/nginx` 09:52:59 — 49 seconds later, and 09:52 local is
/// 02:52 UTC, the exact string embedded in the drifted binary. So a
/// dependency was rebuilt and its consumer relinked in one run, after
/// `a29e7d61…` was cut; `bin/nginx` was the only file among 24 that differed,
/// same size, 611 differing byte positions. Nothing could see it because
/// `build.sh` recorded only `"version": "3.5.7"`, a line two different builds
/// of 3.5.7 write identically. The manifest's `dependencies` block closes
/// that, and it is not a theory: the pre-removal prefix digested to
/// `0810760892…` and the rebuilt one to `e486946b…`.
///
/// **The old pin's provenance was stale a second, independent way.** It was
/// cut from an in-progress revision of its own recipe: its manifest has no
/// `recipe.pcre2.last_checked`, and `git blame` puts that field in `c87ec6c`
/// — PR #57 itself — authored 21:31 +07 on 2026-08-06, while the manifest
/// beside the pinned tarball was written 15:50 +07 the same day. The
/// `include_str!` tripwire below stayed green throughout, because it compares
/// date literals and a key fingerprint, not the recipe revision the bytes
/// came from. That gap is filed separately.
///
/// **To publish:**
///
/// 1. Confirm the tarball at `build/out/nginx-1.30.4-macos-arm64.tar.gz`
///    still hashes to `sha256` below. Since the pack stage stopped writing a
///    timestamp into the gzip header, re-packing the staged prefix
///    (`build/build.sh --from pack nginx 1.30.4`) must print this same hash,
///    so that is a real check rather than a formality. This module's own test
///    `the_catalogue_pins_exactly_one_nginx_build_today` pins the same digest
///    as a literal, so a mismatch there is the first signal something moved.
///
///    **That shortcut costs the manifest, so take a copy first.** A `--from
///    pack` run rewrites the sidecar step 2 publishes: it starts past
///    `configure`, so `configure_flags` comes back empty, and it never links,
///    so the dependency block comes back `"not_observed"` instead of naming
///    the OpenSSL build. It also starts past `audit`, so it exercises the
///    contract once, against the tarball only. The manifest in `build/out`
///    today is from the complete 2026-08-08 run and is the one to ship.
/// 2. Create release `nginx-1.30.4` carrying the tarball, its `.sha256`
///    sidecar and `nginx-1.30.4-macos-arm64.manifest.json`, confirm the
///    served bytes still hash to the pin, then flip `availability` to
///    [`Availability::Published`].
///
/// **`macos-x86_64` is deliberately absent** and this slice does not add it:
/// there is no signature-checked x86_64 artifact — `build/recipes/nginx.sh`'s
/// `recipe_fetch` refuses to even start a non-arm64 build (nginx-recipe
/// design §12) — and shipping an unverified pin to make a table look
/// symmetrical is exactly the failure golden rule 6 exists to prevent. An
/// Intel host gets an honest [`CoreError::NoPackageForTarget`] rather than
/// arm64 binaries.
pub const NGINX_PACKAGES: [NginxPackage; 1] = [NginxPackage {
    major: NGINX_SERIES,
    version: "1.30.4",
    target: PackageTarget::MacosArm64,
    url: "https://github.com/Dhanabhon/openvhost/releases/download/nginx-1.30.4/nginx-1.30.4-macos-arm64.tar.gz",
    sha256: "bc4c42a2618f2ac51145f7c23959421a8d019bde67e0d71946548d9cc9ac4563",
    format: ArchiveFormat::TarGz,
    availability: Availability::AwaitingRelease {
        tag: "nginx-1.30.4",
    },
    // Mirrors `RECIPE_UPSTREAM_RELEASE_DATE` / `RECIPE_LAST_CHECKED` in
    // `build/recipes/nginx.sh`, which records the same two dates in the build
    // manifest. `the_tripwire_dates_agree_with_the_recipe_that_built_the_bytes`
    // makes a drift between the two a test failure rather than a discrepancy
    // nobody reads.
    upstream_released_on: "2026-07-15",
    last_checked_on: "2026-08-06",
}];

/// The catalogue entry for `target`, or an error naming what is missing.
///
/// `target` is an `Option` so the "this host has no packages at all" case is an
/// ordinary value rather than a separate code path — and so both branches are
/// reachable from a test on any one machine.
///
/// **Takes no series argument, unlike [`crate::mysql::mysql_package_for_target`]
/// — the one deliberate departure from that function's shape, and the same
/// departure [`crate::mariadb::mariadb_package_for_target`] already takes.**
/// nginx-recipe design D3 settled this build on stable-only, one line at a
/// time, and the point of that decision is that adding a second line is a
/// decision with a cost rather than a value someone passes in. A parameter
/// whose only legal argument is [`NGINX_SERIES`] would suggest otherwise.
/// When a second line is decided on, this grows the parameter and the callers
/// have to be revisited — which is the intended friction.
///
/// This does **not** report whether the entry can be fetched today; see
/// [`NginxPackage::availability`]. Resolution and availability are separate
/// questions on purpose: a caller that only wants to display what this build
/// pins should not have to care that the release is unpublished.
pub fn nginx_package_for_target(
    target: Option<PackageTarget>,
) -> Result<&'static NginxPackage, CoreError> {
    let Some(target) = target else {
        return Err(CoreError::NoPackageForTarget {
            name: NGINX_PACKAGE_NAME,
            version: NGINX_SERIES.to_string(),
            target: "this host",
        });
    };
    NGINX_PACKAGES
        .iter()
        .find(|p| p.target == target)
        .ok_or(CoreError::NoPackageForTarget {
            name: NGINX_PACKAGE_NAME,
            version: NGINX_SERIES.to_string(),
            target: target.as_str(),
        })
}

/// The catalogue entry for the host this binary was built for.
pub fn nginx_package_for_host() -> Result<&'static NginxPackage, CoreError> {
    nginx_package_for_target(PackageTarget::host())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The recipe that produced the pinned bytes, read at compile time so the
    /// two records of the same facts cannot drift silently. Test-only: the
    /// production build has no dependency on `build/`.
    const RECIPE: &str = include_str!("../../../../../build/recipes/nginx.sh");

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
    // proven by mutation the same way `crate::mariadb::package::catalogue`'s
    // twin group was: flipping one hex digit of the pinned sha256 turns
    // `the_catalogue_pins_exactly_one_nginx_build_today` red on its own, and
    // pointing `NGINX_PACKAGE_NAME` at "mariadb" would redden this group plus
    // every install test.
    // ------------------------------------------------------------------

    #[test]
    fn the_catalogue_pins_exactly_one_nginx_build_today() {
        assert_eq!(NGINX_PACKAGES.len(), 1);
        let e = &NGINX_PACKAGES[0];
        assert_eq!(e.major, "1.30");
        assert_eq!(e.version, "1.30.4");
        assert_eq!(e.target, PackageTarget::MacosArm64);
        assert_eq!(
            e.url,
            "https://github.com/Dhanabhon/openvhost/releases/download/nginx-1.30.4/\
             nginx-1.30.4-macos-arm64.tar.gz"
        );
        assert_eq!(
            e.sha256,
            "bc4c42a2618f2ac51145f7c23959421a8d019bde67e0d71946548d9cc9ac4563"
        );
        assert_eq!(e.format, ArchiveFormat::TarGz);
    }

    #[test]
    fn every_entry_is_https_and_carries_a_well_formed_lowercase_sha256() {
        for e in &NGINX_PACKAGES {
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
        for e in &NGINX_PACKAGES {
            assert_eq!(e.major, NGINX_SERIES, "design D3 pins one stable line only");
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
        assert_eq!(NGINX_PACKAGE_NAME, "nginx");
        assert!(!NGINX_PACKAGE_NAME.contains('/'));
        assert!(!NGINX_PACKAGE_NAME.contains('.'));
    }

    /// Pins the warm-up target. Unlike MariaDB's twin of this test, there is
    /// no `-safe`-style wrapper for this assertion to rule out — nginx's
    /// tarball ships exactly one binary under `bin/` — so this is a pin on the
    /// one binary that exists, not a guard against a dangerous alternative.
    #[test]
    fn the_warm_up_binary_is_the_nginx_binary() {
        assert_eq!(NGINX_WARMUP_BINARY, "bin/nginx");
    }

    // ------------------------------------------------------------------
    // Group 2 — the §14 tripwire.
    //
    // Vacuity: `RECIPE` is a real file read at compile time and the assertions
    // are `contains` against strings built from the catalogue, so they go red
    // the moment either record moves without the other. Proven by mutation —
    // see the report accompanying this change: changing `last_checked_on` to
    // a date the recipe does not carry failed the agreement test while every
    // other test in this module stayed green.
    // ------------------------------------------------------------------

    #[test]
    fn every_entry_carries_both_dates_the_security_obligation_needs() {
        let is_iso_date = |s: &str| {
            s.len() == 10
                && s.as_bytes()[4] == b'-'
                && s.as_bytes()[7] == b'-'
                && s.bytes().filter(|b| b.is_ascii_digit()).count() == 8
        };
        for e in &NGINX_PACKAGES {
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
    /// that produced the bytes — and records of the same facts can drift
    /// silently. Reading the recipe makes that drift a test failure instead
    /// of a discrepancy nobody notices during a CVE response. Covers both
    /// tripwire dates and the signing key fingerprint (see this module's
    /// PROVENANCE note) — the fingerprint matters more than either date, and
    /// until now nothing checked it against the recipe that actually
    /// verified a signature by it.
    #[test]
    fn the_tripwire_dates_agree_with_the_recipe_that_built_the_bytes() {
        let e = &NGINX_PACKAGES[0];
        // nginx's release-signing key primary fingerprint, restated here so it
        // can be checked rather than merely stated in the PROVENANCE prose above.
        const SIGNING_KEY_FPR: &str = "43387825DDB1BB97EC36BA5D007C8D7C15D87369";
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
        ] {
            assert!(
                RECIPE.contains(&want),
                "build/recipes/nginx.sh does not carry {want:?}; the catalogue and \
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
        assert!(RECIPE.contains("nginx"), "that is not the nginx recipe");
    }

    // ------------------------------------------------------------------
    // Group 3 — the entry is honestly marked unpublished.
    //
    // Vacuity: the assertions are on `Availability`, matched exhaustively, so a
    // flip to `Published` fails them. That is intended — publishing is supposed
    // to be a reviewed change, and these tests are the review's checklist.
    // Proven by mutation: setting `availability: Availability::Published`
    // turns exactly this group red and leaves the rest of the catalogue green
    // (mirrors the mutation already proven against MariaDB's identical test).
    // ------------------------------------------------------------------

    /// Publishing is owner-gated and has not happened. When it does, this test
    /// is the checklist: create the release, re-verify the served bytes against
    /// the pin, then change this test and the entry together.
    #[test]
    fn the_pinned_release_is_marked_as_not_yet_published() {
        let e = &NGINX_PACKAGES[0];
        match e.availability {
            Availability::AwaitingRelease { tag } => {
                assert_eq!(tag, "nginx-1.30.4");
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
        let entry = nginx_package_for_target(Some(PackageTarget::MacosArm64)).unwrap();
        assert_eq!(entry.version, "1.30.4");
        assert_eq!(entry.target, PackageTarget::MacosArm64);
        assert!(entry.url.contains("arm64"));
    }

    /// The catastrophic silent bug this rules out: handing arm64 binaries to an
    /// Intel host because the lookup ignored the target. Intel must get a
    /// refusal, and it must name the target it could not serve.
    #[test]
    fn intel_gets_an_honest_refusal_rather_than_the_arm64_build() {
        let err = nginx_package_for_target(Some(PackageTarget::MacosX86_64)).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "nginx");
                assert_eq!(version, "1.30");
                assert_eq!(target, "macos-x86_64");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("macos-x86_64"), "got {err}");
        assert!(err.to_string().contains("nginx"), "got {err}");
    }

    #[test]
    fn an_unsupported_host_is_refused_and_says_so() {
        let err = nginx_package_for_target(None).unwrap_err();
        match err {
            CoreError::NoPackageForTarget {
                name,
                ref version,
                target,
            } => {
                assert_eq!(name, "nginx");
                assert_eq!(version, "1.30");
                assert_eq!(target, "this host");
            }
            ref other => panic!("wrong variant: {other:?}"),
        }
        assert!(err.to_string().contains("this host"), "got {err}");
    }

    /// On the machines this slice ships for, the host lookup and the explicit
    /// arm64 lookup must be the same answer — otherwise
    /// [`nginx_package_for_host`] could be resolving through some path the
    /// target-explicit tests never cover.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_lookup_agrees_with_the_explicit_arm64_lookup() {
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            nginx_package_for_host().unwrap(),
            nginx_package_for_target(Some(PackageTarget::MacosArm64)).unwrap()
        );
    }
}
