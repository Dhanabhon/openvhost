// SPDX-License-Identifier: GPL-3.0-or-later
//! Which PHP major the catch-all server block serves — resolved from a stored
//! preference against what is actually installed.
//!
//! **A preference is a preference; resolving it is a separate step that can
//! fail** (default-PHP design D2). The stored major can stop being installed —
//! the user uninstalls it, or a keg disappears — and that outcome has to be
//! *representable*, not collapsed into "no preference". This project has
//! shipped four defects of exactly that shape (a boolean that could not express
//! `Failed`; an offer union that could not express `awaitingRelease`; a
//! `fallback_brew()` that invented a path; a `brewFound` bool answering a
//! per-major question), so [`DefaultPhp`] spends a variant on each distinct
//! outcome rather than reusing one.

use crate::site::apply::PhpRuntime;
use crate::site::model::PhpVersion;

/// Which PHP major the catch-all (`00-default_server.conf`) serves, **and
/// why**.
///
/// Produced only by [`DefaultPhp::resolve`], which is what makes the variants
/// trustworthy: every `String` in here came out of the installed set that was
/// resolved against, never out of the stored preference, so the major that
/// reaches [`crate::site::apply::socket_path`] always names a runtime for which
/// a php-fpm pool is rendered in the same pass. A struct carrying "the chosen
/// major" *plus* a separate reason field could be built with the two
/// disagreeing; this cannot.
///
/// Matched **exhaustively** everywhere — never through a wildcard arm — so a
/// fifth outcome breaks compilation at every site that has to decide about it
/// instead of quietly falling into someone else's arm. [`Self::serving_major`]
/// is the ONE place that turns an outcome into "the major to actually render",
/// so a caller that only needs the config cannot get that derivation subtly
/// wrong, and the exhaustive match lives there once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultPhp {
    /// No preference is stored **and** no PHP is installed, so the catch-all
    /// gets no PHP `location` block at all. The pre-existing behaviour of an
    /// empty `InstalledRuntimes::php` — an honest absence, not a failure.
    NothingInstalled,
    /// No preference is stored, so the historical rule applies: the FIRST
    /// entry of the discovered order (`discover_php`'s byte-lexicographic sort
    /// by major).
    ///
    /// This is every machine that predates the preference, and the reason the
    /// slice is inert until someone sets one. The sort is a display order being
    /// borrowed to make a runtime selection — the accident this whole design
    /// exists to end — but it is NOT corrected here: doing so would change what
    /// every existing machine serves, which the design (D3) explicitly refuses.
    Unset {
        /// The major actually served: `runtimes.php.first()`'s.
        serving: String,
    },
    /// A preference is stored and that major IS installed. The chosen case.
    Preferred {
        /// The preferred major, taken from the matching installed runtime.
        major: String,
    },
    /// A preference is stored but that major is **not installed** — uninstalled
    /// since, or a keg that disappeared.
    ///
    /// The catch-all still has to serve something, so this falls back to the
    /// same first-installed rule as [`Self::Unset`] — but as a *named* state
    /// carrying what was asked for, so the app can say "your default was 8.4,
    /// which is no longer installed" instead of quietly serving 8.1.
    PreferredMissing {
        /// The major the stored preference names.
        requested: String,
        /// The major actually served instead — `None` when nothing at all is
        /// installed, which is the one case where the fallback has nothing to
        /// fall back to.
        serving: Option<String>,
    },
}

impl DefaultPhp {
    /// Resolve a stored preference against the installed runtimes.
    ///
    /// `installed` is `InstalledRuntimes::php`, **in discovery order** — the
    /// first entry is what the historical rule selects, and both fallback arms
    /// below reproduce that exactly.
    ///
    /// A matched preference yields the **installed runtime's** major string,
    /// not the stored one. They compare equal, so this changes no byte of any
    /// output; it means every major that leaves this function has discovery as
    /// its provenance, which is one less thing for a reader of
    /// [`crate::site::apply::socket_path`] to have to establish.
    pub fn resolve(preference: Option<&PhpVersion>, installed: &[PhpRuntime]) -> DefaultPhp {
        let first = installed.first().map(|rt| rt.major.clone());
        let Some(preference) = preference else {
            return match first {
                Some(serving) => DefaultPhp::Unset { serving },
                None => DefaultPhp::NothingInstalled,
            };
        };
        match installed.iter().find(|rt| rt.major == preference.as_str()) {
            Some(rt) => DefaultPhp::Preferred {
                major: rt.major.clone(),
            },
            None => DefaultPhp::PreferredMissing {
                requested: preference.as_str().to_string(),
                serving: first,
            },
        }
    }

    /// The major the catch-all actually serves, or `None` when it gets no PHP
    /// `location` block at all.
    ///
    /// THE single derivation of "what gets rendered" from an outcome. Every
    /// consumer goes through it, so the fallback rule for
    /// [`Self::PreferredMissing`] is written down once rather than re-derived
    /// (differently) at each call site.
    pub(crate) fn serving_major(&self) -> Option<&str> {
        match self {
            DefaultPhp::NothingInstalled => None,
            DefaultPhp::Unset { serving } => Some(serving),
            DefaultPhp::Preferred { major } => Some(major),
            DefaultPhp::PreferredMissing { serving, .. } => serving.as_deref(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::php::PhpRuntimeSource;
    use std::path::PathBuf;

    fn installed(majors: &[&str]) -> Vec<PhpRuntime> {
        majors
            .iter()
            .map(|m| PhpRuntime {
                major: (*m).to_string(),
                fpm_bin: PathBuf::from(format!("/opt/homebrew/opt/php@{m}/sbin/php-fpm")),
                source: PhpRuntimeSource::Homebrew,
            })
            .collect()
    }

    fn pref(v: &str) -> PhpVersion {
        PhpVersion::parse(v).unwrap()
    }

    #[test]
    fn no_preference_serves_the_first_discovered_runtime() {
        // The historical rule, stated as a test so a future edit to `resolve`
        // that "tidies" it into `min()` fails here rather than silently moving
        // what every existing machine serves.
        let rt = installed(&["8.1", "8.3"]);
        assert_eq!(
            DefaultPhp::resolve(None, &rt),
            DefaultPhp::Unset {
                serving: "8.1".to_string()
            }
        );
        assert_eq!(DefaultPhp::resolve(None, &rt).serving_major(), Some("8.1"));
    }

    #[test]
    fn no_preference_follows_the_slice_order_not_the_lowest_major() {
        // `first()`, NOT `min()`. The list arrives pre-sorted from
        // `discover_php`, but `resolve` must not re-derive that itself: an
        // unsorted list has to come out in list order, or "the first entry is
        // the catch-all's runtime" has quietly become a different rule.
        let rt = installed(&["8.3", "8.1"]);
        assert_eq!(DefaultPhp::resolve(None, &rt).serving_major(), Some("8.3"));
    }

    #[test]
    fn no_preference_and_nothing_installed_is_its_own_state() {
        assert_eq!(DefaultPhp::resolve(None, &[]), DefaultPhp::NothingInstalled);
        assert_eq!(DefaultPhp::resolve(None, &[]).serving_major(), None);
    }

    #[test]
    fn a_preference_naming_an_installed_major_wins_over_the_first() {
        let rt = installed(&["8.1", "8.3"]);
        assert_eq!(
            DefaultPhp::resolve(Some(&pref("8.3")), &rt),
            DefaultPhp::Preferred {
                major: "8.3".to_string()
            }
        );
        assert_eq!(
            DefaultPhp::resolve(Some(&pref("8.3")), &rt).serving_major(),
            Some("8.3")
        );
    }

    #[test]
    fn a_preference_naming_the_first_major_is_still_reported_as_chosen() {
        // Same served major as `Unset` would give, DIFFERENT state. If these
        // two collapsed, "you have chosen 8.1" and "8.1 is what you happen to
        // get" would be indistinguishable — which is the exact conflation D2
        // forbids.
        let rt = installed(&["8.1", "8.3"]);
        let resolved = DefaultPhp::resolve(Some(&pref("8.1")), &rt);
        assert_eq!(
            resolved,
            DefaultPhp::Preferred {
                major: "8.1".to_string()
            }
        );
        assert_ne!(
            resolved,
            DefaultPhp::Unset {
                serving: "8.1".to_string()
            }
        );
    }

    #[test]
    fn a_preference_that_is_not_installed_names_itself_and_still_serves() {
        // No panic, no empty upstream, no silent substitution: the fallback is
        // taken AND the request that could not be honoured is carried out.
        let rt = installed(&["8.1", "8.3"]);
        let resolved = DefaultPhp::resolve(Some(&pref("8.4")), &rt);
        assert_eq!(
            resolved,
            DefaultPhp::PreferredMissing {
                requested: "8.4".to_string(),
                serving: Some("8.1".to_string()),
            }
        );
        assert_eq!(resolved.serving_major(), Some("8.1"));
    }

    #[test]
    fn an_unresolvable_preference_is_never_equal_to_no_preference_at_all() {
        // The defect shape this type exists to prevent, asserted directly:
        // "8.4 is gone, serving 8.1" and "nobody chose, serving 8.1" agree on
        // what is served and must still be distinguishable.
        let rt = installed(&["8.1"]);
        let missing = DefaultPhp::resolve(Some(&pref("8.4")), &rt);
        let unset = DefaultPhp::resolve(None, &rt);
        assert_eq!(missing.serving_major(), unset.serving_major());
        assert_ne!(missing, unset);
    }

    #[test]
    fn a_preference_with_nothing_installed_reports_the_request_and_serves_nothing() {
        // The one case where the fallback has nothing to fall back to. Must
        // still name what was asked for — collapsing it into
        // `NothingInstalled` would lose the only fact worth reporting.
        let resolved = DefaultPhp::resolve(Some(&pref("8.4")), &[]);
        assert_eq!(
            resolved,
            DefaultPhp::PreferredMissing {
                requested: "8.4".to_string(),
                serving: None,
            }
        );
        assert_eq!(resolved.serving_major(), None);
        assert_ne!(resolved, DefaultPhp::NothingInstalled);
    }

    #[test]
    fn a_matched_preference_takes_its_string_from_the_installed_runtime() {
        // Provenance, not equality: the two are byte-equal by construction
        // (that is how the match was made), so this pins that the value
        // returned is the discovered one and a later edit cannot start
        // forwarding the stored string into a socket filename.
        let rt = installed(&["8.3"]);
        let DefaultPhp::Preferred { major } = DefaultPhp::resolve(Some(&pref("8.3")), &rt) else {
            panic!("expected Preferred");
        };
        assert_eq!(major, rt[0].major);
    }
}
