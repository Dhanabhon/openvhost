// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP's own package tree, on the wire (off-Homebrew slice 5C, design D1/D3).
//!
//! A sibling of `mysql_pkg` and `mariadb_pkg` rather than more of `commands.rs`
//! (already ~10 000 lines), holding the two wire types the Languages page needs
//! now that a PHP can arrive from somewhere other than Homebrew:
//!
//! 1. **Where a listed runtime came from** — [`PhpRuntimeSourceDto`]. Slice 5B
//!    made `openvhost_core::PhpRuntimeSource` real and deliberately did NOT add
//!    its wire copy, because nothing rendered it yet. This is that copy.
//! 2. **Whether this build publishes a package for a major on this host** —
//!    [`PhpPackageOfferDto`].
//!
//! **PHP is in MariaDB's situation, not MySQL's.** `MysqlPackageOfferDto` has
//! two states because Oracle publishes its binaries directly, so a pinned entry
//! is fetchable the moment it exists. PHP's artifact is one *we* build and
//! publish (php-recipe design D5), so — exactly like MariaDB's — a pin can be
//! completely correct while the release that would serve it does not exist yet.
//! That is [`PhpPackageOfferDto::AwaitingRelease`], and collapsing it into
//! `Unavailable` would tell an Apple Silicon owner their machine is unsupported
//! when the truth is "nobody can have this yet".
//!
//! **Per major, not per app** (design D1). MariaDB ships one series and left
//! `major` off its own types; PHP's whole point is several majors side by side,
//! so the offer is answered per major and rides on the row
//! ([`crate::commands::PhpRuntimeDto::offer`]) rather than on the environment.
//!
//! **Today every offer this build can make is `AwaitingRelease` or
//! `Unavailable`** — `php-8.4.24` is pinned but unpublished, and no other major
//! has a built artifact at all (`openvhost_core::PHP_PACKAGES`). Nothing here
//! is therefore installable, which is precisely why no install command lives in
//! this module yet: `commands::install_php` installs through Homebrew for every
//! major, which is also every real machine's only route today.

use openvhost_core::PackageTarget;
use openvhost_core::php::Availability;

/// Where a listed PHP runtime's binaries came from — the wire copy of
/// `openvhost_core::PhpRuntimeSource` (PHP-discovery design D1, slice 5B).
///
/// Transcribed from `NginxRuntimeSourceDto`/`MysqlRuntimeSourceDto` rather than
/// reinvented: all three ask the identical question — "which install put these
/// bytes here" — and nothing about PHP's answer needs a different shape.
/// `PhpRuntimeSource::as_str()` stays the one machine-facing spelling for each
/// source; `the_wire_tag_is_php_runtime_source_as_str` below pins this type's
/// serialized `kind` to it for every variant, so the two cannot drift into
/// different words for the same fact.
///
/// `Homebrew` carries **no version, on purpose**, and this is the field that
/// makes the asymmetry visible: a packaged runtime's exact version is a
/// directory name chosen at install time, so reporting it costs nothing, while
/// Homebrew's would have to be probed — and the only prober we have returns
/// `major.minor`, never a patch level. Reporting the major as though it were
/// the full version would be a lie no caller could detect.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpRuntimeSourceDto {
    Packaged { version: String },
    Homebrew,
}

impl From<&openvhost_core::PhpRuntimeSource> for PhpRuntimeSourceDto {
    fn from(s: &openvhost_core::PhpRuntimeSource) -> Self {
        use openvhost_core::PhpRuntimeSource as S;
        match s {
            S::Packaged { version } => Self::Packaged {
                version: version.clone(),
            },
            S::Homebrew => Self::Homebrew,
        }
    }
}

/// Whether this build can install a given PHP major from its own package tree
/// on THIS host, and what it would install — the three states
/// `MariadbPackageOfferDto` spells (`mariadb_pkg.rs`), mirrored exactly.
///
/// Matched exhaustively wherever it is consumed, with **no wildcard arm**: a
/// fourth state must be decided about rather than silently folded into one of
/// the first three.
///
/// `AwaitingRelease`'s own meaning is the one that matters today: the next
/// action belongs to the **maintainer, not the user**, so a row in that state
/// must say what it is waiting for rather than offer a button that would 404.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PhpPackageOfferDto {
    Available {
        version: String,
    },
    /// The pinned build exists and was audited, but the GitHub release that
    /// would serve it has not been published — the next action belongs to the
    /// maintainer, not the user. `tag` is the release to publish, e.g.
    /// `"php-8.4.24"`.
    AwaitingRelease {
        tag: String,
    },
    Unavailable {
        target: String,
    },
}

/// What this build would install for `major` on `target`.
///
/// `target` is an explicit `Option`, mirroring `mysql_pkg::package_offer_for`
/// and `mariadb_pkg::package_offer_for` exactly and for the identical reason:
/// **both refusal branches must be reachable from a test on any one machine.**
/// A mutation that returned an offer for every refusal once survived a whole
/// suite green on Apple Silicon because the Intel arm never executed there.
///
/// `major` is a `&str`, unlike `openvhost_core::php_package_for_target`'s
/// `&PhpMajor`, because the Languages page has rows this build does not manage
/// at all — a hand-installed 7.4, or a major a later catalogue drops (see
/// `PhpRuntimeDto::cataloged`). Such a major cannot be parsed into a
/// `PhpMajor` (that constructor is catalogue-gated, deliberately, because it
/// also guards a `brew` argv), and the honest answer for it is the same
/// absence a cataloged-but-unbuilt 8.1 gets: this build publishes no artifact
/// for it. Parsing is therefore done here and **any** failure to resolve is an
/// absence — this deliberately does not read an error payload to decide, so a
/// future refusal reason cannot accidentally become an offer.
///
/// Nothing here reaches a child process, a URL or a hash: the lookup is a
/// compiled-in table (`openvhost_core::PHP_PACKAGES`) keyed by a parsed major
/// and a `PackageTarget`.
pub(crate) fn package_offer_for(major: &str, target: Option<PackageTarget>) -> PhpPackageOfferDto {
    // `PackageTarget` is named through its own `as_str`; `None` is the host
    // this programme publishes nothing for at all.
    let named = match target {
        Some(t) => t.as_str().to_string(),
        None => "this host".to_string(),
    };
    let Ok(major) = openvhost_core::PhpMajor::parse(major) else {
        return PhpPackageOfferDto::Unavailable { target: named };
    };
    match openvhost_core::php_package_for_target(&major, target) {
        // Exhaustive on `php::Availability`, no wildcard: a third availability
        // state would have to be decided about here too, not silently treated
        // as an offer.
        Ok(entry) => match entry.availability {
            Availability::Published => PhpPackageOfferDto::Available {
                version: entry.version.to_string(),
            },
            Availability::AwaitingRelease { tag } => PhpPackageOfferDto::AwaitingRelease {
                tag: tag.to_string(),
            },
        },
        Err(_) => PhpPackageOfferDto::Unavailable { target: named },
    }
}

/// What this build would install for `major` on the host it was compiled for.
pub(crate) fn package_offer(major: &str) -> PhpPackageOfferDto {
    package_offer_for(major, PackageTarget::host())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tag_of(value: &impl serde::Serialize) -> String {
        serde_json::to_value(value).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // ------------------------------------------------------------------
    // Group 1 — the offer, and its third state.
    //
    // VACUITY: returning `Unavailable` unconditionally from
    // `package_offer_for` reddens
    // `apple_silicon_is_offered_awaiting_release_while_the_pin_is_unpublished`
    // alone; returning `AwaitingRelease` unconditionally reddens the three
    // absence tests below. Both were run.
    // ------------------------------------------------------------------

    /// The state this build is in TODAY for the one pinned major: the release
    /// is not published, so the offer is `AwaitingRelease` — never `Available`
    /// (which would send the user at a 404) and never `Unavailable` (which
    /// would tell an Apple Silicon owner their machine is unsupported).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn apple_silicon_is_offered_awaiting_release_while_the_pin_is_unpublished() {
        let offer = package_offer_for("8.4", Some(PackageTarget::MacosArm64));
        match offer {
            PhpPackageOfferDto::AwaitingRelease { tag } => assert_eq!(tag, "php-8.4.24"),
            other => {
                panic!("expected AwaitingRelease while the release is unpublished, got {other:?}")
            }
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_host_offer_agrees_with_the_explicit_arm64_offer_on_this_machine() {
        assert_eq!(PackageTarget::host(), Some(PackageTarget::MacosArm64));
        assert_eq!(
            package_offer("8.4"),
            package_offer_for("8.4", Some(PackageTarget::MacosArm64))
        );
    }

    /// **The single most load-bearing fact in this slice.** `AwaitingRelease`
    /// is what EVERY offer this build can make resolves to — there is exactly
    /// one pinned major, and its release does not exist — so it is the only
    /// non-absence state any row can carry today, and the state whose render
    /// the page must get right. What the row carries in it is a tag a human
    /// has to publish, and nothing a user can act on.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn awaiting_release_is_the_only_non_absence_offer_this_build_can_make_today() {
        let offered: Vec<(&str, PhpPackageOfferDto)> = openvhost_core::CATALOGUE
            .iter()
            .map(|major| (*major, package_offer(major)))
            .collect();
        for (major, offer) in &offered {
            match offer {
                // No `Available` anywhere: nothing is installable from the
                // package tree until a release is published. When one is, THIS
                // assertion is the tripwire that says the slice's untested
                // install path is now reachable.
                PhpPackageOfferDto::Available { version } => panic!(
                    "PHP {major} reports an installable {version}: the release was published, so \
                     the packaged install path is now reachable and is no longer unproven"
                ),
                PhpPackageOfferDto::AwaitingRelease { tag } => {
                    assert_eq!(*major, "8.4", "only 8.4 has a pinned build today");
                    assert_eq!(tag, "php-8.4.24");
                }
                PhpPackageOfferDto::Unavailable { target } => {
                    assert_eq!(target, "macos-arm64");
                }
            }
        }
        assert_eq!(
            offered
                .iter()
                .filter(|(_, o)| matches!(o, PhpPackageOfferDto::AwaitingRelease { .. }))
                .count(),
            1,
            "exactly one major is pinned-but-unpublished today"
        );
    }

    /// A major this build manages for Homebrew but has never built an artifact
    /// for. The absence is real and names the target, exactly as an
    /// unsupported architecture's does — a pinned catalogue entry is per-major
    /// work, not a URL template.
    #[test]
    fn a_cataloged_major_with_no_pinned_build_is_offered_nothing() {
        assert_eq!(
            package_offer_for("8.1", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
    }

    /// The Intel story: no signature-checked x86_64 artifact exists, so Intel
    /// is offered nothing and the absence names the target — never
    /// `AwaitingRelease`, which would wrongly suggest a build is coming.
    #[test]
    fn an_intel_host_is_offered_nothing_and_the_absence_names_the_target() {
        assert_eq!(
            package_offer_for("8.4", Some(PackageTarget::MacosX86_64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-x86_64".into()
            }
        );
    }

    #[test]
    fn a_host_this_programme_publishes_nothing_for_says_so_without_naming_an_arch() {
        assert_eq!(
            package_offer_for("8.4", None),
            PhpPackageOfferDto::Unavailable {
                target: "this host".into()
            }
        );
    }

    /// A hand-installed major outside the catalogue still gets an answer
    /// rather than a panic or a parse error: this build publishes nothing for
    /// it, on any target. The row's own `cataloged: false` is what tells the
    /// page this is a version it does not manage; the offer only says there
    /// are no bytes.
    #[test]
    fn a_major_outside_the_catalogue_is_offered_nothing() {
        assert_eq!(
            package_offer_for("7.4", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
        // And a value that is not even a version — the walk feeds row majors
        // read off a disk — is an absence too, never a panic.
        assert_eq!(
            package_offer_for("--build-from-source", Some(PackageTarget::MacosArm64)),
            PhpPackageOfferDto::Unavailable {
                target: "macos-arm64".into()
            }
        );
    }

    /// The three states must not be confusable on the wire: distinct tags,
    /// distinct shapes.
    ///
    /// The `match` is exhaustive with no wildcard on purpose — this is the
    /// compile-time site that makes a fourth variant a decision rather than a
    /// silent addition.
    #[test]
    fn the_three_offer_states_serialize_distinctly() {
        let all = [
            PhpPackageOfferDto::Available {
                version: "8.4.24".into(),
            },
            PhpPackageOfferDto::AwaitingRelease {
                tag: "php-8.4.24".into(),
            },
            PhpPackageOfferDto::Unavailable {
                target: "macos-x86_64".into(),
            },
        ];
        for offer in &all {
            // Every state carries exactly one payload field beside its tag,
            // and it is the one a user is shown.
            let wire = serde_json::to_value(offer).unwrap();
            let payload = match offer {
                PhpPackageOfferDto::Available { version } => ("version", version),
                PhpPackageOfferDto::AwaitingRelease { tag } => ("tag", tag),
                PhpPackageOfferDto::Unavailable { target } => ("target", target),
            };
            assert_eq!(wire[payload.0].as_str(), Some(payload.1.as_str()));
        }
        let tags: Vec<String> = all.iter().map(tag_of).collect();
        for (i, a) in tags.iter().enumerate() {
            for (j, b) in tags.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "{:?} and {:?} share a tag", all[i], all[j]);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Group 2 — the source on the wire.
    //
    // VACUITY: mapping `Packaged` to `Self::Homebrew` in the `From` impl
    // reddens both tests below; dropping `version` from the `Packaged` arm
    // reddens `a_packaged_source_carries_the_exact_version_it_was_installed_at`.
    // Both were run.
    // ------------------------------------------------------------------

    /// ONE spelling for each source. `PhpRuntimeSource::as_str()` is the
    /// definition; this pins the wire tag to it for every variant, matched
    /// exhaustively so a third source cannot be added without deciding what it
    /// is called here.
    #[test]
    fn the_wire_tag_is_php_runtime_source_as_str() {
        let all = [
            openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".into(),
            },
            openvhost_core::PhpRuntimeSource::Homebrew,
        ];
        for source in &all {
            let dto = PhpRuntimeSourceDto::from(source);
            assert_eq!(tag_of(&dto), source.as_str(), "{source:?}");
        }
    }

    #[test]
    fn a_packaged_source_carries_the_exact_version_it_was_installed_at() {
        let dto = PhpRuntimeSourceDto::from(&openvhost_core::PhpRuntimeSource::Packaged {
            version: "8.4.24".into(),
        });
        assert_eq!(
            dto,
            PhpRuntimeSourceDto::Packaged {
                version: "8.4.24".into()
            }
        );
        // And Homebrew's answer is a different shape entirely, not a version
        // string that happens to be missing: the two must not be confusable.
        let brew = PhpRuntimeSourceDto::from(&openvhost_core::PhpRuntimeSource::Homebrew);
        assert_eq!(brew, PhpRuntimeSourceDto::Homebrew);
        assert_ne!(
            serde_json::to_value(&dto).unwrap(),
            serde_json::to_value(&brew).unwrap()
        );
    }

    /// Each state carries exactly the payload its tag promises — and a
    /// Homebrew row carries **no version key at all**, rather than one that
    /// happens to be null, so a consumer cannot read an absent patch level as
    /// an empty one.
    ///
    /// Exhaustive over this DTO with **no wildcard**, and that is the point of
    /// writing it as a `match`: a variant added to the wire type has to be
    /// given a tag and a payload here rather than reaching the webview as a
    /// shape nothing has described. Measured: adding a throwaway variant to
    /// `PhpRuntimeSourceDto` failed to compile at exactly this site.
    #[test]
    fn every_source_state_carries_exactly_the_payload_its_tag_promises() {
        for dto in [
            PhpRuntimeSourceDto::Packaged {
                version: "8.4.24".into(),
            },
            PhpRuntimeSourceDto::Homebrew,
        ] {
            let wire = serde_json::to_value(&dto).unwrap();
            let keys = wire.as_object().unwrap().len();
            match &dto {
                PhpRuntimeSourceDto::Packaged { version } => {
                    assert_eq!(wire["kind"], "packaged");
                    assert_eq!(wire["version"], version.as_str());
                    assert_eq!(keys, 2, "got {wire:?}");
                }
                PhpRuntimeSourceDto::Homebrew => {
                    assert_eq!(wire["kind"], "homebrew");
                    assert_eq!(keys, 1, "a Homebrew source must carry no payload: {wire:?}");
                }
            }
        }
    }
}
