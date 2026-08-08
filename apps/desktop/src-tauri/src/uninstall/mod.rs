// SPDX-License-Identifier: GPL-3.0-or-later
//! Removing an installed PHP or MySQL major (package-uninstall design
//! `2026-07-31-p1-pkg-uninstall-design.md`, D1–D5).
//!
//! This module is the PURE half: what an uninstall removes, what it keeps, and
//! what stops it. [`run`] is the half that actually does anything.
//!
//! The organising rule (plan Task 2) is that the removals and the keeps are
//! **data, not scattered `if`s**: [`inventory`] is the single source both the
//! confirmation dialog and the executor read, so the sentence a user is shown
//! and the sequence a machine performs cannot disagree. A new [`PackageKind`]
//! must therefore fail to COMPILE here rather than silently removing nothing.
//!
//! The keeps are not decoration. Design D2: `brew uninstall mysql@8.4` removes
//! binaries and has no idea `<home>/data/mysql/8.4` exists — deleting it would
//! be the single most destructive thing this app could do, and keeping the data
//! while throwing away the root password is the same as destroying it. Every
//! path named in `keeps` is asserted byte- and inode-identical after a
//! successful uninstall, after a failed one, and after a refusal (see `run`'s
//! filesystem tests).

pub(crate) mod run;

use std::path::{Path, PathBuf};

use openvhost_core::mysql::MysqlMajor;
use openvhost_core::{KegProvenance, PhpMajor, Site};
use openvhost_proc::{ServiceState, ServiceStatus, SpawnSpec};

use crate::commands::{InstallKind, IpcError};

/// Which family of package an uninstall targets.
///
/// Exhaustively matched everywhere with **no wildcard arm** — adding a variant
/// must break the build in [`Target::parse`] and [`inventory`], because the
/// alternative (a `_` arm) is an uninstall that reports success having removed
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PackageKind {
    Php,
    Mysql,
    Mariadb,
}

/// One thing that SURVIVES the uninstall, in the user's words.
///
/// `path` is `Some` for anything that lives on disk (so design D6's dialog can
/// say "they stay in `<home>/data/mysql/8.4`" rather than a vague
/// reassurance), and `None` for things that have no path — a row in state.db,
/// a setting on every site. It is NOT an existence check: naming where a
/// directory would be is true whether or not it has been created yet, and
/// stat-ing here would make a pure query depend on the moment it ran.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KeptItem {
    pub what: String,
    pub path: Option<String>,
    /// The one item the confirmation's headline sentence is ABOUT — "Your
    /// databases are not touched — they stay in `<path>`". Exactly one entry
    /// per plan carries it.
    ///
    /// An explicit flag rather than "whichever entry happens to come first
    /// with a path", which is what the UI used to do. Under that rule a
    /// reorder of this list silently changed which directory a destructive
    /// dialog reassures the user about, and the only thing that would fail was
    /// a full-vector `assert_eq!` whose natural fix — update the expected
    /// vector — carries no signal at all that the dialog just started naming
    /// `my.cnf` as the place the user's databases live.
    pub headline: bool,
}

/// Why an uninstall is refused (design D3). Both are refusals, not warnings,
/// and there is deliberately no `--force`: a user who wants the version gone
/// can stop the service or change the sites first, which is the same work with
/// the consequences visible.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Blocker {
    /// The version's service is not in a terminal state. Never auto-stopped:
    /// stopping a database mid-write as a side effect of a menu click is
    /// exactly the surprise this app should not spring.
    #[serde(rename_all = "camelCase")]
    ServiceNotTerminal { id: String, state: String },
    /// Sites are still set to this PHP major. Never silently repointed: that
    /// would edit the user's configuration without asking AND could move a
    /// site onto a PHP version its code does not run on.
    #[serde(rename_all = "camelCase")]
    SitesPinned { domains: Vec<String> },
    /// The keg `brew uninstall <formula>` would remove belongs to a DIFFERENT
    /// formula — brew has aliased this version's name onto another one.
    ///
    /// Not hypothetical, and the reason this variant exists: on a machine where
    /// Homebrew's unversioned `php` is 8.5.9, `brew info php@8.5` reports
    /// `Aliases: php@8.5` and `/opt/homebrew/opt/php@8.5` resolves to
    /// `Cellar/php/8.5.9`. This app would discover 8.5, offer Uninstall, say it
    /// removes "the `php@8.5` formula" — and `brew uninstall php@8.5` would
    /// resolve the alias and remove the user's **linked `php`**, breaking `php`
    /// system-wide. The string shown and the keg removed are not the same
    /// thing.
    ///
    /// A refusal, like every other blocker (D3), extending "never destroy user
    /// data" to "never destroy the user's environment". A user who does mean it
    /// runs `brew uninstall <owner>` themselves, where the consequence is
    /// visible.
    #[serde(rename_all = "camelCase")]
    ForeignKeg {
        /// What this app would have passed to `brew uninstall`, e.g. `php@8.5`.
        formula: String,
        /// The formula that actually owns the keg, e.g. `php` — what would be
        /// removed.
        owner: String,
        /// The keg itself, e.g. `/opt/homebrew/Cellar/php/8.5.9`.
        keg: String,
    },
    /// Nothing under any known Homebrew prefix resolved to a keg for this
    /// formula, so this app cannot say what `brew uninstall <formula>` would
    /// remove.
    ///
    /// Deliberately NOT folded into "fine, proceed". An absent or unreadable
    /// `opt` link is no evidence that the name is safe to hand to brew — brew
    /// resolves its own aliases from its taps whether or not a link exists
    /// here, so the [`ForeignKeg`](Blocker::ForeignKeg) danger is fully present
    /// in this case too, just unprovable. Refusing fails visibly and leaves the
    /// user a manual path; proceeding would fail quietly and take their `php`
    /// with it.
    #[serde(rename_all = "camelCase")]
    UnknownKeg {
        formula: String,
        /// Every `opt` path that was looked at, so the refusal is diagnosable
        /// rather than merely discouraging.
        searched: Vec<String>,
    },
}

impl Blocker {
    /// The refusal in one sentence, for the error a racing execute returns.
    /// The dialog renders the structured form instead; this exists so that
    /// wording and this wording come from types, not from two copies of a
    /// string.
    pub(crate) fn describe(&self) -> String {
        match self {
            Blocker::ServiceNotTerminal { id, state } => {
                format!("{id} is {state}. Stop it first, then try again.")
            }
            Blocker::SitesPinned { domains } => format!(
                "these sites are still set to this PHP version: {}. Change them first, \
                 then try again.",
                domains.join(", ")
            ),
            Blocker::ForeignKeg {
                formula,
                owner,
                keg,
            } => format!(
                "Homebrew treats {formula} as an alias for its unversioned {owner} formula — \
                 it resolves to {keg}. Removing it would remove your linked {owner}, not just \
                 this version, so OpenVHost will not do it. Run `brew uninstall {owner}` \
                 yourself if that is what you mean."
            ),
            Blocker::UnknownKeg { formula, searched } => format!(
                "OpenVHost could not work out which Homebrew keg {formula} refers to (looked \
                 at: {}), and will not run `brew uninstall {formula}` without knowing what \
                 that would remove. Run it yourself if you are sure.",
                searched.join(", ")
            ),
        }
    }
}

/// What an uninstall would do, for the confirmation dialog and for the
/// disabled state. Produced by the pure-query command `uninstall_plan`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UninstallPlan {
    pub kind: PackageKind,
    pub major: String,
    /// Human-readable, in the order the executor performs them.
    pub removes: Vec<String>,
    /// What survives, with paths where they exist.
    pub keeps: Vec<KeptItem>,
    /// Empty => may proceed. Non-empty => the action is refused, and these say
    /// why and what to do about it.
    pub blockers: Vec<Blocker>,
}

/// What OpenVHost's OWN package tree holds for the target an uninstall names —
/// runtime state, resolved by the caller and handed to the pure half.
///
/// The exact counterpart of [`KegProvenance`], and for the same reason
/// (off-Homebrew slice 5D design D1). [`inventory`] must not stat anything: a
/// destructive operation whose plan was recomputed against the disk at
/// execution time could show a dialog saying it removes X while the executor
/// removes Y. So the two filesystem reads this needs — which version directory
/// `current` selects, and whether Homebrew also has this major — happen in
/// [`Target::packaged`], and only the ANSWER crosses into the plan.
///
/// `Some(_)` means the row the user pressed Uninstall on is ours; `None` means
/// it is Homebrew's, which is every machine that has never installed a
/// packaged PHP and is therefore today's behaviour, unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackagedPhp {
    /// The CONCRETE version directory — `packages/php/8.4/8.4.24`, never
    /// `packages/php/8.4/current` (design D4). `current` is a link whose target
    /// can move; recording it would mean the directory the dialog named and the
    /// directory `run`'s `remove_dir_all` reaches are two different questions
    /// asked at two different moments.
    pub(crate) version_dir: PathBuf,
    /// The Homebrew keg for the SAME major, which this uninstall LEAVES ALONE
    /// (design D3), or `None` when Homebrew has no PHP under this major.
    ///
    /// A machine can have both, and discovery shows one row — ours. Removing
    /// both would destroy more than that row described; removing ours in
    /// silence would leave a rescan still showing the major, which a user would
    /// reasonably read as "the uninstall failed". So it is named under
    /// [`UninstallPlan::keeps`], which exists for exactly this.
    pub(crate) brew_keg: Option<PathBuf>,
}

/// The Homebrew prefixes to search, in `openvhost-core`'s own order (Apple
/// Silicon before Intel) so this module classifies a keg against the same
/// installation discovery would run.
///
/// A function rather than a `const`: `openvhost_core::BREW_PREFIXES` is an
/// array of `&'static str`, and turning it into `&Path`s requires dereferencing
/// each element — `.map(Path::new)` over `.iter()` would borrow the temporary
/// array instead, which is why the two call sites here cannot simply share a
/// static.
fn brew_prefixes() -> Vec<&'static Path> {
    openvhost_core::BREW_PREFIXES
        .iter()
        .map(|p| Path::new(*p))
        .collect()
}

/// The keg `formula` resolves to on this machine, when there is one — the
/// directory a plan names under [`UninstallPlan::keeps`] as surviving.
///
/// Exhaustive over [`KegProvenance`] with **no wildcard arm**, and the two
/// `Some` arms are not an oversight:
///
/// * `OwnKeg` is the plain case — Homebrew has this formula's own keg.
/// * `ForeignKeg` is Homebrew's alias trap seen from the other side. On a
///   machine where `php@8.5` is an alias for the unversioned `php`, removing a
///   packaged 8.5 still leaves that keg behind, and a rescan will still show
///   8.5 — so it is exactly the case where saying nothing would read as "the
///   uninstall failed" (design D3's rejected alternative). It is a keg, it
///   survives, it gets named. Nothing is refused here: a refusal is
///   [`keg_blocker`]'s job, and it only applies when a `brew uninstall` is
///   actually going to run.
/// * `Unresolved` means nothing was found to keep. Note this is the OPPOSITE
///   reading to [`Blocker::UnknownKeg`]'s, and correctly so: there, "I could
///   not tell" must not authorise a destructive `brew uninstall`; here it only
///   decides whether a reassurance is printed about a keg no evidence says
///   exists.
///
/// `prefixes` is a parameter rather than [`brew_prefixes`] read inline for the
/// reason [`Target::keg_provenance`]'s own doc gives about the executor: a test
/// that could not choose them would consult the developer's own
/// `/opt/homebrew` and pass or fail on what happens to be installed there.
fn brew_keg_path(prefixes: &[&Path], formula: &str) -> Option<PathBuf> {
    match openvhost_core::keg_provenance(prefixes, formula) {
        KegProvenance::OwnKeg { keg } => Some(keg),
        KegProvenance::ForeignKeg { keg, .. } => Some(keg),
        KegProvenance::Unresolved { .. } => None,
    }
}

/// A package kind paired with its VALIDATED major — the only way anything in
/// this module names a formula, a path or a service id.
///
/// Both constructors are the catalogue-gated `parse` the matching install
/// command already uses, deliberately: it means the formula reaching `brew`'s
/// argv can be influenced by nothing but a version this build offers, with no
/// new, more permissive path into a child process's argv. The cost is that a
/// major installed outside the catalogue (a hand-installed `php@7.4`, a
/// discovered `mysql@9.7`) cannot be uninstalled from the app — it is removed
/// with `brew uninstall` in a terminal, and design D5's rescan then converges
/// the app onto the same state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Php(PhpMajor),
    Mysql(MysqlMajor),
    /// MariaDB carries no major/version of its own: this build ships exactly
    /// one series (`openvhost_core::MARIADB_SERIES`), and there is no
    /// `MariadbMajor` newtype to parse into — the same reasoning
    /// `MariadbInstanceRepo`'s own doc comment gives for leaving `major` off
    /// `MariadbInstance`. A unit variant rather than one carrying the series
    /// string is what makes "there is nothing to vary" a fact the type states
    /// rather than a convention every reader has to remember.
    Mariadb,
}

impl Target {
    pub(crate) fn parse(kind: PackageKind, major: &str) -> Result<Self, IpcError> {
        Ok(match kind {
            PackageKind::Php => Target::Php(PhpMajor::parse(major)?),
            PackageKind::Mysql => Target::Mysql(MysqlMajor::parse(major)?),
            // No `MariadbMajor::parse` exists to delegate to (there is only
            // ever one legal value), so the gate is inline here — but it is
            // still a gate: an out-of-band `major` is refused with the same
            // field-shaped `IpcError::Validation` every other rejected
            // version uses, never silently accepted.
            PackageKind::Mariadb if major == openvhost_core::MARIADB_SERIES => Target::Mariadb,
            PackageKind::Mariadb => {
                return Err(IpcError::Validation {
                    field: "mariadb_version".to_string(),
                    message: format!(
                        "OpenVHost ships MariaDB {} only",
                        openvhost_core::MARIADB_SERIES
                    ),
                });
            }
        })
    }

    pub(crate) fn kind(&self) -> PackageKind {
        match self {
            Target::Php(_) => PackageKind::Php,
            Target::Mysql(_) => PackageKind::Mysql,
            Target::Mariadb => PackageKind::Mariadb,
        }
    }

    pub(crate) fn major(&self) -> &str {
        match self {
            Target::Php(m) => m.as_str(),
            Target::Mysql(m) => m.as_str(),
            Target::Mariadb => openvhost_core::MARIADB_SERIES,
        }
    }

    /// The supervisor row this version owns, derived from `stack`'s own id
    /// builders so it can never name a row the registration side did not
    /// create.
    pub(crate) fn service_id(&self) -> String {
        match self {
            Target::Php(m) => crate::stack::php_fpm_service_id(m.as_str()),
            Target::Mysql(m) => crate::stack::mysql_service_id(m.as_str()),
            Target::Mariadb => crate::stack::mariadb_service_id(openvhost_core::MARIADB_SERIES),
        }
    }

    /// How this version reads in a sentence — "PHP 8.4", "MySQL 8.4",
    /// "MariaDB 11.4".
    pub(crate) fn display(&self) -> String {
        match self {
            Target::Php(m) => format!("PHP {}", m.as_str()),
            Target::Mysql(m) => format!("MySQL {}", m.as_str()),
            Target::Mariadb => format!("MariaDB {}", openvhost_core::MARIADB_SERIES),
        }
    }

    /// The `InstallLock` slot discriminator, and the label that slot carries.
    /// Matches the shapes the install commands use exactly (PHP's label is
    /// bare, MySQL's and MariaDB's are complete phrases) — see
    /// `PendingInstallDto`.
    pub(crate) fn install_kind(&self) -> InstallKind {
        match self {
            Target::Php(_) => InstallKind::Php,
            Target::Mysql(_) => InstallKind::Mysql,
            Target::Mariadb => InstallKind::Mariadb,
        }
    }

    pub(crate) fn pending_label(&self) -> String {
        match self {
            Target::Php(m) => m.as_str().to_string(),
            Target::Mysql(m) => format!("MySQL {}", m.as_str()),
            Target::Mariadb => format!("MariaDB {}", openvhost_core::MARIADB_SERIES),
        }
    }

    /// The Homebrew formula THIS uninstall would name — THE definition, read
    /// from `openvhost-core` so the string a dialog shows, the string a refusal
    /// quotes and the string that reaches `brew`'s argv are one expression
    /// rather than three that can drift.
    ///
    /// `None` for MariaDB (P1 MariaDB UI design D5): a packaged MariaDB has no
    /// Homebrew origin and never will, so there is no correct formula string —
    /// not `""`, which would be a silent empty value in user-facing copy, and
    /// not `"mariadb"`, which would name a formula this app never installs and
    /// cannot uninstall. The type admits the absence instead, and every caller
    /// below ([`Self::keg_provenance`], [`Self::uninstall_spec`]) has to
    /// decide what an absent formula means for it, rather than inheriting a
    /// PHP/MySQL assumption by default.
    ///
    /// **`None` for a PHP major this app packaged too** (off-Homebrew slice 5D
    /// design D2), which is why this takes `packaged` rather than being a
    /// function of `self` alone. The question is not "does Homebrew have a name
    /// for this version" — it always does — but "does THIS uninstall run
    /// `brew uninstall`", and for a packaged row the answer is no: its program
    /// files are [`Removal::PackageTree`], the way MariaDB's are. Answering
    /// `Some` there would offer to remove a formula that need not be installed
    /// at all, and on the both-installed machine would destroy the Homebrew keg
    /// D3 promises to keep.
    ///
    /// Every consequence follows from this one seam, which is the argument for
    /// putting it here rather than special-casing each site: [`Self::
    /// keg_provenance`] stops looking up an alias for a `brew uninstall` that
    /// will not run, [`blockers`] stops refusing on it, and `uninstall_package`
    /// stops demanding Homebrew be installed before it will remove a directory.
    pub(crate) fn formula(&self, packaged: Option<&PackagedPhp>) -> Option<String> {
        match self {
            Target::Php(_) if packaged.is_some() => None,
            Target::Php(m) => Some(openvhost_core::brew_formula(m)),
            // Not gated on `packaged`, and NOT because a packaged MySQL cannot
            // exist — one can, and `install_mysql_package` is wired. This slice
            // is scoped to PHP (design §9), so MySQL's arm is left exactly as
            // it was rather than changed without a spec; the identical gap is
            // recorded in 5D's report for its own slice.
            Target::Mysql(m) => Some(openvhost_core::mysql_brew_formula(m)),
            Target::Mariadb => None,
        }
    }

    /// What [`Self::formula`]'s `opt` link actually resolves to on THIS
    /// machine — `None` when there is no formula to look one up for (MariaDB,
    /// or a PHP major this app packaged).
    ///
    /// A filesystem read (a `canonicalize`), so it lives here rather than
    /// inside the pure [`blockers`] — the caller performs it and passes the
    /// answer in, exactly as it does for the supervisor snapshot and the site
    /// list. That keeps `blockers` a pure function of its inputs and keeps the
    /// executor's tests off the developer's own `/opt/homebrew`.
    ///
    /// For a packaged PHP the `None` is not a shrug: the alias trap this
    /// resolves exists because `brew uninstall php@8.5` can remove the user's
    /// linked `php`, and an uninstall that spawns no `brew` cannot spring it.
    /// The keg is still NAMED, under [`PackagedPhp::brew_keg`] — as something
    /// kept, not something checked.
    pub(crate) fn keg_provenance(&self, packaged: Option<&PackagedPhp>) -> Option<KegProvenance> {
        let formula = self.formula(packaged)?;
        Some(openvhost_core::keg_provenance(&brew_prefixes(), &formula))
    }

    /// What OpenVHost's own package tree holds for this target, and the
    /// Homebrew keg that tree's uninstall would leave alone.
    ///
    /// **The filesystem read, kept out of the plan.** Two cheap calls — a
    /// `read_link` plus an `is_file` for our own tree, and a `canonicalize` per
    /// Homebrew prefix — performed HERE and handed to [`inventory`] and
    /// [`build_plan`] as a value, exactly as [`Self::keg_provenance`] is. It is
    /// re-read by the executor rather than carried over from whatever plan a
    /// dialog was built from, for the same reason the blockers are: an install
    /// can finish, or a `current` link swing, while a confirmation sits open.
    ///
    /// The packaged answer comes from
    /// [`openvhost_core::packaged_php_install`] — the very predicate
    /// `discover_php` used to build the row the user pressed Uninstall on, not
    /// a second opinion about it. That is what makes "remove what the row
    /// described" (design D3) a property rather than a hope.
    pub(crate) fn packaged(&self, home: &Path) -> Option<PackagedPhp> {
        match self {
            Target::Php(major) => {
                let root = openvhost_core::PackagesRoot::from_home(home);
                let install = openvhost_core::packaged_php_install(&root, major.as_str())?;
                Some(PackagedPhp {
                    version_dir: install.dir,
                    // The same builder `Self::formula` delegates to, called
                    // directly for the same reason `inventory`'s PHP arm calls
                    // it directly: `Self::formula` answers "what would this
                    // uninstall name", and on this path that is deliberately
                    // `None` — while the question here is the different one of
                    // what Homebrew calls this major, whoever removes it.
                    brew_keg: brew_keg_path(&brew_prefixes(), &openvhost_core::brew_formula(major)),
                })
            }
            // Not `None` because a packaged MySQL cannot exist — see
            // `Self::formula`'s MySQL arm for why this slice leaves it alone.
            Target::Mysql(_) => None,
            // MariaDB's package tree needs nothing threaded in: it has exactly
            // one series and `inventory` builds its `PackageTree` path from
            // compile-time constants, which is what let it ship before this
            // parameter existed.
            Target::Mariadb => None,
        }
    }

    /// `brew uninstall <formula>`, composed entirely inside `openvhost-core`.
    ///
    /// The MariaDB arm is unreachable in normal operation — [`inventory`]
    /// never emits a [`Removal::BrewFormula`] step for [`Target::Mariadb`], so
    /// nothing calls this for it — but the match stays exhaustive and explicit
    /// rather than a wildcard: a future edit that DID wire a `BrewFormula`
    /// removal to MariaDB by mistake gets a clear refusal here instead of a
    /// formula string quietly built from nothing.
    pub(crate) fn uninstall_spec(&self, brew: &Path) -> Result<SpawnSpec, IpcError> {
        Ok(match self {
            Target::Php(m) => openvhost_core::brew_uninstall_spec(brew, m)?,
            Target::Mysql(m) => openvhost_core::mysql_brew_uninstall_spec(brew, m)?,
            Target::Mariadb => {
                return Err(IpcError::Core {
                    message: "MariaDB has no Homebrew formula to uninstall".to_string(),
                });
            }
        })
    }
}

/// One step of an uninstall. The executor matches this exhaustively; the plan
/// renders [`Removal::describe`]. Same value, two readers — which is what stops
/// the confirmation text from promising something the executor never does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Removal {
    /// `brew uninstall <formula>` — the program files.
    BrewFormula { formula: String, what: String },
    /// A file THIS app generated. Never a directory, never recursive: see
    /// `run`'s executor for why `remove_file` is the only filesystem call an
    /// uninstall makes.
    GeneratedFile { path: PathBuf, what: String },
    /// A package tree THIS APP's OWN installer created under
    /// `<home>/packages/` — never a Homebrew keg. Emitted whenever
    /// [`Target::formula`] is `None`, i.e. whenever there is no `brew
    /// uninstall` to spawn: MariaDB always (P1 MariaDB UI design D5), and a PHP
    /// major this app packaged (off-Homebrew slice 5D design D2).
    ///
    /// **`path` is NOT built from compile-time constants alone.** It was, while
    /// MariaDB was the only source — `MARIADB_PACKAGE_NAME`/`MARIADB_SERIES`
    /// joined onto the resolved home — and that sentence stood here until slice
    /// 5D made it false. PHP's version directory carries a component read off
    /// the disk: `packages/php/<major>/<version>/`, where `<version>` is
    /// whatever this major's `current` link named.
    ///
    /// What holds instead, and is the property to check when auditing the
    /// `remove_dir_all` this feeds:
    ///
    /// * every component is produced inside `openvhost-core`, through
    ///   `PackagesRoot`'s layout facade, from a resolved home — **no IPC or
    ///   client input reaches any of them**, which is the guarantee the old
    ///   sentence was really making;
    /// * `<major>` is a validated `PhpMajor` (catalogue-gated at
    ///   [`Target::parse`]) or a compile-time constant;
    /// * `<version>` has passed `openvhost_core::mysql::current_version`'s
    ///   single-`Component::Normal` rule and `packaged_php_install`'s
    ///   direct-child check, so it cannot be `..`, absolute, or multi-component
    ///   — see [`openvhost_core::packaged_php_install`];
    /// * `path` names the CONCRETE version directory and never routes through
    ///   the `current` link (design D4).
    ///
    /// None of that is traversal-proof on its own, and it is not claimed to be:
    /// a symlink at an INTERMEDIATE component still redirects the delete, which
    /// is why design D4 requires the executor to canonicalise this path against
    /// the packages root and refuse if it escapes, rather than trusting a check
    /// made three layers up by a caller it cannot see. See `run`'s executor for
    /// the removal itself.
    PackageTree { path: PathBuf, what: String },
    /// `Supervisor::unregister` — the row on the Services page and in the tray.
    ServiceRow { id: String },
}

impl Removal {
    pub(crate) fn describe(&self) -> String {
        match self {
            // The second sentence is design D6's promise ("the confirmation
            // states what is removed") meeting an observed fact: `brew
            // uninstall php@8.3` also removed `aspell` (768 files, 338 MB), and
            // `brew uninstall mysql@8.4` also removed `abseil`, `protobuf` and
            // `zlib-ng-compat`. Naming one formula while brew quietly takes
            // four is not "stating what is removed".
            //
            // It is attached to the brew step rather than added as a separate
            // list entry on purpose: `removes` is the executor's own list, one
            // entry per step, and this caveat is a property of THIS step. It
            // deliberately does NOT try to predict the set — `brew uninstall
            // --dry-run` is another child process and another failure mode on a
            // path that must stay a pure query, and a prediction that went
            // stale between the dialog and the run would be worse than an
            // honest "watch the output".
            Removal::BrewFormula { formula, what } => format!(
                "{what} (the {formula} formula). Homebrew may also remove dependencies it \
                 believes nothing else needs; its output names any it takes."
            ),
            Removal::GeneratedFile { path, what } => format!("{what} at {}", path.display()),
            // No Homebrew caveat: unlike `BrewFormula`, this removal takes
            // exactly what is named and nothing else — there is no dependency
            // graph for OpenVHost's own package tree to pull in extra kegs
            // from.
            Removal::PackageTree { path, what } => format!("{what} at {}", path.display()),
            Removal::ServiceRow { id } => format!("The {id} entry in Services"),
        }
    }
}

/// The removals and the keeps for one target — the single source described in
/// this module's own docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub(crate) removes: Vec<Removal>,
    pub(crate) keeps: Vec<KeptItem>,
}

fn kept(what: &str, path: Option<PathBuf>) -> KeptItem {
    KeptItem {
        what: what.to_string(),
        path: path.map(|p| p.display().to_string()),
        headline: false,
    }
}

/// The one kept item the confirmation's headline sentence is about — see
/// [`KeptItem::headline`]. Exactly one per inventory arm, pinned by
/// `exactly_one_kept_item_is_the_headline_for_every_kind`.
fn kept_headline(what: &str, path: Option<PathBuf>) -> KeptItem {
    KeptItem {
        headline: true,
        ..kept(what, path)
    }
}

/// What removing `target` does, and what it deliberately leaves alone.
///
/// Exhaustive over [`Target`] — and therefore over [`PackageKind`] — with no
/// wildcard arm, so a third package family cannot inherit PHP's inventory (or
/// an empty one) by default.
///
/// A pure function of `(target, home, packaged)`: it does not stat anything,
/// so the plan a dialog shows and the sequence the executor runs are the same
/// value even if the disk changed in between. The one consequence is that
/// `Removal::ServiceRow` is listed even when no row happens to be registered
/// (an installed-but-never-initialized MySQL major has none) — the executor
/// treats "already absent" as done, which is the honest reading of a removal
/// anyway.
///
/// `packaged` is the third input rather than a lookup for exactly that reason
/// (off-Homebrew slice 5D design D1): whether a PHP major's program files are
/// a Homebrew keg or a directory of ours is a question about the disk, and
/// asking it HERE would make the answer depend on the moment the question was
/// asked. [`Target::packaged`] asks it once, in the caller; the value crosses;
/// the plan stays a function of what it was handed. See [`PackagedPhp`].
///
/// `None` on any non-PHP target — the parameter describes PHP's two install
/// sources, and the other arms ignore it by construction rather than by
/// convention.
pub(crate) fn inventory(target: &Target, home: &Path, packaged: Option<&PackagedPhp>) -> Inventory {
    match target {
        Target::Php(major) => {
            let m = major.as_str();
            // `PhpVersion::parse` cannot actually fail for a value
            // `PhpMajor::parse` already accepted (the same digit-dot-digit
            // shape, a longer length limit) — but "cannot fail" is not a
            // reason to `.expect()`, and a kept item with no path still tells
            // the user their logs survive, which is the part that matters.
            let log_dir = openvhost_core::PhpVersion::parse(m).ok().and_then(|v| {
                openvhost_core::LogPaths::new(home)
                    .php_fpm_error(&v)
                    .parent()
                    .map(Path::to_path_buf)
            });
            // The ONE structural difference a packaged install makes, and it is
            // deliberately the only one: the program files come off disk
            // directly instead of through `brew uninstall`. Everything below —
            // the generated pool config, the service row, the logs, the
            // overrides, the site settings — is identical whichever source
            // provided the binaries, because none of it was ever Homebrew's.
            let program_files = match packaged {
                // 5D D2/D4. `version_dir` is the concrete
                // `packages/php/<major>/<version>`, resolved by the caller and
                // never routed through `current` — see `Removal::PackageTree`'s
                // own doc comment for exactly what that path shape does and
                // does not guarantee.
                Some(pkg) => Removal::PackageTree {
                    path: pkg.version_dir.clone(),
                    what: format!("The PHP {m} program files"),
                },
                // Unchanged, and it is every machine that never installed a
                // packaged PHP.
                None => Removal::BrewFormula {
                    // Not `target.formula(packaged)`: that returns
                    // `Option<String>` — `None` for MariaDB (D5) and now for a
                    // packaged PHP (5D D2) — and this arm already knows it has
                    // one. Calling the same builder `Target::formula` itself
                    // delegates to is the one expression, not an `.expect()` on
                    // the option.
                    formula: openvhost_core::brew_formula(major),
                    what: format!("The PHP {m} program files"),
                },
            };
            // 5D D3. A machine can have both sources for one major and
            // discovery shows a single row, ours; this uninstall takes that row
            // and says out loud what it is walking past. `None` when Homebrew
            // has no PHP under this major, and `None` for a Homebrew row, where
            // the keg is what is being REMOVED.
            let surviving_keg = packaged.and_then(|pkg| pkg.brew_keg.as_ref()).map(|keg| {
                kept(
                    &format!("The Homebrew PHP {m} keg — untouched"),
                    Some(keg.clone()),
                )
            });
            Inventory {
                removes: vec![
                    program_files,
                    Removal::GeneratedFile {
                        path: crate::stack::php_pool_config_path(home, m),
                        what: "The generated php-fpm pool config".to_string(),
                    },
                    Removal::ServiceRow {
                        id: target.service_id(),
                    },
                ],
                keeps: vec![
                    // D2: a user often uninstalls a version BECAUSE it failed,
                    // and removing the evidence at the moment it becomes
                    // relevant is backwards. THE headline: the confirmation
                    // says "Your logs are not touched — they stay in <path>".
                    kept_headline(&format!("Your PHP {m} logs"), log_dir),
                    kept(
                        "Your own php-fpm pool overrides",
                        Some(home.join("config/custom/php").join(m).join("pool.d")),
                    ),
                    // D3: site PHP versions are left pointing at the removed
                    // major. That is the honest record of what the user
                    // configured; the apply pipeline already rejects it with a
                    // validation error, which is visible and recoverable,
                    // whereas a silent repoint is neither.
                    kept(
                        &format!("Every site's saved PHP version — a site set to {m} keeps it"),
                        None,
                    ),
                ]
                .into_iter()
                // Appended rather than inserted: the headline stays first, and
                // the three entries above stay byte-for-byte where a brew-only
                // machine has always seen them.
                .chain(surviving_keg)
                .collect(),
            }
        }
        Target::Mysql(major) => {
            let m = major.as_str();
            let paths = openvhost_core::mysql_paths(home, major);
            Inventory {
                removes: vec![
                    Removal::BrewFormula {
                        // See the PHP arm's matching comment above.
                        formula: openvhost_core::mysql_brew_formula(major),
                        what: format!("The MySQL {m} program files"),
                    },
                    // NO GeneratedFile here, and that is load-bearing rather
                    // than an omission: nothing re-renders `my.cnf` for a
                    // datadir this app finds ALREADY initialized (see
                    // `stack::mysql_spec`), so deleting it would leave a
                    // reinstalled 8.4 spawning mysqld against a missing
                    // `--defaults-file` — breaking the very round trip D2
                    // promises. It is listed under `keeps` instead.
                    Removal::ServiceRow {
                        id: target.service_id(),
                    },
                ],
                keeps: vec![
                    // THE reason this slice exists (plan principle 2), and THE
                    // headline: the confirmation says "Your databases are not
                    // touched — they stay in <path>". That sentence must name
                    // the datadir and nothing else; naming `my.cnf` there would
                    // tell a user their databases live in a config file.
                    kept_headline("Your databases", Some(paths.datadir.clone())),
                    // D2: keeping the data and throwing away the key is the
                    // same as destroying it.
                    kept("The stored root password", None),
                    kept("This instance's my.cnf", Some(paths.my_cnf.clone())),
                    kept("Your own MySQL overrides", Some(paths.custom_confd.clone())),
                ],
            }
        }
        // MariaDB's own arm, deliberately shaped like MySQL's rather than
        // PHP's: no site setting references it, its datadir/credential/my.cnf
        // are kept for the identical reason MySQL's are, and the ONE
        // structural difference — no Homebrew formula, so the program files
        // are `Removal::PackageTree`, not `Removal::BrewFormula` — is exactly
        // the difference `Target::formula` returning `None` for this variant
        // exists to force a decision about (D5).
        Target::Mariadb => {
            let root = openvhost_core::PackagesRoot::from_home(home);
            let package_tree = root.major_dir(
                openvhost_core::MARIADB_PACKAGE_NAME,
                openvhost_core::MARIADB_SERIES,
            );
            let paths = openvhost_core::mariadb_paths(home);
            Inventory {
                removes: vec![
                    Removal::PackageTree {
                        path: package_tree,
                        what: format!(
                            "The MariaDB {} program files",
                            openvhost_core::MARIADB_SERIES
                        ),
                    },
                    Removal::ServiceRow {
                        id: target.service_id(),
                    },
                ],
                keeps: vec![
                    // Same headline shape as MySQL's, and for the same reason
                    // (plan principle 2): the confirmation says "Your
                    // databases are not touched — they stay in <path>", naming
                    // the datadir and nothing else.
                    kept_headline("Your databases", Some(paths.datadir.clone())),
                    kept("The stored root password", None),
                    kept("This instance's my.cnf", Some(paths.my_cnf.clone())),
                    kept(
                        "Your own MariaDB overrides",
                        Some(paths.custom_confd.clone()),
                    ),
                ],
            }
        }
    }
}

/// Whether a service in `state` blocks an uninstall, and what that state is
/// called in the refusal.
///
/// Exhaustive over [`ServiceState`] with **no wildcard arm**, deliberately: a
/// new variant must be classified here on purpose. Defaulting an unknown state
/// to "removable" would let `brew uninstall` delete the binaries of a running
/// service; defaulting it to "blocked" would quietly make some future state
/// un-uninstallable. Neither is a decision a `_` should make.
///
/// `openvhost_proc`'s own `check_terminal` makes the identical decision for
/// `Supervisor::unregister`, but it is private to that crate and Task 1
/// deliberately did not widen it — so this is a second, independent match by
/// design. The two cannot silently disagree about the NAMES, because the name
/// comes from `control::state_label`, the one vocabulary `openvhost status`
/// also speaks; and `agrees_with_the_supervisors_own_terminal_check` pins that
/// they cannot disagree about the DECISION either.
pub(crate) fn service_blocker(id: &str, state: &ServiceState) -> Option<Blocker> {
    match state {
        ServiceState::Stopped | ServiceState::Failed { .. } => None,
        ServiceState::Starting | ServiceState::Running => Some(Blocker::ServiceNotTerminal {
            id: id.to_string(),
            state: crate::control::state_label(state).to_string(),
        }),
    }
}

/// Whether the keg `brew uninstall <formula>` would remove is this version's
/// own, and the refusal when it is not.
///
/// Exhaustive over [`KegProvenance`] with **no wildcard arm**: the difference
/// between these three is the difference between removing one PHP version and
/// removing the user's linked `php`, which is not a decision a `_` should make.
///
/// `None` when `target` has no Homebrew formula at all ([`Target::formula`],
/// P1 MariaDB UI design D5). In production this never happens: [`blockers`]
/// only calls this function when it already holds a REAL `KegProvenance`,
/// which [`Target::keg_provenance`] only ever produces for a formula-having
/// target in the first place — so a formula-less target and a `Some(keg)`
/// cannot occur together on that path. Kept as a graceful `None` here too,
/// rather than fabricating a formula string, so that invariant does not have
/// to be trusted by a caller who reaches this function some other way (this
/// module's own tests among them).
///
/// **`packaged` is not a second copy of that state, it is the same one**
/// (off-Homebrew slice 5D): this check is about what a `brew uninstall` would
/// remove, so it must ask the same [`Target::formula`] the spawn will, with the
/// same argument. A packaged PHP therefore answers `None` here for the same
/// structural reason MariaDB does — there is no `brew uninstall` to protect.
pub(crate) fn keg_blocker(
    target: &Target,
    keg: &KegProvenance,
    packaged: Option<&PackagedPhp>,
) -> Option<Blocker> {
    let formula = target.formula(packaged)?;
    match keg {
        KegProvenance::OwnKeg { .. } => None,
        KegProvenance::ForeignKeg { owner, keg } => Some(Blocker::ForeignKeg {
            formula,
            owner: owner.clone(),
            keg: keg.display().to_string(),
        }),
        KegProvenance::Unresolved { searched } => Some(Blocker::UnknownKeg {
            formula,
            searched: searched.iter().map(|p| p.display().to_string()).collect(),
        }),
    }
}

/// Everything standing in the way of removing `target`, in the order a user
/// should read them (design D3).
///
/// Pure, and re-run by the executor rather than trusted from the plan: between
/// a dialog opening and its confirm button being pressed, a service can be
/// started from the tray and a site can be repointed. `keg` is read from the
/// filesystem by the caller for the same reason `services` and `sites` are —
/// see [`Target::keg_provenance`].
///
/// `keg` is `None` for a target with no Homebrew formula (MariaDB, D5; and a
/// packaged PHP, 5D D2): there is no `brew uninstall` for an alias to redirect,
/// so there is nothing for this check to refuse. `packaged` is passed through
/// to [`keg_blocker`] so that both ends of that sentence are decided by one
/// expression rather than by a caller remembering to pass `keg: None`.
///
/// The keg check comes FIRST because it is categorically different from the
/// other two: those say "do this, then retry", and this one says OpenVHost will
/// never do it.
pub(crate) fn blockers(
    target: &Target,
    services: &[ServiceStatus],
    sites: &[Site],
    keg: Option<&KegProvenance>,
    packaged: Option<&PackagedPhp>,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    if let Some(keg) = keg
        && let Some(blocker) = keg_blocker(target, keg, packaged)
    {
        out.push(blocker);
    }
    let id = target.service_id();
    if let Some(status) = services.iter().find(|s| s.id == id)
        && let Some(blocker) = service_blocker(&id, &status.state)
    {
        out.push(blocker);
    }
    match target {
        Target::Php(major) => {
            let domains: Vec<String> = sites
                .iter()
                .filter(|s| s.php_version.as_str() == major.as_str())
                .map(|s| s.domain.as_str().to_string())
                .collect();
            if !domains.is_empty() {
                out.push(Blocker::SitesPinned { domains });
            }
        }
        // Nothing in state.db pins a site to a MySQL major the way
        // `php_version` pins one to a PHP major, and the two things that DO
        // reference this major — the datadir and the credential row — are
        // KEPT by design D2, not obstacles to be cleared. An initialized
        // datadir full of the user's data must not block removing the engine:
        // that round trip (uninstall, reinstall, read the data back) is
        // precisely what D2 promises.
        Target::Mysql(_) => {}
        // Same reasoning as MySQL's arm immediately above, restated as its
        // own arm (rather than folded into it with an `|`) so a future third
        // formula-less engine cannot silently inherit "no blocker" without a
        // reviewer seeing a new line added here.
        Target::Mariadb => {}
    }
    out
}

/// Build the plan a dialog renders and a disabled state reads.
///
/// `disabled` is `!blockers.is_empty()`; the UI does not re-derive the rule.
///
/// Every argument after `target` is state the CALLER read — the supervisor
/// snapshot, the site list, the Homebrew keg and (off-Homebrew slice 5D)
/// OpenVHost's own package tree. Nothing in here or below it touches the disk,
/// which is what makes the value a dialog renders and the value the executor
/// walks the same value rather than two answers to the same question.
pub(crate) fn build_plan(
    target: &Target,
    home: &Path,
    services: &[ServiceStatus],
    sites: &[Site],
    keg: Option<&KegProvenance>,
    packaged: Option<&PackagedPhp>,
) -> UninstallPlan {
    let inv = inventory(target, home, packaged);
    UninstallPlan {
        kind: target.kind(),
        major: target.major().to_string(),
        removes: inv.removes.iter().map(Removal::describe).collect(),
        keeps: inv.keeps,
        blockers: blockers(target, services, sites, keg, packaged),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    pub(super) fn php(major: &str) -> Target {
        Target::parse(PackageKind::Php, major).expect("catalogue major")
    }

    pub(super) fn mysql(major: &str) -> Target {
        Target::parse(PackageKind::Mysql, major).expect("catalogue major")
    }

    pub(super) fn mariadb() -> Target {
        Target::parse(PackageKind::Mariadb, openvhost_core::MARIADB_SERIES)
            .expect("the pinned series")
    }

    /// The ordinary case: the formula owns its own keg, so nothing about
    /// Homebrew's aliasing stands in the way. Used everywhere a test is about
    /// something else.
    pub(super) fn own_keg() -> KegProvenance {
        KegProvenance::OwnKeg {
            keg: PathBuf::from("/opt/homebrew/Cellar/php@8.4/8.4.13"),
        }
    }

    fn status(id: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: None,
            pid: None,
            state,
        }
    }

    // ---- packaged-PHP fixtures (off-Homebrew slice 5D) -------------------

    /// The resolved packaged state for `<home>/packages/php/<major>/<version>`,
    /// built BY HAND rather than by resolving a real tree.
    ///
    /// That is the point of design D1: `inventory` is handed a value, so the
    /// tests of the plan need no disk at all and keep using the same
    /// nonexistent `/tmp/ovh` every other inventory test uses.
    /// [`Target::packaged`] — the half that does touch a filesystem — is tested
    /// separately, inside a `TempDir`.
    fn packaged_at(home: &str, major: &str, version: &str, brew_keg: Option<&str>) -> PackagedPhp {
        PackagedPhp {
            version_dir: PathBuf::from(home)
                .join("packages/php")
                .join(major)
                .join(version),
            brew_keg: brew_keg.map(PathBuf::from),
        }
    }

    /// The common case: ours, and Homebrew has nothing under this major.
    fn packaged_only(major: &str, version: &str) -> PackagedPhp {
        packaged_at("/tmp/ovh", major, version, None)
    }

    /// Lay down `packages/php/<major>/<version>/bin/php-fpm` under `home`,
    /// exactly where `build/recipes/php.sh` puts it (`bin`, never brew's
    /// `sbin`), and point `current` at it the way `openvhost-pkg` does — a
    /// RELATIVE symlink whose target is the bare version string.
    ///
    /// Mirrors `openvhost_core::php::discover`'s own fixtures deliberately: the
    /// property under test is that [`Target::packaged`] answers what discovery
    /// answered, so it has to be built the way discovery's tests build it.
    #[cfg(unix)]
    fn install_packaged_php(home: &Path, major: &str, version: &str) {
        let root = openvhost_core::PackagesRoot::from_home(home);
        let bin = root
            .package_dir(openvhost_core::PHP_PACKAGE_NAME, major, version)
            .join("bin/php-fpm");
        std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("create version dir");
        std::fs::write(&bin, format!("{version} fpm\n")).expect("write php-fpm");
    }

    /// Point (or re-point) `packages/php/<major>/current` at `version`.
    #[cfg(unix)]
    fn point_current(home: &Path, major: &str, version: &str) {
        let root = openvhost_core::PackagesRoot::from_home(home);
        let link = root.current_link(openvhost_core::PHP_PACKAGE_NAME, major);
        std::fs::create_dir_all(link.parent().expect("major dir")).expect("create major dir");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(PathBuf::from(version), &link).expect("link current");
    }

    /// Everything else a PHP inventory names, so a test that then deletes the
    /// tree is deleting something that was really there.
    #[cfg(unix)]
    fn provision_php_paths(home: &Path, major: &str) {
        for path in [
            crate::stack::php_pool_config_path(home, major),
            home.join("logs/services")
                .join(format!("php-fpm-{major}"))
                .join("error.log"),
            home.join("config/custom/php")
                .join(major)
                .join("pool.d/z.conf"),
        ] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
            std::fs::write(&path, b"x").expect("write file");
        }
    }

    /// brew's real layout: `<root>/Cellar/<owner>/<version>`, with
    /// `<root>/opt/<formula>` a RELATIVE symlink into it — the same fixture
    /// `openvhost_core::keg`'s own tests use, so `brew_keg_path` meets the
    /// shape it meets in production rather than an absolute link it never sees.
    #[cfg(unix)]
    fn brew_layout(root: &Path, formula: &str, owner: &str, version: &str) {
        let keg = root.join("Cellar").join(owner).join(version);
        std::fs::create_dir_all(&keg).expect("create keg");
        let opt = root.join("opt");
        std::fs::create_dir_all(&opt).expect("create opt");
        std::os::unix::fs::symlink(
            PathBuf::from("..").join("Cellar").join(owner).join(version),
            opt.join(formula),
        )
        .expect("link opt");
    }

    /// Every `ServiceState`, so the blocker predicate is exercised over the
    /// whole enum rather than the two variants that happened to come to mind.
    fn every_state() -> Vec<ServiceState> {
        vec![
            ServiceState::Stopped,
            ServiceState::Starting,
            ServiceState::Running,
            ServiceState::Failed {
                exit: Some(1),
                stderr_tail: vec!["boom".into()],
            },
        ]
    }

    // ---- the inventory, exhaustively per kind ----------------------------
    //
    // VACUITY (RED first): written before `inventory` existed — they did not
    // compile, then failed on every field. Additionally neutered afterwards:
    // deleting the `GeneratedFile` entry from the PHP arm failed
    // `a_php_uninstall_removes_the_formula_the_pool_config_and_the_row`, and
    // adding a `GeneratedFile { my_cnf }` entry to the MySQL arm failed
    // `a_mysql_uninstall_removes_only_the_formula_and_the_row`.

    #[test]
    fn a_php_uninstall_removes_the_formula_the_pool_config_and_the_row() {
        let home = Path::new("/tmp/ovh");
        let inv = inventory(&php("8.4"), home, None);
        assert_eq!(
            inv.removes,
            vec![
                Removal::BrewFormula {
                    formula: "php@8.4".to_string(),
                    what: "The PHP 8.4 program files".to_string(),
                },
                Removal::GeneratedFile {
                    path: PathBuf::from("/tmp/ovh/config/generated/php/8.4/php-fpm.conf"),
                    what: "The generated php-fpm pool config".to_string(),
                },
                Removal::ServiceRow {
                    id: "php-fpm-8.4".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_php_uninstall_keeps_the_logs_the_custom_pool_dir_and_every_site_setting() {
        let inv = inventory(&php("8.4"), Path::new("/tmp/ovh"), None);
        let paths: Vec<Option<&str>> = inv.keeps.iter().map(|k| k.path.as_deref()).collect();
        assert_eq!(
            paths,
            vec![
                Some("/tmp/ovh/logs/services/php-fpm-8.4"),
                Some("/tmp/ovh/config/custom/php/8.4/pool.d"),
                // No path: it is a column on every site row, not a file.
                None,
            ]
        );
    }

    #[test]
    fn a_mysql_uninstall_removes_only_the_formula_and_the_row() {
        // The absence of a `GeneratedFile` here is the assertion. Removing
        // my.cnf would break the reinstall round trip D2 promises, because
        // nothing re-renders it for an already-initialized datadir.
        let inv = inventory(&mysql("8.4"), Path::new("/tmp/ovh"), None);
        assert_eq!(
            inv.removes,
            vec![
                Removal::BrewFormula {
                    formula: "mysql@8.4".to_string(),
                    what: "The MySQL 8.4 program files".to_string(),
                },
                Removal::ServiceRow {
                    id: "mysql-8.4".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_mysql_uninstall_keeps_the_datadir_the_password_the_my_cnf_and_the_overrides() {
        let inv = inventory(&mysql("8.4"), Path::new("/tmp/ovh"), None);
        assert_eq!(
            inv.keeps,
            vec![
                KeptItem {
                    what: "Your databases".to_string(),
                    path: Some("/tmp/ovh/data/mysql/8.4".to_string()),
                    headline: true,
                },
                KeptItem {
                    what: "The stored root password".to_string(),
                    path: None,
                    headline: false,
                },
                KeptItem {
                    what: "This instance's my.cnf".to_string(),
                    path: Some("/tmp/ovh/config/generated/mysql/8.4/my.cnf".to_string()),
                    headline: false,
                },
                KeptItem {
                    what: "Your own MySQL overrides".to_string(),
                    path: Some("/tmp/ovh/config/custom/mysql/8.4/conf.d".to_string()),
                    headline: false,
                },
            ]
        );
    }

    // ---- the headline kept item (fix R4) ---------------------------------

    #[test]
    fn exactly_one_kept_item_is_the_headline_for_every_kind() {
        // The invariant the UI codes against: `keeps.find(k => k.headline)`
        // must find exactly one thing, whatever a future kind adds. Checked
        // against the list itself rather than a pinned vector, so this cannot
        // be "fixed" by updating an expectation.
        //
        // VACUITY: changing either `kept_headline` call to `kept` makes the
        // count 0 and this fails; adding a second `kept_headline` makes it 2
        // and it fails the other way.
        for target in [php("8.1"), php("8.4"), mysql("8.4")] {
            let keeps = inventory(&target, Path::new("/tmp/ovh"), None).keeps;
            let headlines: Vec<KeptItem> = keeps.into_iter().filter(|k| k.headline).collect();
            assert_eq!(
                headlines.len(),
                1,
                "{} must have exactly one headline kept item, got {headlines:?}",
                target.display()
            );
        }
    }

    #[test]
    fn the_headline_is_the_item_the_confirmations_sentence_names() {
        // A count alone would pass with the flag on the wrong row. The MySQL
        // sentence is "Your databases are not touched — they stay in <path>",
        // so the flagged item must be the DATADIR: `my.cnf` also carries a
        // path, and naming it there would tell a user their databases live in
        // a config file. The PHP sentence is about the logs.
        let mysql_headline = inventory(&mysql("8.4"), Path::new("/tmp/ovh"), None)
            .keeps
            .into_iter()
            .find(|k| k.headline)
            .expect("a headline");
        assert_eq!(mysql_headline.what, "Your databases");
        assert_eq!(
            mysql_headline.path.as_deref(),
            Some("/tmp/ovh/data/mysql/8.4")
        );

        let php_headline = inventory(&php("8.4"), Path::new("/tmp/ovh"), None)
            .keeps
            .into_iter()
            .find(|k| k.headline)
            .expect("a headline");
        assert_eq!(php_headline.what, "Your PHP 8.4 logs");
        assert_eq!(
            php_headline.path.as_deref(),
            Some("/tmp/ovh/logs/services/php-fpm-8.4")
        );
    }

    #[test]
    fn the_headline_survives_reaching_the_wire() {
        // The flag exists for a UI in another language; a serde attribute typo
        // would only show up as a dialog quietly picking the wrong path again.
        let plan = build_plan(
            &mysql("8.4"),
            Path::new("/tmp/ovh"),
            &[],
            &[],
            Some(&own_keg()),
            None,
        );
        let v = serde_json::to_value(&plan).unwrap();
        assert_eq!(v["keeps"][0]["headline"], true);
        assert_eq!(v["keeps"][1]["headline"], false);
        assert_eq!(v["keeps"][0]["what"], "Your databases");
    }

    #[test]
    fn no_removal_of_any_kind_ever_names_a_data_or_log_path() {
        // The blanket invariant, checked against the removal list itself
        // rather than against a particular arm: whatever a future kind adds,
        // it may not put the user's data or logs on the chopping block.
        let home = Path::new("/tmp/ovh");
        for target in [php("8.1"), php("8.4"), mysql("8.4")] {
            for removal in inventory(&target, home, None).removes {
                if let Removal::GeneratedFile { path, .. } = removal {
                    let p = path.display().to_string();
                    assert!(
                        p.starts_with("/tmp/ovh/config/generated/"),
                        "an uninstall may only delete generated config, got {p}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_brew_line_warns_that_dependencies_may_go_too() {
        // Observed in the live proof: `brew uninstall php@8.3` also removed
        // `aspell` (768 files, 338 MB), and `brew uninstall mysql@8.4` also
        // removed `abseil`, `protobuf` and `zlib-ng-compat`. D6 promises the
        // confirmation states what is removed; naming one formula while brew
        // quietly takes four does not.
        //
        // Asserted on the PLAN (what the dialog renders), not on `describe`
        // directly, so deleting the sentence from either end fails here.
        //
        // VACUITY: removing the second sentence from `Removal::BrewFormula`'s
        // arm makes both assertions fail for both kinds.
        for target in [php("8.4"), mysql("8.4")] {
            let plan = build_plan(
                &target,
                Path::new("/tmp/ovh"),
                &[],
                &[],
                Some(&own_keg()),
                None,
            );
            let brew_line = plan.removes.first().expect("the formula is removal #1");
            assert!(
                brew_line.contains("may also remove dependencies"),
                "got {brew_line:?}"
            );
            // And it must point at where the names will appear, rather than
            // leaving the user to guess which ones.
            assert!(
                brew_line.contains("output names any it takes"),
                "got {brew_line:?}"
            );
        }
    }

    #[test]
    fn the_displayed_formula_is_the_one_that_reaches_brews_argv() {
        // Two readers of one fact: the dialog's `removes` line and the child
        // process's argv. Pinned against the REAL spec builder so they cannot
        // drift into naming different formulas.
        let brew = Path::new("/opt/homebrew/bin/brew");
        for target in [php("8.4"), mysql("8.4")] {
            let inv = inventory(&target, Path::new("/tmp/ovh"), None);
            let Some(Removal::BrewFormula { formula, .. }) = inv.removes.first() else {
                panic!("the formula must be the FIRST removal: {:?}", inv.removes);
            };
            let spec = target.uninstall_spec(brew).unwrap();
            let args: Vec<String> = spec
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(args, vec!["uninstall".to_string(), formula.clone()]);
        }
    }

    // ---- blockers, over every ServiceState -------------------------------
    //
    // VACUITY (RED first): written before `service_blocker` existed. Neutered
    // afterwards by making the `Starting` arm return `None` — both
    // `every_non_terminal_state_blocks...` and
    // `a_starting_service_blocks_...` failed; restoring it made them pass.

    #[test]
    fn only_a_non_terminal_state_blocks_and_it_is_named() {
        for state in every_state() {
            let blocker = service_blocker("php-fpm-8.4", &state);
            match &state {
                ServiceState::Stopped | ServiceState::Failed { .. } => {
                    assert!(blocker.is_none(), "{state:?} must not block an uninstall");
                }
                ServiceState::Starting | ServiceState::Running => {
                    let Some(Blocker::ServiceNotTerminal { id, state: label }) = blocker else {
                        panic!("{state:?} must block, got {blocker:?}");
                    };
                    assert_eq!(id, "php-fpm-8.4");
                    assert_eq!(label, crate::control::state_label(&state));
                }
            }
        }
    }

    #[test]
    fn agrees_with_the_supervisors_own_terminal_check() {
        // Two independent exhaustive matches decide the same question: this
        // module's pre-flight blocker, and `openvhost_proc`'s private
        // `check_terminal` behind `Supervisor::unregister`. If they ever
        // disagreed, the pre-flight would wave through an uninstall that then
        // ran `brew uninstall` and could not remove the row afterwards. Drive
        // the REAL supervisor to answer the second half.
        use openvhost_proc::{ServiceSpec, SpawnSpec, Supervisor, default_driver};

        for state in every_state() {
            let sup = Supervisor::new(default_driver());
            sup.register(ServiceSpec {
                id: "svc".into(),
                display_name: "svc".into(),
                endpoint: None,
                spawn: SpawnSpec {
                    program: PathBuf::from("/nonexistent"),
                    args: vec![],
                    cwd: None,
                    env: vec![],
                },
                readiness: openvhost_proc::ReadinessProbe::default(),
                grace: openvhost_proc::DEFAULT_GRACE,
            });
            // Only the terminal states are reachable on a freshly registered
            // row without spawning anything, and those are exactly the ones
            // where a disagreement would be dangerous (blocker says "fine",
            // unregister says "no"). `Starting`/`Running` are covered by
            // `unregister`'s own tests in openvhost-proc.
            if matches!(state, ServiceState::Starting | ServiceState::Running) {
                continue;
            }
            let blocked = service_blocker("svc", &state).is_some();
            let refused = sup.unregister("svc").is_err();
            assert_eq!(
                blocked, refused,
                "{state:?}: pre-flight blocked={blocked} but unregister refused={refused}"
            );
        }
    }

    // ---- the aliased-keg refusal (fix R1) --------------------------------
    //
    // VACUITY (neuter-and-watch-it-fail): deleting the `keg_blocker` push from
    // `blockers` makes `an_aliased_php_major_is_refused_...` and
    // `an_unresolvable_keg_is_refused_...` both report an empty blocker list.
    // Making `keg_blocker`'s `Unresolved` arm return `None` fails the second
    // alone, which is what keeps the two states from collapsing back together.

    #[test]
    fn an_aliased_php_major_is_refused_and_the_refusal_names_what_would_go() {
        // This machine's actual shape: `php@8.5` is one of brew's aliases for
        // the unversioned `php`, so `brew uninstall php@8.5` removes the user's
        // linked PHP.
        let keg = KegProvenance::ForeignKeg {
            owner: "php".to_string(),
            keg: PathBuf::from("/opt/homebrew/Cellar/php/8.5.9"),
        };
        assert_eq!(
            blockers(&php("8.5"), &[], &[], Some(&keg), None),
            vec![Blocker::ForeignKeg {
                formula: "php@8.5".to_string(),
                owner: "php".to_string(),
                keg: "/opt/homebrew/Cellar/php/8.5.9".to_string(),
            }]
        );
        // The prose has to name BOTH the formula the user would have clicked
        // and the thing that would actually be removed — a refusal that says
        // only "can't do that" teaches nothing.
        let text = blockers(&php("8.5"), &[], &[], Some(&keg), None)[0].describe();
        assert!(text.contains("php@8.5"), "{text}");
        assert!(text.contains("/opt/homebrew/Cellar/php/8.5.9"), "{text}");
        assert!(text.contains("brew uninstall php"), "{text}");
    }

    #[test]
    fn the_same_refusal_covers_mysql() {
        // Homebrew applies the identical aliasing rule to every
        // versioned-formula family, and one code path serves both — so this is
        // the same branch, not a dead one.
        let keg = KegProvenance::ForeignKeg {
            owner: "mysql".to_string(),
            keg: PathBuf::from("/opt/homebrew/Cellar/mysql/8.4.11"),
        };
        assert_eq!(
            blockers(&mysql("8.4"), &[], &[], Some(&keg), None),
            vec![Blocker::ForeignKeg {
                formula: "mysql@8.4".to_string(),
                owner: "mysql".to_string(),
                keg: "/opt/homebrew/Cellar/mysql/8.4.11".to_string(),
            }]
        );
    }

    #[test]
    fn an_unresolvable_keg_is_refused_rather_than_assumed_safe() {
        // "I could not tell" is not "it is fine". brew resolves its own aliases
        // from its taps with or without an `opt` link, so the alias danger is
        // fully present here — just unprovable.
        let keg = KegProvenance::Unresolved {
            searched: vec![
                PathBuf::from("/opt/homebrew/opt/php@8.4"),
                PathBuf::from("/usr/local/opt/php@8.4"),
            ],
        };
        assert_eq!(
            blockers(&php("8.4"), &[], &[], Some(&keg), None),
            vec![Blocker::UnknownKeg {
                formula: "php@8.4".to_string(),
                searched: vec![
                    "/opt/homebrew/opt/php@8.4".to_string(),
                    "/usr/local/opt/php@8.4".to_string(),
                ],
            }]
        );
    }

    #[test]
    fn a_formula_that_owns_its_keg_is_not_blocked_by_the_keg_check() {
        // The other side of the refusal — without this the check would be
        // indistinguishable from "uninstall never works".
        assert!(keg_blocker(&php("8.4"), &own_keg(), None).is_none());
        assert!(blockers(&php("8.4"), &[], &[], Some(&own_keg()), None).is_empty());
    }

    #[test]
    fn the_keg_refusal_is_reported_first_and_alongside_the_others() {
        // Categorically different from the other two: those say "do this, then
        // retry", this one says OpenVHost will never do it. It must be the
        // first thing read — and it must not SUPPRESS the others, or a user
        // clears one obstacle at a time.
        let services = vec![status("php-fpm-8.5", ServiceState::Running)];
        let sites = vec![site("shop", "shop.localhost", "8.5")];
        let keg = KegProvenance::ForeignKeg {
            owner: "php".to_string(),
            keg: PathBuf::from("/opt/homebrew/Cellar/php/8.5.9"),
        };
        let found = blockers(&php("8.5"), &services, &sites, Some(&keg), None);
        assert_eq!(found.len(), 3, "got {found:?}");
        assert!(matches!(found[0], Blocker::ForeignKeg { .. }));
        assert!(matches!(found[1], Blocker::ServiceNotTerminal { .. }));
        assert!(matches!(found[2], Blocker::SitesPinned { .. }));
    }

    #[test]
    fn the_refused_formula_is_the_one_that_would_have_reached_brews_argv() {
        // The whole bug was a mismatch between the string shown and the keg
        // removed. Pin the refusal's `formula` against the REAL spec builder so
        // a refusal can never name something other than what it prevented.
        for target in [php("8.4"), mysql("8.4")] {
            let keg = KegProvenance::ForeignKeg {
                owner: "other".to_string(),
                keg: PathBuf::from("/opt/homebrew/Cellar/other/1.0"),
            };
            let Some(Blocker::ForeignKeg { formula, .. }) = keg_blocker(&target, &keg, None) else {
                panic!("expected a ForeignKeg refusal");
            };
            let spec = target
                .uninstall_spec(Path::new("/opt/homebrew/bin/brew"))
                .unwrap();
            assert_eq!(spec.args[1].to_string_lossy(), formula);
        }
    }

    #[test]
    fn the_new_refusals_reach_the_wire_as_tagged_unions() {
        let v = serde_json::to_value(Blocker::ForeignKeg {
            formula: "php@8.5".into(),
            owner: "php".into(),
            keg: "/opt/homebrew/Cellar/php/8.5.9".into(),
        })
        .unwrap();
        assert_eq!(v["kind"], "foreignKeg");
        assert_eq!(v["formula"], "php@8.5");
        assert_eq!(v["owner"], "php");
        assert_eq!(v["keg"], "/opt/homebrew/Cellar/php/8.5.9");

        let v = serde_json::to_value(Blocker::UnknownKeg {
            formula: "php@8.4".into(),
            searched: vec!["/opt/homebrew/opt/php@8.4".into()],
        })
        .unwrap();
        assert_eq!(v["kind"], "unknownKeg");
        assert_eq!(v["searched"][0], "/opt/homebrew/opt/php@8.4");
    }

    #[test]
    fn a_php_major_a_site_still_uses_is_refused_and_the_site_is_named() {
        let sites = vec![
            site("shop", "shop.localhost", "8.4"),
            site("blog", "blog.localhost", "8.1"),
            site("wiki", "wiki.localhost", "8.4"),
        ];
        let found = blockers(&php("8.4"), &[], &sites, Some(&own_keg()), None);
        assert_eq!(
            found,
            vec![Blocker::SitesPinned {
                domains: vec!["shop.localhost".to_string(), "wiki.localhost".to_string()],
            }]
        );
    }

    #[test]
    fn a_php_major_no_site_uses_has_no_site_blocker() {
        let sites = vec![site("blog", "blog.localhost", "8.1")];
        assert!(blockers(&php("8.4"), &[], &sites, Some(&own_keg()), None).is_empty());
    }

    #[test]
    fn a_disabled_site_still_pins_its_php_version() {
        // A disabled site is still a site whose configuration names 8.4, and
        // re-enabling it after the uninstall would fail to apply. Naming it
        // now is the recoverable outcome.
        let mut s = site("shop", "shop.localhost", "8.4");
        s.enabled = false;
        assert_eq!(
            blockers(&php("8.4"), &[], &[s], Some(&own_keg()), None),
            vec![Blocker::SitesPinned {
                domains: vec!["shop.localhost".to_string()],
            }]
        );
    }

    #[test]
    fn a_mysql_uninstall_is_never_blocked_by_sites() {
        // Sites do not reference a MySQL major, and the things that DO — the
        // datadir and the credential row — are kept, not obstacles. A future
        // edit that "helpfully" blocked on an initialized datadir would break
        // D2's whole round trip, so pin it.
        let sites = vec![site("shop", "shop.localhost", "8.4")];
        assert!(blockers(&mysql("8.4"), &[], &sites, Some(&own_keg()), None).is_empty());
    }

    #[test]
    fn a_running_service_and_pinned_sites_are_both_reported() {
        // Not "the first obstacle": a user who stops the service only to be
        // told about the sites has been made to guess twice.
        let services = vec![status("php-fpm-8.4", ServiceState::Running)];
        let sites = vec![site("shop", "shop.localhost", "8.4")];
        let found = blockers(&php("8.4"), &services, &sites, Some(&own_keg()), None);
        assert_eq!(found.len(), 2, "got {found:?}");
        assert!(matches!(found[0], Blocker::ServiceNotTerminal { .. }));
        assert!(matches!(found[1], Blocker::SitesPinned { .. }));
    }

    #[test]
    fn another_versions_running_service_does_not_block_this_one() {
        // Over-matching here would make every uninstall refuse whenever any
        // pool was up.
        let services = vec![
            status("php-fpm-8.1", ServiceState::Running),
            status("nginx", ServiceState::Running),
            status("mysql-8.4", ServiceState::Running),
        ];
        assert!(blockers(&php("8.4"), &services, &[], Some(&own_keg()), None).is_empty());
    }

    #[test]
    fn a_plan_with_no_blockers_may_proceed_and_lists_both_halves() {
        let plan = build_plan(
            &mysql("8.4"),
            Path::new("/tmp/ovh"),
            &[],
            &[],
            Some(&own_keg()),
            None,
        );
        assert!(plan.blockers.is_empty());
        assert_eq!(plan.kind, PackageKind::Mysql);
        assert_eq!(plan.major, "8.4");
        assert_eq!(plan.removes.len(), 2);
        assert!(
            plan.keeps.iter().any(|k| k.what == "Your databases"),
            "the dialog must be able to say the databases stay: {:?}",
            plan.keeps
        );
    }

    #[test]
    fn the_plans_removes_are_the_inventorys_removes_in_order() {
        // The dialog and the executor read one list. Pinned so a future edit
        // cannot re-order or filter one side only.
        let target = php("8.4");
        let home = Path::new("/tmp/ovh");
        let plan = build_plan(&target, home, &[], &[], Some(&own_keg()), None);
        let expected: Vec<String> = inventory(&target, home, None)
            .removes
            .iter()
            .map(Removal::describe)
            .collect();
        assert_eq!(plan.removes, expected);
    }

    #[test]
    fn an_out_of_catalogue_major_is_rejected_before_anything_is_named() {
        // The formula reaching brew can be influenced by nothing but a
        // catalogue major, so a hand-installed 7.4 cannot be uninstalled from
        // the app at all — it is refused here, at parse.
        for bad in ["7.4", "9.9", "--build-from-source", "8", ""] {
            assert!(
                Target::parse(PackageKind::Php, bad).is_err(),
                "accepted {bad:?}"
            );
        }
        for bad in ["8.0", "9.7", "--cask", "8.4.1"] {
            assert!(
                Target::parse(PackageKind::Mysql, bad).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn a_rejected_version_names_the_field_so_the_ui_can_mark_it() {
        match Target::parse(PackageKind::Php, "7.4").unwrap_err() {
            IpcError::Validation { field, .. } => assert_eq!(field, "php_version"),
            other => panic!("expected Validation, got {other:?}"),
        }
        match Target::parse(PackageKind::Mysql, "8.0").unwrap_err() {
            IpcError::Validation { field, .. } => assert_eq!(field, "mysql_version"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn the_wire_shapes_are_the_tagged_unions_the_ui_codes_against() {
        // Task 3 is written against these exact shapes; a serde attribute
        // typo would only show up as a silently-unrendered dialog.
        let v = serde_json::to_value(Blocker::ServiceNotTerminal {
            id: "php-fpm-8.4".into(),
            state: "running".into(),
        })
        .unwrap();
        assert_eq!(v["kind"], "serviceNotTerminal");
        assert_eq!(v["id"], "php-fpm-8.4");
        let v = serde_json::to_value(Blocker::SitesPinned {
            domains: vec!["a.localhost".into()],
        })
        .unwrap();
        assert_eq!(v["kind"], "sitesPinned");
        assert_eq!(v["domains"][0], "a.localhost");
        let v = serde_json::to_value(build_plan(
            &php("8.4"),
            Path::new("/tmp/ovh"),
            &[],
            &[],
            Some(&own_keg()),
            None,
        ))
        .unwrap();
        assert_eq!(v["kind"], "php");
        assert_eq!(v["major"], "8.4");
        assert!(v["keeps"][0]["what"].is_string());
        // `kind` is an INPUT too — the command takes it, so it must round
        // trip, not merely serialize.
        assert_eq!(
            serde_json::from_value::<PackageKind>(serde_json::json!("mysql")).unwrap(),
            PackageKind::Mysql
        );
    }

    // ---- MariaDB: no Homebrew formula, admitted rather than fabricated
    // (P1 MariaDB UI design D5) -------------------------------------------

    #[test]
    fn mariadb_has_no_homebrew_formula_and_php_mysql_still_do() {
        assert_eq!(mariadb().formula(None), None);
        assert_eq!(php("8.4").formula(None), Some("php@8.4".to_string()));
        assert_eq!(mysql("8.4").formula(None), Some("mysql@8.4".to_string()));
    }

    #[test]
    fn mariadb_has_no_keg_to_resolve() {
        assert_eq!(mariadb().keg_provenance(None), None);
    }

    /// VACUITY (neuter-and-watch-it-fail): temporarily made this arm
    /// delegate to `openvhost_core::mysql_brew_uninstall_spec` with a
    /// hand-built `MysqlMajor` (as if MariaDB were just another versioned
    /// MySQL-shaped formula) — this test failed because a spec WAS produced
    /// instead of an error, and its argv named `mysql@…`, not any MariaDB
    /// identity. Restoring the explicit refusal arm made it pass again.
    #[test]
    fn mariadb_uninstall_spec_refuses_rather_than_fabricating_a_formula() {
        let err = mariadb()
            .uninstall_spec(Path::new("/opt/homebrew/bin/brew"))
            .unwrap_err();
        match err {
            IpcError::Core { message } => assert!(
                message.contains("Homebrew"),
                "refusal should name the reason: {message}"
            ),
            other => panic!("expected IpcError::Core, got {other:?}"),
        }
    }

    /// `keg_blocker` itself stays graceful (not a panic) for a formula-less
    /// target reached some other way than through `blockers` — see its own
    /// doc comment for why this is a documented fallback rather than the
    /// production path.
    #[test]
    fn keg_blocker_admits_absence_for_a_formula_less_target_rather_than_fabricating_one() {
        assert_eq!(keg_blocker(&mariadb(), &own_keg(), None), None);
    }

    #[test]
    fn mariadb_parse_accepts_only_the_pinned_series() {
        assert_eq!(
            Target::parse(PackageKind::Mariadb, openvhost_core::MARIADB_SERIES).unwrap(),
            Target::Mariadb
        );
        for bad in ["11.5", "10.4", "", "11.4.9"] {
            let err = Target::parse(PackageKind::Mariadb, bad).unwrap_err();
            match err {
                IpcError::Validation { field, .. } => assert_eq!(field, "mariadb_version"),
                other => panic!("expected Validation, got {other:?} for {bad:?}"),
            }
        }
    }

    /// The removal shape is `PackageTree` (never `BrewFormula`, D5) plus the
    /// service row, and the keeps mirror MySQL's exactly — same headline
    /// promise about the datadir, same stored-password and my.cnf entries.
    ///
    /// VACUITY: written before `Target::Mariadb`'s `inventory` arm existed —
    /// it did not compile, then failed on every field once the arm existed
    /// but still matched PHP's `BrewFormula` shape by copy-paste.
    #[test]
    fn a_mariadb_uninstall_removes_the_package_tree_and_the_row_and_keeps_the_data() {
        let home = Path::new("/tmp/ovh");
        let inv = inventory(&mariadb(), home, None);
        assert_eq!(
            inv.removes,
            vec![
                Removal::PackageTree {
                    path: PathBuf::from("/tmp/ovh/packages/mariadb/11.4"),
                    what: "The MariaDB 11.4 program files".to_string(),
                },
                Removal::ServiceRow {
                    id: "mariadb-11.4".to_string(),
                },
            ]
        );
        assert_eq!(
            inv.keeps,
            vec![
                KeptItem {
                    what: "Your databases".to_string(),
                    path: Some("/tmp/ovh/data/mariadb/11.4".to_string()),
                    headline: true,
                },
                KeptItem {
                    what: "The stored root password".to_string(),
                    path: None,
                    headline: false,
                },
                KeptItem {
                    what: "This instance's my.cnf".to_string(),
                    path: Some("/tmp/ovh/config/generated/mariadb/11.4/my.cnf".to_string()),
                    headline: false,
                },
                KeptItem {
                    what: "Your own MariaDB overrides".to_string(),
                    path: Some("/tmp/ovh/config/custom/mariadb/11.4/conf.d".to_string()),
                    headline: false,
                },
            ]
        );
    }

    #[test]
    fn a_mariadb_uninstall_has_exactly_one_headline_kept_item_naming_the_datadir() {
        let keeps = inventory(&mariadb(), Path::new("/tmp/ovh"), None).keeps;
        let headlines: Vec<KeptItem> = keeps.into_iter().filter(|k| k.headline).collect();
        assert_eq!(headlines.len(), 1, "got {headlines:?}");
        assert_eq!(headlines[0].what, "Your databases");
    }

    /// No formula, no keg check, and nothing in state.db pins a site to
    /// MariaDB — `blockers` must be empty regardless of services or sites,
    /// mirroring `a_mysql_uninstall_is_never_blocked_by_sites`'s reasoning
    /// extended to the keg check too (`keg` is `None` here, never `Some`).
    #[test]
    fn a_mariadb_uninstall_is_never_blocked_by_sites_or_the_keg_check() {
        let sites = vec![site("shop", "shop.localhost", "8.4")];
        assert!(blockers(&mariadb(), &[], &sites, None, None).is_empty());
    }

    /// The one blocker MariaDB CAN still have: its own service still
    /// running. Proves `blockers`'s service check applies uniformly across
    /// every `Target`, not only the formula-having ones.
    #[test]
    fn a_running_mariadb_service_still_blocks_its_own_uninstall() {
        let services = vec![status("mariadb-11.4", ServiceState::Running)];
        let found = blockers(&mariadb(), &services, &[], None, None);
        assert_eq!(
            found,
            vec![Blocker::ServiceNotTerminal {
                id: "mariadb-11.4".to_string(),
                state: crate::control::state_label(&ServiceState::Running).to_string(),
            }]
        );
    }

    #[test]
    fn a_mariadb_plan_with_no_blockers_may_proceed_and_lists_both_halves() {
        let plan = build_plan(&mariadb(), Path::new("/tmp/ovh"), &[], &[], None, None);
        assert!(plan.blockers.is_empty());
        assert_eq!(plan.kind, PackageKind::Mariadb);
        assert_eq!(plan.major, "11.4");
        assert_eq!(plan.removes.len(), 2);
        assert!(
            plan.keeps.iter().any(|k| k.what == "Your databases"),
            "the dialog must be able to say the databases stay: {:?}",
            plan.keeps
        );
    }

    #[test]
    fn the_mariadb_kind_reaches_the_wire_as_its_own_tag() {
        let v = serde_json::to_value(PackageKind::Mariadb).unwrap();
        assert_eq!(v, "mariadb");
        assert_eq!(
            serde_json::from_value::<PackageKind>(serde_json::json!("mariadb")).unwrap(),
            PackageKind::Mariadb
        );
    }

    // ======================================================================
    // Off-Homebrew slice 5D — a PHP major THIS app packaged.
    // ======================================================================

    /// Every `(target, packaged)` combination this module can be asked about.
    ///
    /// The blanket invariants below are checked over the whole space rather
    /// than over the case that came to mind — `Target` has three variants and
    /// PHP now has three shapes (Homebrew's, ours alone, ours beside a keg),
    /// and it is the combinations that a per-arm test misses.
    fn every_shape() -> Vec<(Target, Option<PackagedPhp>)> {
        vec![
            (php("8.1"), None),
            (php("8.4"), None),
            (php("8.4"), Some(packaged_only("8.4", "8.4.24"))),
            (
                php("8.4"),
                Some(packaged_at(
                    "/tmp/ovh",
                    "8.4",
                    "8.4.24",
                    Some("/opt/homebrew/Cellar/php@8.4/8.4.13"),
                )),
            ),
            (mysql("8.4"), None),
            (mariadb(), None),
        ]
    }

    // ---- PURITY: `inventory` stats nothing (design D1) -------------------
    //
    // VACUITY: `inventory_stats_nothing_...` fails the moment `inventory`
    // resolves anything itself — replacing its `packaged` parameter with an
    // inline `target.packaged(home)` makes the second call name 8.4.99 and the
    // assertion reports two different `Removal::PackageTree` paths. Measured;
    // see the task report.

    /// ESTABLISHED rather than asserted: the disk really moves between the two
    /// calls, in three ways that would each change the answer of a function
    /// that looked, and the value does not move with it.
    ///
    /// For a destructive operation this is a TOCTOU defence, not a style
    /// preference: if the plan were recomputed against the disk at execution
    /// time, the dialog could say it removes 8.4.24 while the executor removes
    /// 8.4.99.
    #[cfg(unix)]
    #[test]
    fn inventory_stats_nothing_so_the_disk_can_move_under_a_plan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        install_packaged_php(home, "8.4", "8.4.24");
        point_current(home, "8.4", "8.4.24");
        provision_php_paths(home, "8.4");

        // Resolved the way production resolves it, so what follows is about a
        // real value and not a hand-built one.
        let packaged = php("8.4").packaged(home).expect("a packaged 8.4");
        let before = inventory(&php("8.4"), home, Some(&packaged));

        // Three independent ways to change the answer of a function that
        // looked at the disk:
        // 1. a second version installed and `current` swung onto it;
        install_packaged_php(home, "8.4", "8.4.99");
        point_current(home, "8.4", "8.4.99");
        // 2. the version directory the plan names, deleted;
        std::fs::remove_dir_all(&packaged.version_dir).expect("remove version dir");
        // 3. every other path the plan names, deleted.
        std::fs::remove_dir_all(home.join("config")).expect("remove config");
        std::fs::remove_dir_all(home.join("logs")).expect("remove logs");

        // The fixture is discriminating: re-resolving NOW names a different
        // directory, so a version of `inventory` that consulted the disk would
        // have answered differently. Without this the test could pass against a
        // mutation that changed nothing.
        let re_resolved = php("8.4").packaged(home).expect("8.4.99 is packaged now");
        assert_ne!(
            re_resolved.version_dir, packaged.version_dir,
            "the disk did not actually move; this test would prove nothing"
        );

        assert_eq!(
            inventory(&php("8.4"), home, Some(&packaged)),
            before,
            "inventory must be a function of its arguments, not of the disk"
        );
    }

    /// The other half of the invariant: the value a dialog renders and the list
    /// the executor walks are ONE value, for a packaged target too — the
    /// packaged twin of `the_plans_removes_are_the_inventorys_removes_in_order`.
    #[test]
    fn a_packaged_plan_renders_exactly_the_list_the_executor_walks() {
        let target = php("8.4");
        let home = Path::new("/tmp/ovh");
        let packaged = packaged_at(
            "/tmp/ovh",
            "8.4",
            "8.4.24",
            Some("/opt/homebrew/Cellar/php@8.4/8.4.13"),
        );
        let plan = build_plan(&target, home, &[], &[], None, Some(&packaged));
        let inv = inventory(&target, home, Some(&packaged));
        assert_eq!(
            plan.removes,
            inv.removes
                .iter()
                .map(Removal::describe)
                .collect::<Vec<_>>()
        );
        // The keeps travel whole, D3's new entry included — a dialog that
        // dropped it would promise nothing about the keg it walks past.
        assert_eq!(plan.keeps, inv.keeps);
    }

    // ---- A BREW-ONLY MAJOR IS UNCHANGED ----------------------------------

    /// The whole value, both halves at once, pinned against what this function
    /// returned before slice 5D existed.
    ///
    /// MEASURED, not assumed: `inventory` was dumped with `{:#?}` for PHP 8.1,
    /// 8.4 and 8.5, MySQL 8.4 and MariaDB on `c4b0732` (the commit this branch
    /// is based on) and again on this branch with `packaged: None`. The two
    /// dumps were byte-for-byte identical, 6046 bytes each, including every
    /// `Removal::describe` string. This test is the part of that measurement
    /// that keeps running.
    ///
    /// It pins the value TOGETHER rather than `removes` and `keeps` separately
    /// (which the tests above already do): the failure it is here to catch is
    /// an entry appearing somewhere, and a per-half assertion cannot see one
    /// added to the other half.
    #[test]
    fn a_brew_only_php_majors_inventory_is_exactly_what_it_was_before_this_slice() {
        assert_eq!(
            inventory(&php("8.4"), Path::new("/tmp/ovh"), None),
            Inventory {
                removes: vec![
                    Removal::BrewFormula {
                        formula: "php@8.4".to_string(),
                        what: "The PHP 8.4 program files".to_string(),
                    },
                    Removal::GeneratedFile {
                        path: PathBuf::from("/tmp/ovh/config/generated/php/8.4/php-fpm.conf"),
                        what: "The generated php-fpm pool config".to_string(),
                    },
                    Removal::ServiceRow {
                        id: "php-fpm-8.4".to_string(),
                    },
                ],
                keeps: vec![
                    KeptItem {
                        what: "Your PHP 8.4 logs".to_string(),
                        path: Some("/tmp/ovh/logs/services/php-fpm-8.4".to_string()),
                        headline: true,
                    },
                    KeptItem {
                        what: "Your own php-fpm pool overrides".to_string(),
                        path: Some("/tmp/ovh/config/custom/php/8.4/pool.d".to_string()),
                        headline: false,
                    },
                    KeptItem {
                        what: "Every site's saved PHP version — a site set to 8.4 keeps it"
                            .to_string(),
                        path: None,
                        headline: false,
                    },
                ],
            }
        );
    }

    /// Spec §8.8 — nothing changes on a machine with no package tree, which is
    /// every real machine today. Driven through the REAL resolver rather than a
    /// hand-passed `None`, so it is the production path that is pinned.
    #[cfg(unix)]
    #[test]
    fn a_machine_with_no_package_tree_gets_the_homebrew_plan_it_always_got() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let packaged = php("8.4").packaged(home);
        assert_eq!(packaged, None, "an empty home has no packaged PHP");
        assert_eq!(
            inventory(&php("8.4"), Path::new("/tmp/ovh"), packaged.as_ref()),
            inventory(&php("8.4"), Path::new("/tmp/ovh"), None)
        );
        assert_eq!(
            php("8.4").formula(packaged.as_ref()),
            Some("php@8.4".to_string())
        );
    }

    // ---- A PACKAGED MAJOR PLANS NO BREW STEP (design D2) -----------------

    #[test]
    fn a_packaged_php_major_is_removed_as_a_package_tree_with_no_brew_step() {
        let packaged = packaged_only("8.4", "8.4.24");
        let inv = inventory(&php("8.4"), Path::new("/tmp/ovh"), Some(&packaged));
        assert_eq!(
            inv.removes,
            vec![
                Removal::PackageTree {
                    path: PathBuf::from("/tmp/ovh/packages/php/8.4/8.4.24"),
                    what: "The PHP 8.4 program files".to_string(),
                },
                // Unchanged from the Homebrew arm, and that is the claim:
                // neither of these was ever Homebrew's to provide.
                Removal::GeneratedFile {
                    path: PathBuf::from("/tmp/ovh/config/generated/php/8.4/php-fpm.conf"),
                    what: "The generated php-fpm pool config".to_string(),
                },
                Removal::ServiceRow {
                    id: "php-fpm-8.4".to_string(),
                },
            ]
        );
    }

    #[test]
    fn a_packaged_php_major_names_no_homebrew_formula_and_looks_up_no_keg() {
        let packaged = packaged_only("8.4", "8.4.24");
        assert_eq!(php("8.4").formula(Some(&packaged)), None);
        assert_eq!(php("8.4").keg_provenance(Some(&packaged)), None);
        // The same target without a packaged install is untouched — the seam
        // is the packaged state, not the major.
        assert_eq!(php("8.4").formula(None), Some("php@8.4".to_string()));
    }

    /// The consequence that matters most in practice. On the machine this
    /// project measured, `php@8.5` is one of Homebrew's aliases for the
    /// unversioned `php`, and a Homebrew 8.5 uninstall is REFUSED for it (see
    /// `an_aliased_php_major_is_refused_and_the_refusal_names_what_would_go`).
    /// A packaged 8.5 spawns no brew at all, so the same machine state must not
    /// refuse it — otherwise the only PHP a user could remove is the one this
    /// app did not install.
    #[test]
    fn a_packaged_major_is_not_refused_by_homebrews_alias_trap() {
        let keg = KegProvenance::ForeignKeg {
            owner: "php".to_string(),
            keg: PathBuf::from("/opt/homebrew/Cellar/php/8.5.9"),
        };
        let packaged = packaged_only("8.5", "8.5.9");
        assert_eq!(keg_blocker(&php("8.5"), &keg, Some(&packaged)), None);
        assert!(blockers(&php("8.5"), &[], &[], Some(&keg), Some(&packaged)).is_empty());
        // And the refusal did not go away — it stopped applying to a target it
        // was never about. Without this the test above is indistinguishable
        // from having deleted the check.
        assert!(!blockers(&php("8.5"), &[], &[], Some(&keg), None).is_empty());
    }

    /// The blockers that are NOT about Homebrew still apply. A packaged
    /// uninstall is not a privileged one.
    #[test]
    fn a_packaged_major_is_still_blocked_by_its_own_service_and_by_pinned_sites() {
        let packaged = packaged_only("8.4", "8.4.24");
        let services = vec![status("php-fpm-8.4", ServiceState::Running)];
        let sites = vec![site("shop", "shop.localhost", "8.4")];
        let found = blockers(&php("8.4"), &services, &sites, None, Some(&packaged));
        assert_eq!(found.len(), 2, "got {found:?}");
        assert!(matches!(found[0], Blocker::ServiceNotTerminal { .. }));
        assert!(matches!(found[1], Blocker::SitesPinned { .. }));
    }

    /// The seam D2 exists to hold, checked over every shape rather than per
    /// arm: `uninstall_package` decides whether Homebrew must be FOUND from
    /// `Target::formula`, and `inventory` decides whether brew is SPAWNED. If
    /// those two could disagree, a machine would either be told it needs
    /// Homebrew to delete a directory, or reach `run_brew` with a placeholder
    /// path for a `brew` that was never located.
    #[test]
    fn a_brew_step_is_planned_exactly_when_this_uninstall_names_a_formula() {
        for (target, packaged) in every_shape() {
            let p = packaged.as_ref();
            let plans_brew = inventory(&target, Path::new("/tmp/ovh"), p)
                .removes
                .iter()
                .any(|r| matches!(r, Removal::BrewFormula { .. }));
            assert_eq!(
                plans_brew,
                target.formula(p).is_some(),
                "{} (packaged: {}) plans brew={plans_brew} but names formula={:?}",
                target.display(),
                p.is_some(),
                target.formula(p)
            );
        }
    }

    /// Exactly one removal provides the program files, and never both shapes.
    /// Two would remove the version twice over; zero would report success
    /// having left the binaries in place.
    #[test]
    fn exactly_one_removal_provides_the_program_files_in_every_shape() {
        for (target, packaged) in every_shape() {
            let count = inventory(&target, Path::new("/tmp/ovh"), packaged.as_ref())
                .removes
                .iter()
                .filter(|r| matches!(r, Removal::BrewFormula { .. } | Removal::PackageTree { .. }))
                .count();
            assert_eq!(
                count,
                1,
                "{} (packaged: {}) has {count} program-files removals",
                target.display(),
                packaged.is_some()
            );
        }
    }

    /// T1's half of design D4's containment: the plan may only ever hand the
    /// executor a path inside `<home>/packages/`.
    ///
    /// Deliberately NOT claimed as traversal-proof. This is a lexical test on a
    /// value; a symlink at an intermediate component still redirects a
    /// `remove_dir_all`, and no test of the plan can see that. The executor's
    /// own canonicalising guard (task T2, spec D4) is the half that can.
    #[test]
    fn every_package_tree_removal_names_a_path_under_the_packages_root() {
        let home = Path::new("/tmp/ovh");
        for (target, packaged) in every_shape() {
            for removal in inventory(&target, home, packaged.as_ref()).removes {
                if let Removal::PackageTree { path, .. } = removal {
                    let p = path.display().to_string();
                    assert!(
                        p.starts_with("/tmp/ovh/packages/"),
                        "{} would remove {p}, which is outside the package tree",
                        target.display()
                    );
                }
            }
        }
    }

    // ---- BOTH INSTALLED (design D3) --------------------------------------

    #[test]
    fn with_both_installed_the_packaged_tree_goes_and_the_homebrew_keg_is_kept() {
        let packaged = packaged_at(
            "/tmp/ovh",
            "8.4",
            "8.4.24",
            Some("/opt/homebrew/Cellar/php@8.4/8.4.13"),
        );
        let inv = inventory(&php("8.4"), Path::new("/tmp/ovh"), Some(&packaged));

        assert_eq!(
            inv.removes.first(),
            Some(&Removal::PackageTree {
                path: PathBuf::from("/tmp/ovh/packages/php/8.4/8.4.24"),
                what: "The PHP 8.4 program files".to_string(),
            })
        );
        assert_eq!(
            inv.keeps.last(),
            Some(&KeptItem {
                what: "The Homebrew PHP 8.4 keg — untouched".to_string(),
                path: Some("/opt/homebrew/Cellar/php@8.4/8.4.13".to_string()),
                headline: false,
            })
        );
        // "Untouched" has to be true of the list the executor WALKS, not only
        // of the sentence printed beside it.
        for removal in &inv.removes {
            let line = removal.describe();
            assert!(!line.contains("Cellar"), "a removal names the keg: {line}");
            assert!(
                !matches!(removal, Removal::BrewFormula { .. }),
                "a brew uninstall would take the keg with it: {line}"
            );
        }
    }

    #[test]
    fn the_keg_keep_is_absent_when_homebrew_has_nothing_under_this_major() {
        let inv = inventory(
            &php("8.4"),
            Path::new("/tmp/ovh"),
            Some(&packaged_only("8.4", "8.4.24")),
        );
        assert!(
            !inv.keeps.iter().any(|k| k.what.contains("Homebrew")),
            "nothing to keep, so nothing to promise: {:?}",
            inv.keeps
        );
    }

    #[test]
    fn a_homebrew_row_never_lists_its_own_keg_as_kept() {
        // There the keg is what is being REMOVED. A "kept" line about it would
        // be the exact opposite of true.
        let inv = inventory(&php("8.4"), Path::new("/tmp/ovh"), None);
        assert!(
            !inv.keeps.iter().any(|k| k.what.contains("keg")),
            "{:?}",
            inv.keeps
        );
    }

    /// The headline is the sentence the confirmation is ABOUT, and adding a
    /// fourth kept item must not move it — see [`KeptItem::headline`] for the
    /// reorder bug this flag exists to prevent.
    #[test]
    fn the_headline_is_still_the_logs_in_every_packaged_shape() {
        for (target, packaged) in every_shape() {
            let keeps = inventory(&target, Path::new("/tmp/ovh"), packaged.as_ref()).keeps;
            let headlines: Vec<&KeptItem> = keeps.iter().filter(|k| k.headline).collect();
            assert_eq!(
                headlines.len(),
                1,
                "{} (packaged: {}) has {} headline items",
                target.display(),
                packaged.is_some(),
                headlines.len()
            );
            // And it is still the RIGHT item: a count alone passes with the
            // flag on the keg.
            if matches!(target, Target::Php(_)) {
                assert_eq!(
                    headlines[0].what,
                    format!("Your PHP {} logs", target.major())
                );
            }
        }
    }

    #[test]
    fn the_kept_keg_reaches_the_wire_with_its_path() {
        // The dialog is in another language; a keep whose path did not
        // serialize would render as a bare reassurance naming nowhere.
        let packaged = packaged_at(
            "/tmp/ovh",
            "8.4",
            "8.4.24",
            Some("/opt/homebrew/Cellar/php@8.4/8.4.13"),
        );
        let plan = build_plan(
            &php("8.4"),
            Path::new("/tmp/ovh"),
            &[],
            &[],
            None,
            Some(&packaged),
        );
        let v = serde_json::to_value(&plan).unwrap();
        assert_eq!(
            v["keeps"][3]["what"],
            "The Homebrew PHP 8.4 keg — untouched"
        );
        assert_eq!(v["keeps"][3]["path"], "/opt/homebrew/Cellar/php@8.4/8.4.13");
        assert_eq!(v["keeps"][3]["headline"], false);
        assert_eq!(v["keeps"][0]["headline"], true);
    }

    // ---- `Target::packaged` — the filesystem read, kept out of the plan ---

    #[cfg(unix)]
    #[test]
    fn packaged_resolves_the_concrete_version_directory_and_never_current() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        install_packaged_php(home, "8.4", "8.4.24");
        point_current(home, "8.4", "8.4.24");

        let packaged = php("8.4").packaged(home).expect("a packaged 8.4");
        assert_eq!(
            packaged.version_dir,
            home.join("packages/php/8.4/8.4.24"),
            "design D4: the concrete version directory"
        );
        assert!(
            !packaged.version_dir.ends_with("current"),
            "a path through `current` is a path whose target can move"
        );
    }

    /// "Remove what the row described" (design D3) is only a property if the
    /// uninstall's question and discovery's question are the SAME question.
    /// They are: one function answers both.
    #[cfg(unix)]
    #[test]
    fn packaged_agrees_with_the_discovery_that_built_the_row() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        install_packaged_php(home, "8.4", "8.4.24");
        point_current(home, "8.4", "8.4.24");

        let root = openvhost_core::PackagesRoot::from_home(home);
        let found = openvhost_core::discover_php(&root, &[], &|_| None);
        assert_eq!(found.runtimes.len(), 1, "got {found:?}");
        assert_eq!(
            found.runtimes[0].source,
            openvhost_core::PhpRuntimeSource::Packaged {
                version: "8.4.24".to_string()
            }
        );

        let packaged = php("8.4").packaged(home).expect("a packaged 8.4");
        assert!(
            found.runtimes[0].fpm_bin.starts_with(&packaged.version_dir),
            "the row's binary {:?} must live inside the directory the uninstall removes ({:?})",
            found.runtimes[0].fpm_bin,
            packaged.version_dir
        );
    }

    /// The other direction, and the reason the `bin/php-fpm` check belongs in
    /// the shared predicate: a package tree discovery will not USE is not one
    /// this uninstall may remove, because the row the user pressed Uninstall on
    /// is then Homebrew's and `brew uninstall` is the right plan for it.
    #[cfg(unix)]
    #[test]
    fn a_package_tree_discovery_will_not_use_is_not_one_this_uninstall_removes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let root = openvhost_core::PackagesRoot::from_home(home);
        // A version directory with no `bin/php-fpm` in it at all.
        std::fs::create_dir_all(root.package_dir(
            openvhost_core::PHP_PACKAGE_NAME,
            "8.4",
            "8.4.24",
        ))
        .expect("create version dir");
        point_current(home, "8.4", "8.4.24");

        assert_eq!(php("8.4").packaged(home), None);
        let found = openvhost_core::discover_php(&root, &[], &|_| None);
        assert!(found.runtimes.is_empty(), "got {found:?}");
        assert!(
            !found.is_complete(),
            "a broken tree of OURS is reported, not dropped"
        );
    }

    /// `version_dir` is what T2's `remove_dir_all` is pointed at, so the link
    /// shapes `current_version` refuses matter here more than anywhere else in
    /// the codebase.
    #[cfg(unix)]
    #[test]
    fn a_tampered_current_link_yields_no_packaged_install_to_remove() {
        for target in ["..", "../../../etc", "/etc", "a/b", "./8.4.24", ""] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let home = tmp.path();
            install_packaged_php(home, "8.4", "8.4.24");
            point_current(home, "8.4", target);
            assert_eq!(
                php("8.4").packaged(home),
                None,
                "`current` -> {target:?} was accepted"
            );
        }
    }

    /// Scope, stated in a test rather than only in a comment: this slice
    /// threads the packaged state in for PHP only. A packaged MySQL CAN exist
    /// — `install_mysql_package` is wired — and its uninstall still plans
    /// `brew uninstall mysql@8.4`. That gap is real and is recorded for its own
    /// slice; when it is closed, this test fails and has to be changed on
    /// purpose rather than a behaviour changing quietly.
    #[cfg(unix)]
    #[test]
    fn only_php_threads_a_packaged_state_in_for_now() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let root = openvhost_core::PackagesRoot::from_home(home);
        let dir = root.package_dir(openvhost_core::MYSQL_PACKAGE_NAME, "8.4", "8.4.11");
        for name in ["mysqld", "mysql", "mysqladmin"] {
            let bin = dir.join("bin").join(name);
            std::fs::create_dir_all(bin.parent().expect("bin dir")).expect("create bin dir");
            std::fs::write(&bin, b"x").expect("write binary");
        }
        let link = root.current_link(openvhost_core::MYSQL_PACKAGE_NAME, "8.4");
        std::os::unix::fs::symlink(PathBuf::from("8.4.11"), &link).expect("link current");
        // Discovery finds it, so this is a real packaged MySQL and not an
        // empty directory the assertion below would pass over trivially.
        assert!(
            openvhost_core::packaged_mysql_runtime(
                &root,
                &openvhost_core::MysqlMajor::parse("8.4").expect("catalogue major")
            )
            .is_some()
        );

        assert_eq!(mysql("8.4").packaged(home), None);
        assert_eq!(mariadb().packaged(home), None);
    }

    // ---- which keg is named as surviving ---------------------------------

    #[cfg(unix)]
    #[test]
    fn the_kept_keg_is_the_one_homebrew_actually_resolves_to() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prefix = tmp.path();
        brew_layout(prefix, "php@8.4", "php@8.4", "8.4.13");
        // Canonicalized, because `keg_provenance` is: on macOS a tempdir under
        // `/var` resolves to `/private/var`, and comparing against the
        // uncanonicalized path would make a correct answer look wrong.
        let expected =
            std::fs::canonicalize(prefix.join("Cellar/php@8.4/8.4.13")).expect("canonicalize");
        assert_eq!(brew_keg_path(&[prefix], "php@8.4"), Some(expected));
    }

    /// Homebrew's alias trap seen from the KEEPING side. On a machine where
    /// `php@8.5` is an alias for the unversioned `php`, removing a packaged 8.5
    /// still leaves that keg behind and a rescan still shows 8.5 — which is
    /// precisely the case where saying nothing would read as "the uninstall
    /// failed" (design D3's rejected alternative).
    #[cfg(unix)]
    #[test]
    fn an_aliased_keg_is_still_named_as_surviving() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prefix = tmp.path();
        brew_layout(prefix, "php@8.5", "php", "8.5.9");
        let expected =
            std::fs::canonicalize(prefix.join("Cellar/php/8.5.9")).expect("canonicalize");
        assert_eq!(brew_keg_path(&[prefix], "php@8.5"), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_named_as_surviving_when_homebrew_has_no_such_keg() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(brew_keg_path(&[tmp.path()], "php@8.4"), None);
    }

    /// A `Site` with the fields these predicates read; everything else is
    /// filler. Built through the real newtypes so a charset change here fails
    /// loudly rather than letting a test use a domain production would reject.
    pub(super) fn site(name: &str, domain: &str, php_version: &str) -> Site {
        use openvhost_core::{Docroot, Domain, PhpVersion, SiteId, SiteName, WebServer};
        Site {
            id: SiteId::new(),
            name: SiteName::parse(name).expect("valid name"),
            domain: Domain::parse(domain).expect("valid domain"),
            docroot: Docroot::parse("/tmp/ovh/www").expect("valid docroot"),
            web_server: WebServer::Nginx,
            php_version: PhpVersion::parse(php_version).expect("valid version"),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }
}
