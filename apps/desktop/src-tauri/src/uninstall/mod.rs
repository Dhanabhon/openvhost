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
use openvhost_core::{PhpMajor, Site};
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
}

impl Target {
    pub(crate) fn parse(kind: PackageKind, major: &str) -> Result<Self, IpcError> {
        Ok(match kind {
            PackageKind::Php => Target::Php(PhpMajor::parse(major)?),
            PackageKind::Mysql => Target::Mysql(MysqlMajor::parse(major)?),
        })
    }

    pub(crate) fn kind(&self) -> PackageKind {
        match self {
            Target::Php(_) => PackageKind::Php,
            Target::Mysql(_) => PackageKind::Mysql,
        }
    }

    pub(crate) fn major(&self) -> &str {
        match self {
            Target::Php(m) => m.as_str(),
            Target::Mysql(m) => m.as_str(),
        }
    }

    /// The supervisor row this version owns, derived from `stack`'s own id
    /// builders so it can never name a row the registration side did not
    /// create.
    pub(crate) fn service_id(&self) -> String {
        match self {
            Target::Php(m) => crate::stack::php_fpm_service_id(m.as_str()),
            Target::Mysql(m) => crate::stack::mysql_service_id(m.as_str()),
        }
    }

    /// How this version reads in a sentence — "PHP 8.4", "MySQL 8.4".
    pub(crate) fn display(&self) -> String {
        match self {
            Target::Php(m) => format!("PHP {}", m.as_str()),
            Target::Mysql(m) => format!("MySQL {}", m.as_str()),
        }
    }

    /// The `InstallLock` slot discriminator, and the label that slot carries.
    /// Matches the shapes the install commands use exactly (PHP's label is
    /// bare, MySQL's is a complete phrase) — see `PendingInstallDto`.
    pub(crate) fn install_kind(&self) -> InstallKind {
        match self {
            Target::Php(_) => InstallKind::Php,
            Target::Mysql(_) => InstallKind::Mysql,
        }
    }

    pub(crate) fn pending_label(&self) -> String {
        match self {
            Target::Php(m) => m.as_str().to_string(),
            Target::Mysql(m) => format!("MySQL {}", m.as_str()),
        }
    }

    /// `brew uninstall <formula>`, composed entirely inside `openvhost-core`.
    pub(crate) fn uninstall_spec(&self, brew: &Path) -> Result<SpawnSpec, IpcError> {
        Ok(match self {
            Target::Php(m) => openvhost_core::brew_uninstall_spec(brew, m)?,
            Target::Mysql(m) => openvhost_core::mysql_brew_uninstall_spec(brew, m)?,
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
    /// `Supervisor::unregister` — the row on the Services page and in the tray.
    ServiceRow { id: String },
}

impl Removal {
    pub(crate) fn describe(&self) -> String {
        match self {
            Removal::BrewFormula { formula, what } => format!("{what} (the {formula} formula)"),
            Removal::GeneratedFile { path, what } => format!("{what} at {}", path.display()),
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
    }
}

/// What removing `target` does, and what it deliberately leaves alone.
///
/// Exhaustive over [`Target`] — and therefore over [`PackageKind`] — with no
/// wildcard arm, so a third package family cannot inherit PHP's inventory (or
/// an empty one) by default.
///
/// A pure function of `(target, home)`: it does not stat anything, so the plan
/// a dialog shows and the sequence the executor runs are the same value even
/// if the disk changed in between. The one consequence is that
/// `Removal::ServiceRow` is listed even when no row happens to be registered
/// (an installed-but-never-initialized MySQL major has none) — the executor
/// treats "already absent" as done, which is the honest reading of a removal
/// anyway.
pub(crate) fn inventory(target: &Target, home: &Path) -> Inventory {
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
            Inventory {
                removes: vec![
                    Removal::BrewFormula {
                        formula: openvhost_core::brew_formula(major),
                        what: format!("The PHP {m} program files"),
                    },
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
                    // relevant is backwards.
                    kept(&format!("Your PHP {m} logs"), log_dir),
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
                ],
            }
        }
        Target::Mysql(major) => {
            let m = major.as_str();
            let paths = openvhost_core::mysql_paths(home, major);
            Inventory {
                removes: vec![
                    Removal::BrewFormula {
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
                    // THE reason this slice exists (plan principle 2).
                    kept("Your databases", Some(paths.datadir.clone())),
                    // D2: keeping the data and throwing away the key is the
                    // same as destroying it.
                    kept("The stored root password", None),
                    kept("This instance's my.cnf", Some(paths.my_cnf.clone())),
                    kept("Your own MySQL overrides", Some(paths.custom_confd.clone())),
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

/// Everything standing in the way of removing `target`, in the order a user
/// should read them (design D3).
///
/// Pure, and re-run by the executor rather than trusted from the plan: between
/// a dialog opening and its confirm button being pressed, a service can be
/// started from the tray and a site can be repointed.
pub(crate) fn blockers(
    target: &Target,
    services: &[ServiceStatus],
    sites: &[Site],
) -> Vec<Blocker> {
    let mut out = Vec::new();
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
    }
    out
}

/// Build the plan a dialog renders and a disabled state reads.
///
/// `disabled` is `!blockers.is_empty()`; the UI does not re-derive the rule.
pub(crate) fn build_plan(
    target: &Target,
    home: &Path,
    services: &[ServiceStatus],
    sites: &[Site],
) -> UninstallPlan {
    let inv = inventory(target, home);
    UninstallPlan {
        kind: target.kind(),
        major: target.major().to_string(),
        removes: inv.removes.iter().map(Removal::describe).collect(),
        keeps: inv.keeps,
        blockers: blockers(target, services, sites),
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

    fn status(id: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: None,
            pid: None,
            state,
        }
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
        let inv = inventory(&php("8.4"), home);
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
        let inv = inventory(&php("8.4"), Path::new("/tmp/ovh"));
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
        let inv = inventory(&mysql("8.4"), Path::new("/tmp/ovh"));
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
        let inv = inventory(&mysql("8.4"), Path::new("/tmp/ovh"));
        assert_eq!(
            inv.keeps,
            vec![
                KeptItem {
                    what: "Your databases".to_string(),
                    path: Some("/tmp/ovh/data/mysql/8.4".to_string()),
                },
                KeptItem {
                    what: "The stored root password".to_string(),
                    path: None,
                },
                KeptItem {
                    what: "This instance's my.cnf".to_string(),
                    path: Some("/tmp/ovh/config/generated/mysql/8.4/my.cnf".to_string()),
                },
                KeptItem {
                    what: "Your own MySQL overrides".to_string(),
                    path: Some("/tmp/ovh/config/custom/mysql/8.4/conf.d".to_string()),
                },
            ]
        );
    }

    #[test]
    fn no_removal_of_any_kind_ever_names_a_data_or_log_path() {
        // The blanket invariant, checked against the removal list itself
        // rather than against a particular arm: whatever a future kind adds,
        // it may not put the user's data or logs on the chopping block.
        let home = Path::new("/tmp/ovh");
        for target in [php("8.1"), php("8.4"), mysql("8.4")] {
            for removal in inventory(&target, home).removes {
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
    fn the_displayed_formula_is_the_one_that_reaches_brews_argv() {
        // Two readers of one fact: the dialog's `removes` line and the child
        // process's argv. Pinned against the REAL spec builder so they cannot
        // drift into naming different formulas.
        let brew = Path::new("/opt/homebrew/bin/brew");
        for target in [php("8.4"), mysql("8.4")] {
            let inv = inventory(&target, Path::new("/tmp/ovh"));
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

    #[test]
    fn a_php_major_a_site_still_uses_is_refused_and_the_site_is_named() {
        let sites = vec![
            site("shop", "shop.localhost", "8.4"),
            site("blog", "blog.localhost", "8.1"),
            site("wiki", "wiki.localhost", "8.4"),
        ];
        let found = blockers(&php("8.4"), &[], &sites);
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
        assert!(blockers(&php("8.4"), &[], &sites).is_empty());
    }

    #[test]
    fn a_disabled_site_still_pins_its_php_version() {
        // A disabled site is still a site whose configuration names 8.4, and
        // re-enabling it after the uninstall would fail to apply. Naming it
        // now is the recoverable outcome.
        let mut s = site("shop", "shop.localhost", "8.4");
        s.enabled = false;
        assert_eq!(
            blockers(&php("8.4"), &[], &[s]),
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
        assert!(blockers(&mysql("8.4"), &[], &sites).is_empty());
    }

    #[test]
    fn a_running_service_and_pinned_sites_are_both_reported() {
        // Not "the first obstacle": a user who stops the service only to be
        // told about the sites has been made to guess twice.
        let services = vec![status("php-fpm-8.4", ServiceState::Running)];
        let sites = vec![site("shop", "shop.localhost", "8.4")];
        let found = blockers(&php("8.4"), &services, &sites);
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
        assert!(blockers(&php("8.4"), &services, &[]).is_empty());
    }

    #[test]
    fn a_plan_with_no_blockers_may_proceed_and_lists_both_halves() {
        let plan = build_plan(&mysql("8.4"), Path::new("/tmp/ovh"), &[], &[]);
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
        let plan = build_plan(&target, home, &[], &[]);
        let expected: Vec<String> = inventory(&target, home)
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
        let v =
            serde_json::to_value(build_plan(&php("8.4"), Path::new("/tmp/ovh"), &[], &[])).unwrap();
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
