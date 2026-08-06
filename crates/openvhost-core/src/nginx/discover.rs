// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the nginx this app should run — OpenVHost's own package tree first,
//! then Homebrew. See
//! docs/superpowers/specs/2026-08-06-p2-nginx-discovery-design.md (D1-D5).
//!
//! Mirrors [`crate::mysql::discover`]'s two-source model rather than
//! [`crate::mariadb::discover`]'s single-source one: nginx, like MySQL and
//! unlike MariaDB, still has a live Homebrew fallback during the migration
//! (design D2) — the owner may be running a brew-installed nginx today, and
//! this walk must not strand them the moment our own tree can serve it. Every
//! runtime records [`NginxRuntimeSource`] for the identical reason
//! [`crate::mysql::MysqlRuntimeSource`] does: "which nginx am I actually
//! running" needs an honest answer from discovery itself, not a guess at the
//! call site.
//!
//! **The Homebrew half here is INDEPENDENT of
//! [`crate::platform::macos::demo_stack::find_brew_binaries`].** That prober
//! requires BOTH nginx and php-fpm in one prefix or returns nothing (design
//! table, row 2) — a coupling that has no purpose for nginx's OWN discovery
//! (php-fpm is found by its own, separate walk) and only ever *degrades* it:
//! a machine with nginx but no unversioned `php` formula would otherwise see
//! nginx go missing too. [`discover_nginx`] therefore probes nginx alone.
//!
//! Never resolves anything through `PATH` — same rule as
//! [`crate::php::discover`] and [`crate::mysql::discover`], for the same
//! reason (a ServBay install shadows binaries there).
//!
//! Spawns nothing and probes no version: a packaged nginx's version comes for
//! free from the directory name (design D1), and a Homebrew nginx's version is
//! answered by the existing [`openvhost_conf::probe_nginx_version`] wherever a
//! caller actually needs it — never by this module, which only ever reads
//! paths.

use std::path::{Path, PathBuf};

use openvhost_pkg::PackagesRoot;

use super::{NGINX_PACKAGE_NAME, NGINX_SERIES};

/// The binary this app drives directly. nginx's tarball — ours and
/// Homebrew's alike — ships exactly one executable that matters here; unlike
/// MySQL/MariaDB there is no second or third binary whose absence would make
/// an otherwise-complete tree unusable.
const NGINX_BIN_REL: &str = "bin/nginx";

/// Where a discovered nginx binary came from.
///
/// A field on [`NginxRuntime`] rather than something inferred from a path at
/// the call site — the mirror of [`crate::mysql::MysqlRuntimeSource`], for the
/// identical reason: two install sources coexist by design during the
/// migration (design D2), so "which nginx am I actually running" is a
/// question a user will ask, and it is only answerable honestly if discovery
/// records the answer at the moment it walks the directory.
///
/// Matched **exhaustively** everywhere — never through a wildcard arm — so a
/// third source (a user-registered runtime, a future Windows package) breaks
/// compilation at every site that has to decide about it instead of silently
/// rendering as one of these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NginxRuntimeSource {
    /// OpenVHost's own package tree: `packages/nginx/<series>/<version>/`,
    /// fetched from the pinned upstream tarball and SHA-256 verified before
    /// extraction.
    ///
    /// The exact version comes for free (design D1): we asked the catalogue
    /// for it, and the tree records it as a directory name, so nothing has to
    /// execute `bin/nginx` to find out — nginx has no `--version`, only `-v`,
    /// and probing it is the exact class of cost this design point exists to
    /// avoid (the same measurement that produced audit finding F1 for
    /// `mysqld --version`).
    Packaged {
        /// The exact upstream release, e.g. `"1.30.4"` — the version
        /// directory this series' `current` link selects.
        version: String,
    },
    /// A Homebrew install this app did not put there, found under a
    /// `brew --prefix`. Retired once slice 7 of the off-Homebrew programme
    /// removes the Homebrew fallback entirely — not here.
    Homebrew,
}

impl NginxRuntimeSource {
    /// The stable, machine-facing spelling. ONE definition, so a log field
    /// and a future DTO tag cannot drift into two different words for the
    /// same fact.
    pub fn as_str(&self) -> &'static str {
        match self {
            NginxRuntimeSource::Packaged { .. } => "packaged",
            NginxRuntimeSource::Homebrew => "homebrew",
        }
    }

    /// The exact version, when the source knows it.
    ///
    /// `None` for Homebrew, and deliberately so: nginx has no `--version`
    /// flag to probe cheaply (only `-v`, which still means executing the
    /// binary), and reporting the packaged series as though it were the
    /// Homebrew install's exact version would be a lie no caller could
    /// detect. A caller that genuinely needs the Homebrew version calls
    /// [`openvhost_conf::probe_nginx_version`] itself.
    pub fn version(&self) -> Option<&str> {
        match self {
            NginxRuntimeSource::Packaged { version } => Some(version),
            NginxRuntimeSource::Homebrew => None,
        }
    }
}

/// One discovered nginx installation: the binary this app spawns and
/// validates against directly, and where it came from.
///
/// **The path is always concrete** (design D5, mirroring
/// [`crate::mariadb::packaged_mariadb_runtime`]'s own doc comment). For a
/// packaged runtime it names `packages/nginx/<series>/<version>/bin/nginx`,
/// never `packages/nginx/<series>/current/bin/nginx`: a supervised child is
/// spawned from whatever is recorded here, and spawning *through* the link
/// would mean a later `current` swap silently changed which binary a restart
/// brings up, with the running process and the one the UI describes diverging
/// and nothing in between to notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NginxRuntime {
    pub bin: PathBuf,
    /// Which install produced [`Self::bin`] — see [`NginxRuntimeSource`].
    pub source: NginxRuntimeSource,
}

/// The runtime rooted at `dir`, when `bin/nginx` is really there.
///
/// The single place an [`NginxRuntime`] is built, so both sources go through
/// the identical existence check.
fn runtime_in(dir: &Path, source: NginxRuntimeSource) -> Option<NginxRuntime> {
    let bin = dir.join(NGINX_BIN_REL);
    if !bin.is_file() {
        return None;
    }
    Some(NginxRuntime { bin, source })
}

/// The packaged runtime this series' `current` link selects, or `None` when
/// there is no usable packaged install.
///
/// Copies [`crate::mariadb::packaged_mariadb_runtime`]'s discipline exactly
/// rather than re-deriving it: `current` is resolved through
/// [`PackagesRoot`]'s own facade (never `major_dir.join("current")` spelled by
/// hand — the installer swings this link through that same facade, and a
/// second spelling here is how the writer and the reader end up naming
/// different files), the link's target is validated by
/// [`crate::mysql::current_version`] (a single plain directory-name component,
/// containing neither `..` nor an absolute path — see that function's own
/// doc comment for the security reasoning), and the resolved version
/// directory is checked to be a DIRECT CHILD of the series directory before
/// its binary is ever handed out. That last check is belt-and-braces over
/// `current_version`'s own rule, stated structurally so it keeps holding
/// whatever a future `join` does with an unexpected target shape.
pub fn packaged_nginx_runtime(root: &PackagesRoot) -> Option<NginxRuntime> {
    let series_dir = root.major_dir(NGINX_PACKAGE_NAME, NGINX_SERIES);
    let version =
        crate::mysql::current_version(&root.current_link(NGINX_PACKAGE_NAME, NGINX_SERIES))?;
    let dir = root.package_dir(NGINX_PACKAGE_NAME, NGINX_SERIES, &version);
    if dir.parent() != Some(series_dir.as_path()) {
        return None;
    }
    runtime_in(&dir, NginxRuntimeSource::Packaged { version })
}

/// The Homebrew nginx, if any — independent of
/// [`crate::platform::macos::demo_stack::find_brew_binaries`] (see this
/// module's header for why that prober's php-fpm requirement must not gate
/// nginx's own discovery).
///
/// First prefix in `prefixes` holding `opt/nginx/bin/nginx` wins, matching
/// `find_brew_binaries_in`'s own layout convention and the "earlier prefix
/// wins" rule every other Homebrew walk in this crate follows (Apple Silicon
/// before Intel, so a native binary is preferred over a Rosetta one).
///
/// No version probe: unlike MySQL's Homebrew walk, nginx has no per-major
/// catalogue to merge against, so nothing here needs to know which exact
/// version a brew prefix holds — [`discover_nginx`] prefers the packaged
/// runtime outright, with no per-version comparison to make. Private: the
/// Homebrew walk is reachable only through [`discover_nginx`], mirroring
/// [`crate::mysql::discover`]'s own `discover_brew`.
fn brew_nginx_runtime(prefixes: &[&Path]) -> Option<NginxRuntime> {
    prefixes
        .iter()
        .find_map(|prefix| runtime_in(&prefix.join("opt/nginx"), NginxRuntimeSource::Homebrew))
}

/// The nginx this app should run: OpenVHost's own package tree first, then
/// Homebrew, then `None` (design D2/D3).
///
/// `None` is not a failure — it is the honest state of a machine with neither
/// installed, and every caller that used to invent a path in that case
/// (`fallback_brew`) now has to decide what an absent nginx means at its own
/// site instead of being handed a lie.
///
/// `packages` is minted from a resolved home ([`PackagesRoot::from_home`]),
/// never from user input. `prefixes` is expected in [`crate::BREW_PREFIXES`]
/// order.
pub fn discover_nginx(packages: &PackagesRoot, prefixes: &[&Path]) -> Option<NginxRuntime> {
    packaged_nginx_runtime(packages).or_else(|| brew_nginx_runtime(prefixes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A packages root on a home that does not exist — for tests that are
    /// only about the Homebrew walk. A missing tree must read as "no
    /// packaged nginx", never as an error.
    fn no_packages() -> PackagesRoot {
        PackagesRoot::from_home(Path::new("/nonexistent/openvhost-home"))
    }

    /// Lay down `packages/nginx/1.30/<version>/bin/nginx` with the binary's
    /// body set to `body`, so a test can tell one version's binary from
    /// another's by CONTENT rather than by the path it was reached through —
    /// the whole point of the `current`-swap proof below.
    fn install_fake_package(root: &PackagesRoot, version: &str, body: &str) {
        let bin = root
            .package_dir(NGINX_PACKAGE_NAME, NGINX_SERIES, version)
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("nginx"), body.as_bytes()).unwrap();
    }

    /// Point (or re-point) `packages/nginx/1.30/current` at `target`, exactly
    /// as `openvhost-pkg` does: a RELATIVE symlink whose target is the bare
    /// version string.
    #[cfg(unix)]
    fn point_current(root: &PackagesRoot, target: &str) {
        let link = root.current_link(NGINX_PACKAGE_NAME, NGINX_SERIES);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(target), &link).unwrap();
    }

    /// A home with `packages/nginx/1.30/1.30.4/` installed and selected.
    #[cfg(unix)]
    fn packaged_1_30_4() -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "1.30.4", "1.30.4 server\n");
        point_current(&root, "1.30.4");
        (home, root)
    }

    /// A real brew layout: `opt/nginx/bin/nginx`, matching
    /// `find_brew_binaries_in`'s own convention.
    fn brew_prefix(body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("opt/nginx/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("nginx"), body.as_bytes()).unwrap();
        dir
    }

    // ------------------------------------------------------------------
    // Group 1 — absence is honest, on both sources at once.
    // ------------------------------------------------------------------

    #[test]
    fn neither_source_present_is_none_not_an_error() {
        assert!(discover_nginx(&no_packages(), &[]).is_none());
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        assert!(
            discover_nginx(
                &no_packages(),
                &[Path::new("/nonexistent/openvhost-prefix")]
            )
            .is_none()
        );
    }

    #[test]
    fn a_binary_missing_from_an_otherwise_real_prefix_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("opt/nginx/bin")).unwrap();
        // The directory exists, but bin/nginx itself does not.
        assert!(discover_nginx(&no_packages(), &[dir.path()]).is_none());
    }

    // ------------------------------------------------------------------
    // Group 2 — packaged first, Homebrew second (design D2), and each
    // records its own source.
    //
    // VACUITY, measured by mutation: replacing `discover_nginx`'s body with
    // `brew_nginx_runtime(prefixes)` alone (packaged never consulted) fails
    // `a_packaged_runtime_is_found_through_the_current_link` and
    // `a_packaged_runtime_beats_a_homebrew_one`; replacing it with
    // `packaged_nginx_runtime(packages)` alone (Homebrew never consulted)
    // fails `a_homebrew_only_nginx_is_still_found`.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_is_found_through_the_current_link() {
        let (_home, root) = packaged_1_30_4();
        let found = discover_nginx(&root, &[]).expect("a packaged nginx is installed");
        assert_eq!(
            found.source,
            NginxRuntimeSource::Packaged {
                version: "1.30.4".to_string()
            }
        );
        assert!(found.bin.ends_with("packages/nginx/1.30/1.30.4/bin/nginx"));
    }

    #[cfg(unix)]
    #[test]
    fn a_packaged_runtime_beats_a_homebrew_one() {
        let (_home, root) = packaged_1_30_4();
        let brew = brew_prefix("brew nginx\n");

        let found = discover_nginx(&root, &[brew.path()]).expect("a runtime is found");
        assert!(
            found.bin.starts_with(root.as_path()),
            "the packaged runtime must win: {:?}",
            found.bin
        );
        assert_eq!(
            found.source,
            NginxRuntimeSource::Packaged {
                version: "1.30.4".to_string()
            }
        );
    }

    #[test]
    fn a_homebrew_only_nginx_is_still_found() {
        // D2's whole point: the owner may be running a brew nginx right now,
        // and adopting our own tree must not strand them.
        let brew = brew_prefix("brew nginx\n");
        let found = discover_nginx(&no_packages(), &[brew.path()]).expect("brew nginx is there");
        assert_eq!(found.source, NginxRuntimeSource::Homebrew);
        assert_eq!(found.source.version(), None);
        assert!(found.bin.starts_with(brew.path()));
    }

    #[test]
    fn an_earlier_prefix_wins_over_a_later_one() {
        let a = brew_prefix("a\n");
        let b = brew_prefix("b\n");
        let found = discover_nginx(&no_packages(), &[a.path(), b.path()]).unwrap();
        assert!(found.bin.starts_with(a.path()));
    }

    #[test]
    fn the_two_sources_have_distinct_stable_spellings() {
        // Asserted PAIRWISE, not for non-emptiness: two sources that render
        // identically are worse than no label at all, because a future UI
        // would look like it answered.
        let packaged = NginxRuntimeSource::Packaged {
            version: "1.30.4".to_string(),
        };
        assert_eq!(packaged.as_str(), "packaged");
        assert_eq!(NginxRuntimeSource::Homebrew.as_str(), "homebrew");
        assert_ne!(packaged.as_str(), NginxRuntimeSource::Homebrew.as_str());
        assert_ne!(packaged, NginxRuntimeSource::Homebrew);
    }

    // ------------------------------------------------------------------
    // Group 3 — design D5: a concrete version directory, never `current`.
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_discovered_path_is_concrete_and_never_the_current_link() {
        let (_home, root) = packaged_1_30_4();
        let found = discover_nginx(&root, &[]).unwrap();
        assert!(
            found
                .bin
                .starts_with(root.package_dir(NGINX_PACKAGE_NAME, NGINX_SERIES, "1.30.4")),
            "{:?} is not inside the concrete version directory",
            found.bin
        );
        assert!(
            !found.bin.components().any(|c| c.as_os_str() == "current"),
            "{:?} runs through the current link",
            found.bin
        );
    }

    /// THE assertion that pins D5, mirroring
    /// `crate::mysql::discover::a_current_swap_does_not_change_a_path_discovery_already_handed_out`.
    /// A path that merely *looks* concrete would pass the test above against
    /// a `current` link that happens to resolve — this one swaps the link
    /// underneath and demands the already-handed-out path still reach the
    /// binary it named.
    ///
    /// VACUITY, proven by mutation: rewriting [`packaged_nginx_runtime`] to
    /// build its path from `root.current_link(...)` instead of
    /// `root.package_dir(...)` — the exact mistake D5 forbids — fails this
    /// test: the read after the swap returns the NEW version's bytes,
    /// `b"1.30.5 server\n"`, instead of the original `b"1.30.4 server\n"`.
    #[cfg(unix)]
    #[test]
    fn a_current_swap_does_not_change_a_path_discovery_already_handed_out() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        install_fake_package(&root, "1.30.4", "1.30.4 server\n");
        install_fake_package(&root, "1.30.5", "1.30.5 server\n");
        point_current(&root, "1.30.4");

        let found = discover_nginx(&root, &[]).unwrap();
        let bin = found.bin.clone();
        assert_eq!(std::fs::read(&bin).unwrap(), b"1.30.4 server\n");

        // A `current` swap is a legitimate operation (a future upgrade flow
        // does exactly this). It must not reach back and change what an
        // already-resolved path names.
        point_current(&root, "1.30.5");
        assert_eq!(
            std::fs::read(&bin).unwrap(),
            b"1.30.4 server\n",
            "a current swap changed the binary an already-resolved path reaches"
        );

        // ...and a fresh discovery DOES follow the swap, which is what makes
        // the assertion above a statement about D5 rather than about a
        // broken symlink.
        let after = discover_nginx(&root, &[]).unwrap();
        assert_eq!(std::fs::read(&after.bin).unwrap(), b"1.30.5 server\n");
    }

    // ------------------------------------------------------------------
    // Group 4 — a tampered `current` link is refused, mirroring
    // `crate::mysql::discover`'s identical proof for the same containment
    // rule (reused here via `crate::mysql::current_version`, not
    // re-implemented).
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_current_link_escaping_the_series_directory_is_refused() {
        let (_home, root) = packaged_1_30_4();
        assert!(discover_nginx(&root, &[]).is_some(), "baseline");
        for target in ["../../elsewhere", "/etc", "a/b", "..", "./1.30.4"] {
            point_current(&root, target);
            assert!(
                packaged_nginx_runtime(&root).is_none(),
                "current -> {target:?} must be refused"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_current_link_naming_a_version_that_is_not_installed_yields_nothing() {
        let (_home, root) = packaged_1_30_4();
        point_current(&root, "1.30.99");
        assert!(packaged_nginx_runtime(&root).is_none());
    }

    #[test]
    fn a_missing_current_link_yields_no_packaged_runtime() {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        assert!(packaged_nginx_runtime(&root).is_none());
    }
}
