// SPDX-License-Identifier: GPL-3.0-or-later
//! What a Homebrew `opt/<formula>` link actually points at.
//!
//! Homebrew keeps every installed version in `<prefix>/Cellar/<formula>/<version>`
//! and publishes a stable entry point for each formula at `<prefix>/opt/<formula>`,
//! a symlink into that keg. Two facts fall out of resolving that link, and this
//! app needs both:
//!
//! 1. **The version, without executing anything.** `…/Cellar/mysql@8.4/8.4.11`
//!    states the version in the path. Reading it is a `readlink`; asking the
//!    binary is a process launch that on macOS can take **11.5 s the first time**
//!    a freshly extracted 55 MB `mysqld` runs (Gatekeeper's first-run scan of a
//!    file carrying `com.apple.provenance`), which is well past the 5 s bound
//!    `openvhost_conf::PROBE_TIMEOUT` puts on a probe. That is not hypothetical:
//!    it made a *successful* `brew install mysql@8.4` come back as "not detected"
//!    every single time, because the probe was killed before it could answer and
//!    a killed probe was indistinguishable from "nothing installed".
//!
//! 2. **Whose keg it is.** SECURITY-ADJACENT, and the reason this module exists
//!    at all. Homebrew ALIASES the versioned name of the current release onto the
//!    unversioned formula: on a machine where `php` is 8.5.9,
//!    `/opt/homebrew/opt/php@8.5` resolves to `…/Cellar/php/8.5.9`, and
//!    `brew uninstall php@8.5` therefore removes **`php`** — the user's linked
//!    PHP, breaking `php` system-wide — while every string this app would show
//!    says `php@8.5`. The string shown and the keg removed are not the same
//!    thing. [`keg_provenance`] is how a caller refuses instead of guessing.
//!
//! Nothing here spawns a process or writes anything; every function is a
//! `canonicalize` plus string work.

use std::path::{Path, PathBuf};

/// The directory Homebrew keeps its kegs in, directly under a prefix.
///
/// Compared case-INSENSITIVELY wherever it is matched: the default macOS volume
/// is case-insensitive, so a keg reached through a differently-cased path
/// component must still be recognised as a keg. Recognising it is the
/// conservative direction — the alternative is failing to notice that a keg
/// belongs to another formula.
const CELLAR: &str = "Cellar";

/// A resolved Homebrew keg — the `…/Cellar/<owner>/<version>` directory an
/// `opt/<formula>` link points into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeg {
    /// The formula that OWNS this keg: `php@8.4` for a versioned formula,
    /// `php` when the versioned name was one of brew's aliases for the
    /// unversioned formula. Compare it against the formula you asked for —
    /// [`keg_provenance`] does exactly that.
    pub owner: String,
    /// The keg directory's own name, i.e. brew's full version, possibly with a
    /// revision suffix: `8.4.11`, `8.4.13_1`.
    pub version: String,
    /// The canonical keg directory.
    pub path: PathBuf,
}

impl ResolvedKeg {
    /// `8.4` out of `8.4.11` (and out of `8.4.13_1` — brew's revision suffix
    /// rides on the patch component, which this never looks at).
    ///
    /// `None` when the keg directory is not `<digits>.<digits>…` shaped, which
    /// is a keg this app cannot name a major for; the caller falls back to its
    /// probe rather than inventing one.
    pub fn major_minor(&self) -> Option<String> {
        let mut parts = self.version.split('.');
        let major = parts.next()?;
        let minor = parts.next()?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        // `then` and not `then_some`: the format! must not run when the guard
        // is false.
        (digits(major) && digits(minor)).then(|| format!("{major}.{minor}"))
    }
}

/// Resolve one `<prefix>/opt/<formula>` link to the keg it points into.
///
/// `None` when the link does not exist, cannot be resolved (a dangling
/// symlink, an unreadable directory), or resolves somewhere that is not
/// `…/Cellar/<owner>/<version>` — a layout this app does not model. Callers
/// treat `None` as "I could not tell", NEVER as "it is fine": see
/// [`keg_provenance`]'s `Unresolved`.
pub fn resolve_keg(opt_link: &Path) -> Option<ResolvedKeg> {
    // `canonicalize`, not `read_link`: brew writes RELATIVE link targets
    // (`../Cellar/php/8.5.9`), and a prefix that is itself a symlink has to be
    // resolved before the `Cellar` component can be found at all.
    let real = std::fs::canonicalize(opt_link).ok()?;
    let mut components: Vec<&std::ffi::OsStr> = real.components().map(|c| c.as_os_str()).collect();
    let version = components.pop()?.to_str()?.to_string();
    let owner = components.pop()?.to_str()?.to_string();
    let cellar = components.pop()?.to_str()?;
    if !cellar.eq_ignore_ascii_case(CELLAR) {
        return None;
    }
    Some(ResolvedKeg {
        owner,
        version,
        path: real,
    })
}

/// Whether `brew uninstall <formula>` would remove `<formula>`'s OWN keg, or
/// somebody else's.
///
/// Exhaustively matched by callers with no wildcard arm: the difference between
/// these three is the difference between removing one version and breaking the
/// user's `php`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KegProvenance {
    /// `opt/<formula>` resolves into `Cellar/<formula>/…`. The formula owns its
    /// keg, so `brew uninstall <formula>` removes that version and nothing this
    /// app can name.
    OwnKeg { keg: PathBuf },
    /// `opt/<formula>` resolves into `Cellar/<owner>/…` for a DIFFERENT
    /// `owner` — in practice because brew aliases the current release's
    /// versioned name onto the unversioned formula. `brew uninstall <formula>`
    /// resolves that alias and removes `<owner>`.
    ForeignKeg { owner: String, keg: PathBuf },
    /// Nothing under any searched prefix resolved to a keg. NOT a synonym for
    /// `OwnKeg`: an absent or unreadable `opt` link is no evidence that the
    /// formula name is safe to hand to `brew uninstall` — brew resolves its own
    /// aliases from its taps, with or without a link here.
    Unresolved { searched: Vec<PathBuf> },
}

/// Classify the keg `opt/<formula>` points into, searching `prefixes` in order
/// and taking the first that resolves.
///
/// Prefix ORDER matters and is the caller's to choose: `crate::BREW_PREFIXES`
/// is Apple Silicon before Intel, the same order discovery uses, so a machine
/// with both is classified against the same installation the app would run.
pub fn keg_provenance(prefixes: &[&Path], formula: &str) -> KegProvenance {
    let mut searched: Vec<PathBuf> = Vec::new();
    for prefix in prefixes {
        let link = prefix.join("opt").join(formula);
        searched.push(link.clone());
        let Some(keg) = resolve_keg(&link) else {
            continue;
        };
        // Case-insensitive for the same reason `CELLAR` is: on a
        // case-insensitive volume `Cellar/PHP` and `Cellar/php` are one
        // directory, and treating them as different owners would report a
        // formula as foreign to its own keg.
        return if keg.owner.eq_ignore_ascii_case(formula) {
            KegProvenance::OwnKeg { keg: keg.path }
        } else {
            KegProvenance::ForeignKeg {
                owner: keg.owner,
                keg: keg.path,
            }
        };
    }
    KegProvenance::Unresolved { searched }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Build `<root>/Cellar/<owner>/<version>` and link
    /// `<root>/opt/<formula>` at it — brew's real layout, with brew's real
    /// RELATIVE link target, so `resolve_keg` is exercised against the shape it
    /// meets in production rather than an absolute link it would never see.
    #[cfg(unix)]
    fn brew_layout(root: &Path, formula: &str, owner: &str, version: &str) {
        let keg = root.join("Cellar").join(owner).join(version);
        std::fs::create_dir_all(&keg).unwrap();
        let opt = root.join("opt");
        std::fs::create_dir_all(&opt).unwrap();
        std::os::unix::fs::symlink(
            PathBuf::from("..").join("Cellar").join(owner).join(version),
            opt.join(formula),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_versioned_formula_owns_its_own_keg() {
        let dir = tempfile::tempdir().unwrap();
        brew_layout(dir.path(), "php@8.4", "php@8.4", "8.4.13");
        match keg_provenance(&[dir.path()], "php@8.4") {
            KegProvenance::OwnKeg { keg } => {
                assert!(keg.ends_with("Cellar/php@8.4/8.4.13"), "got {keg:?}")
            }
            other => panic!("expected OwnKeg, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_alias_shape_this_machine_actually_has_is_reported_as_foreign() {
        // Reproduces the live-proof finding verbatim: `brew info php@8.5`
        // reports `Aliases: php@8.5` on `php`, and
        // `/opt/homebrew/opt/php@8.5 -> ../Cellar/php/8.5.9`. Uninstalling
        // "php@8.5" here removes the user's linked `php`.
        let dir = tempfile::tempdir().unwrap();
        brew_layout(dir.path(), "php@8.5", "php", "8.5.9");
        match keg_provenance(&[dir.path()], "php@8.5") {
            KegProvenance::ForeignKeg { owner, keg } => {
                assert_eq!(owner, "php");
                assert!(keg.ends_with("Cellar/php/8.5.9"), "got {keg:?}");
            }
            other => panic!("expected ForeignKeg, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_same_alias_shape_is_reported_for_mysql_too() {
        // Not a hypothetical branch: brew applies the identical aliasing rule
        // to every versioned-formula family, so the day `mysql` is the major
        // this build offers, `opt/mysql@X` resolves into `Cellar/mysql`. One
        // classifier covers both families, so neither can be forgotten.
        let dir = tempfile::tempdir().unwrap();
        brew_layout(dir.path(), "mysql@9.4", "mysql", "9.4.0");
        assert!(matches!(
            keg_provenance(&[dir.path()], "mysql@9.4"),
            KegProvenance::ForeignKeg { .. }
        ));
    }

    #[test]
    fn a_formula_with_no_opt_link_anywhere_is_unresolved_not_ok() {
        let dir = tempfile::tempdir().unwrap();
        match keg_provenance(&[dir.path()], "php@8.4") {
            KegProvenance::Unresolved { searched } => {
                assert_eq!(searched, vec![dir.path().join("opt").join("php@8.4")]);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_opt_link_is_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let opt = dir.path().join("opt");
        std::fs::create_dir_all(&opt).unwrap();
        std::os::unix::fs::symlink("../Cellar/php@8.4/8.4.13", opt.join("php@8.4")).unwrap();
        assert!(matches!(
            keg_provenance(&[dir.path()], "php@8.4"),
            KegProvenance::Unresolved { .. }
        ));
    }

    #[test]
    fn a_directory_that_is_not_under_cellar_is_unresolved() {
        // A layout this app does not model. "I do not recognise this" must not
        // collapse into "this formula owns its keg".
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("opt").join("php@8.4")).unwrap();
        assert!(matches!(
            keg_provenance(&[dir.path()], "php@8.4"),
            KegProvenance::Unresolved { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_first_prefix_that_resolves_wins() {
        // Apple Silicon before Intel: a machine with both must be classified
        // against the installation discovery would also pick.
        let silicon = tempfile::tempdir().unwrap();
        let intel = tempfile::tempdir().unwrap();
        brew_layout(silicon.path(), "php@8.4", "php@8.4", "8.4.13");
        brew_layout(intel.path(), "php@8.4", "php", "8.4.13");
        assert!(matches!(
            keg_provenance(&[silicon.path(), intel.path()], "php@8.4"),
            KegProvenance::OwnKeg { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_prefix_that_does_not_resolve_falls_through_to_the_next() {
        let empty = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        brew_layout(real.path(), "php@8.4", "php@8.4", "8.4.13");
        assert!(matches!(
            keg_provenance(&[empty.path(), real.path()], "php@8.4"),
            KegProvenance::OwnKeg { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_differently_cased_owner_is_still_the_same_formula() {
        // The default macOS volume is case-insensitive: `Cellar/PHP@8.4` and
        // `Cellar/php@8.4` are one directory there. Reporting the formula as
        // foreign to its own keg would refuse a perfectly ordinary uninstall.
        let dir = tempfile::tempdir().unwrap();
        brew_layout(dir.path(), "php@8.4", "PHP@8.4", "8.4.13");
        // On a case-insensitive volume the link resolves through the name that
        // was created first; either way the comparison must not be what decides.
        assert!(matches!(
            keg_provenance(&[dir.path()], "php@8.4"),
            KegProvenance::OwnKeg { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_version_comes_out_of_the_keg_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        brew_layout(dir.path(), "mysql@8.4", "mysql@8.4", "8.4.11");
        let keg = resolve_keg(&dir.path().join("opt").join("mysql@8.4")).unwrap();
        assert_eq!(keg.version, "8.4.11");
        assert_eq!(keg.major_minor().as_deref(), Some("8.4"));
    }

    #[test]
    fn a_brew_revision_suffix_does_not_disturb_the_major() {
        // `8.4.13_1` is brew's revision notation; the suffix rides on the patch
        // component, which the major/minor read never looks at.
        let keg = ResolvedKeg {
            owner: "php@8.4".into(),
            version: "8.4.13_1".into(),
            path: PathBuf::from("/x"),
        };
        assert_eq!(keg.major_minor().as_deref(), Some("8.4"));
    }

    #[test]
    fn a_keg_name_that_is_not_a_version_yields_no_major() {
        for version in ["HEAD", "", "8", "8.", ".4", "eight.four", "8.x"] {
            let keg = ResolvedKeg {
                owner: "php".into(),
                version: version.to_string(),
                path: PathBuf::from("/x"),
            };
            assert!(
                keg.major_minor().is_none(),
                "accepted {version:?} as a version"
            );
        }
    }
}
