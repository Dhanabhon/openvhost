// SPDX-License-Identifier: GPL-3.0-or-later
//! The one managed handle to `state.db` (optional-state.db design, D1).
//!
//! `Db::open` is best-effort at startup — a missing or unreadable store must
//! never stop the supervisor — so for as long as commands took
//! `State<'_, Db>`, the degraded machine got Tauri's own refusal instead of an
//! answer: *"state not managed for field `db` on command `php_environment`.
//! You must call `.manage()` before using this command."* That sentence
//! reached a **user**, in a page that had lost all its rows and controls.
//!
//! [`DbHandle`] is what replaces it. **Both** arms of the open manage one, so
//! extraction succeeds whichever way the open went and each command answers for
//! itself — a typed, renderable refusal ([`DbHandle::require`]) or a degraded
//! but real result ([`DbHandle::optional`]).
//!
//! **"Both arms of the open" is the whole claim, and it is narrower than
//! "unconditionally".** `lib.rs`'s `app.manage` for this sits inside
//! `resolve_home() == Ok(home)` *and* `InstanceLock::acquire() == Ok(Some(_))`.
//! A second instance, a lock error, or a home that will not resolve all return
//! `Ok(())` from setup with the window already created and **nothing managed
//! beyond the six values `lib.rs` manages before that match** (`UiReady`,
//! `ApplyLock`, `InstallLock`, `BulkLock`, `TrayInitiated`, `Quitting`) — so
//! every command that extracts state managed inside that arm gets Tauri's own
//! `.manage()` string back, `state_store_status` included because it reads a
//! `DbHandle`, and the banner stays silent because a failed ask renders as
//! silence by design. The four commands extracting only `InstallLock`
//! (`pending_install` and the three `cancel_*_install`) still answer on that
//! path. That boot path is a separate slice's; this module only promises what
//! it can, which is that opening the store cannot be the reason extraction
//! fails.
//!
//! Two properties are load-bearing rather than incidental:
//!
//! 1. **There is no `inner()`-shaped escape hatch.** Neither accessor hands
//!    out a `&Db` without the caller acknowledging absence, so the worst a new
//!    command can do is refuse.
//! 2. **`Db` itself is never managed again.** That turns "a new
//!    `State<'_, Db>` parameter" from a bug that only fires on a machine whose
//!    store is broken into one that fires on *every* machine, including the
//!    developer's, on the first invocation — it cannot reach a user (D6).

use openvhost_core::Db;
use openvhost_core::mysql::InstallLedger;

use crate::commands::IpcError;

/// The sentence every refusal and the startup log line open with.
///
/// Shared so the developer reading a terminal and the user reading a banner
/// are told the same thing about the same condition.
pub const STORE_UNAVAILABLE: &str = "OpenVHost's data store (state.db) is unavailable this run";

/// `STORE_UNAVAILABLE`, with the reason the open actually failed for.
///
/// **Carrying the reason is the point** — startup already has the `CoreError`
/// and used to only `eprintln!` it, so a refusal could say no more than
/// "unavailable". It can now say *permission denied*.
pub fn unavailable_message(reason: &str) -> String {
    format!("{STORE_UNAVAILABLE}: {reason}")
}

/// The managed store: open, or absent with the reason it is absent.
///
/// Managed on **both arms of the open, and exactly once** — `Manager::manage`
/// does not overwrite an existing value (its own doc example asserts
/// `assert!(!app.manage(MyInt(1)))`), so a "manage `Unavailable` early, the
/// real one later" split would silently pin every user to `Unavailable`.
/// `Manager::unmanage` exists; it is deliberately not used to fake a
/// re-manage.
///
/// Not *unconditionally*: the boot arms around that call can still leave this
/// and every other managed value absent — see this module's header.
pub enum DbHandle {
    /// `Db::open` succeeded at startup.
    Ready(Db),
    /// It did not. `reason` is that failure's own `Display`.
    Unavailable { reason: String },
}

impl DbHandle {
    /// REFUSE: the store, or a typed error naming why there isn't one.
    ///
    /// `IpcError::Core` and **no new variant**: the error genuinely came from
    /// openvhost-core, nothing branches on `kind`, and every affected page
    /// already renders `.message` — a variant earns nothing until some UI
    /// switches on it.
    pub fn require(&self) -> Result<&Db, IpcError> {
        match self {
            DbHandle::Ready(db) => Ok(db),
            DbHandle::Unavailable { reason } => Err(IpcError::Core {
                message: unavailable_message(reason),
            }),
        }
    }

    /// DEGRADE: the store if there is one, and the caller handles `None`.
    ///
    /// For the commands whose real work does not need state.db — only their
    /// bookkeeping does. Returning `Option` rather than a `&Db` is what forces
    /// that handling to be written down.
    pub fn optional(&self) -> Option<&Db> {
        match self {
            DbHandle::Ready(db) => Some(db),
            DbHandle::Unavailable { .. } => None,
        }
    }

    /// DEGRADE, for the packaged installs: a ledger over the store, or `None`.
    ///
    /// The same decision [`optional`](Self::optional) expresses, named once for
    /// the three callers that all make it — `mysql_pkg`, `mariadb_pkg` and
    /// `php_pkg` each turned a handle into an [`InstallLedger`] inline, inside a
    /// function taking `tauri::AppHandle` (= `AppHandle<Wry>`, which
    /// `mock_builder` cannot construct). Nothing could reach that line, so an
    /// edit putting [`require`](Self::require) where `optional` was — silently
    /// turning a DEGRADE command back into a REFUSE — would first have shown up
    /// on a real machine with a broken store. Here it is one function, tested on
    /// both arms.
    ///
    /// What that does **not** cover, said plainly: the call sites still pass the
    /// handle in, and no test can construct the `AppHandle<Wry>` needed to drive
    /// them end to end. This moves the *decision* somewhere a test can reach it;
    /// it does not move the wiring.
    pub fn install_ledger(&self) -> Option<InstallLedger> {
        self.optional().map(InstallLedger::new)
    }

    /// Why the store is unavailable, for a DEGRADE path that wants to say so.
    ///
    /// `None` means it is available — this is not an error channel.
    ///
    /// Its caller is [`state_store_status`] below, the command behind D5's
    /// app-level banner: the one place that needs the reason itself rather
    /// than a refusal built from it.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            DbHandle::Ready(_) => None,
            DbHandle::Unavailable { reason } => Some(reason),
        }
    }
}

/// Whether `state.db` opened this run, and if it did not, why (D5).
///
/// The honesty half of DEGRADE. A command that degrades returns a *shorter*
/// answer — `list_log_sources` drops every site row, `php_environment` reports
/// no chosen default — and a shorter answer is indistinguishable from "you have
/// no sites" or "you have no preference". Those are quiet wrong answers, which
/// is the failure mode this project keeps getting burned by, so the app asks
/// this once and says so.
///
/// **`Some(reason)` is the whole payload**, not a bare boolean: the banner is
/// there to tell a user *permission denied* or *unable to open database file*
/// rather than a generic sentence they can do nothing with.
///
/// `Result` with an error that never occurs, like `pending_install` next door:
/// this is a status read with nothing to fail, and every command on this surface
/// shares the one envelope the frontend's `unwrap` understands. Zero-arg — there
/// is nothing to validate, and a caller can learn only what the app already
/// renders.
#[tauri::command]
#[specta::specta]
pub async fn state_store_status(
    db: tauri::State<'_, DbHandle>,
) -> Result<Option<String>, IpcError> {
    Ok(db.unavailable_reason().map(str::to_string))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The whole point of the slice, pinned at the source: a refusal names the
    /// reason the store is missing, and says nothing about `.manage()`.
    #[test]
    fn require_refuses_with_the_reason_and_never_mentions_manage() {
        let handle = DbHandle::Unavailable {
            reason: "Permission denied (os error 13)".into(),
        };

        // `Db` has no `Debug`, so the `Ok` arm is named rather than unwrapped.
        let Err(err) = handle.require() else {
            panic!("an unavailable store must refuse");
        };
        let IpcError::Core { message } = &err else {
            panic!("expected IpcError::Core, got {err:?}");
        };
        assert!(
            message.contains("Permission denied (os error 13)"),
            "the refusal must carry the reason: {message:?}"
        );
        assert!(
            message.contains("state.db"),
            "the refusal must name what is unavailable: {message:?}"
        );
        assert!(
            !message.contains(".manage()"),
            "the user must never be told to call a Rust API: {message:?}"
        );
    }

    #[tokio::test]
    async fn a_ready_handle_hands_out_the_store_through_both_accessors() {
        let handle = DbHandle::Ready(Db::open_in_memory().await.expect("in-memory db"));

        assert!(handle.require().is_ok());
        assert!(handle.optional().is_some());
        assert_eq!(
            handle.unavailable_reason(),
            None,
            "a Ready handle has no reason to report"
        );
    }

    #[test]
    fn an_unavailable_handle_degrades_to_none_and_reports_why() {
        let handle = DbHandle::Unavailable {
            reason: "disk I/O error".into(),
        };

        assert!(handle.optional().is_none());
        assert_eq!(handle.unavailable_reason(), Some("disk I/O error"));
    }

    // ---- install_ledger: the one DEGRADE line with no other seam ----------
    //
    // Both directions, because either alone is satisfiable by a constant: a body
    // hardcoded to `None` keeps `…_yields_no_ledger…` green while reddening
    // `…_hands_the_install_paths_a_ledger`, and there is no way to hardcode
    // `Some` at all — an `InstallLedger` cannot be built without a `&Db`, which
    // is design D6 holding at the type level rather than by assertion.

    #[tokio::test]
    async fn a_ready_handle_hands_the_install_paths_a_ledger() {
        let handle = DbHandle::Ready(Db::open_in_memory().await.expect("in-memory db"));

        assert!(
            handle.install_ledger().is_some(),
            "a working store must produce the ledger the installs record into"
        );
    }

    #[test]
    fn an_unavailable_handle_yields_no_ledger_rather_than_refusing() {
        let handle = DbHandle::Unavailable {
            reason: "unable to open database file (os error 14)".into(),
        };

        // The whole D2/D4 point: `None`, not an error. The install still runs and
        // reports `LedgerWrite::Failed`; it is not refused.
        assert!(
            handle.install_ledger().is_none(),
            "a store that never opened must cost the ledger row, never the install"
        );
    }

    // ---- state_store_status: the banner's one input ----------------------
    //
    // Both directions, because a status command that answers the same thing
    // either way is worse than none: the banner would then be permanently on
    // (crying wolf on every healthy machine) or permanently off (which is
    // exactly the silence D5 exists to break). Vacuity: hardcoding `Ok(None)`
    // reddens `…_reports_the_reason_when_the_store_is_down` and leaves
    // `…_is_silent_on_a_healthy_store` green; hardcoding `Ok(Some(...))`
    // reddens the second and leaves the first green. Neither test can stand in
    // for the other.

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[tokio::test]
    async fn state_store_status_is_silent_on_a_healthy_store() {
        use tauri::Manager;
        let app = mock_app();
        app.manage(DbHandle::Ready(Db::open_in_memory().await.unwrap()));

        assert_eq!(
            state_store_status(app.state::<DbHandle>()).await.unwrap(),
            None,
            "a working store must say nothing at all — the banner is keyed off this"
        );
    }

    #[tokio::test]
    async fn state_store_status_reports_the_reason_when_the_store_is_down() {
        use tauri::Manager;
        let app = mock_app();
        app.manage(crate::commands::store_down());

        let status = state_store_status(app.state::<DbHandle>())
            .await
            .unwrap()
            .expect("a store that never opened must report itself");

        // The REASON, not merely "it is down". A banner that could only say
        // "unavailable" would leave the user with nothing to act on, which is
        // the state this slice replaces.
        assert_eq!(status, crate::commands::STORE_DOWN_REASON);
    }

    // ---- D6's guard: no command may take a bare `Db` again ----------------
    //
    // The real guarantee is a property, not this test: `Db` is managed nowhere,
    // so a new `db: State<'_, Db>` parameter is refused on EVERY machine — the
    // developer's included — on its first invocation, and cannot reach a user.
    // This is the cheap tripwire that says so at `cargo test` time instead of at
    // first click, and the design (§7) asks for exactly it.
    //
    // Honest about its reach, measured by building each of these as a real
    // registered command and watching the guard pass:
    //
    // - `State<'_, openvhost_core::Db>` — a fully qualified path. Still slips
    //   past; it is a textual scan.
    // - `use openvhost_core::Db as Store;` or a type alias — likewise.
    // - `State<'a, Db>` on a command declared `fn f<'a>(…)`. This one used to
    //   slip past and no longer does: the scan matches the SHAPE, any lifetime
    //   name, not the literal `'_`. It was the cheapest of the three to close,
    //   and the only one where the evading code looks entirely ordinary.
    //
    // The two that remain are acceptable because this is the SECOND line of
    // defence, not the first.

    /// Whether a whitespace-stripped line declares `State<'…, T>`, where `tail`
    /// is the `,T>` being looked for — for **any** lifetime, `'_` included.
    ///
    /// A shape match rather than one literal, because `'_` is not the only
    /// lifetime that compiles in that position (see the note above). The pieces
    /// are spelled with `concat!` here and at the call sites so that scanning
    /// this very file does not find the scanner: `db_state.rs` is in
    /// `COMMAND_FILES` on purpose — it defines a command now — and a single
    /// literal would match itself and fail forever.
    fn declares_state(squeezed: &str, tail: &str) -> bool {
        let head = concat!("State", "<'");
        let mut rest = squeezed;
        while let Some(at) = rest.find(head) {
            let after = &rest[at + head.len()..];
            // The lifetime's own name: `_`, or an identifier.
            let name_end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            if name_end > 0 && after[name_end..].starts_with(tail) {
                return true;
            }
            rest = after;
        }
        false
    }

    /// The `,T>` tail of the parameter no command may take.
    fn bare_db_tail() -> &'static str {
        concat!(",", "Db>")
    }

    /// The `,T>` tail of the parameter every command takes instead.
    fn db_handle_tail() -> &'static str {
        concat!(",", "DbHandle>")
    }

    /// Every file in this crate that defines a `#[tauri::command]`, plus
    /// `lib.rs` (which defines none today, but holds `collect_commands!` and is
    /// where a stray one would most plausibly land).
    ///
    /// **Hand-maintained, and that is this guard's widest gap** — a brand new
    /// `*_pkg.rs` full of commands would simply not be scanned, and nothing here
    /// would say so. `every_registered_command_lives_in_a_scanned_file` below is
    /// what notices, by tying this list's attribute count to `collect_commands!`.
    const COMMAND_FILES: &[(&str, &str)] = &[
        ("commands.rs", include_str!("commands.rs")),
        ("db_state.rs", include_str!("db_state.rs")),
        ("php_pkg.rs", include_str!("php_pkg.rs")),
        ("mysql_pkg.rs", include_str!("mysql_pkg.rs")),
        ("mariadb_pkg.rs", include_str!("mariadb_pkg.rs")),
        ("uninstall/run.rs", include_str!("uninstall/run.rs")),
        ("lib.rs", include_str!("lib.rs")),
    ];

    /// `(file, line number, line)` for every CODE line `matches` accepts, the
    /// line being whitespace-stripped before it is offered.
    ///
    /// Whole-line comments are skipped and nothing else is: six `//` lines
    /// across three files in this crate document the very property being
    /// guarded, and deleting them to make a test easier would be the wrong
    /// trade. A trailing comment on a code line would still trip the scan —
    /// deliberately, since that is the failing direction that is safe to be
    /// wrong in.
    fn code_hits(matches: impl Fn(&str) -> bool) -> Vec<(&'static str, usize, String)> {
        let mut hits = Vec::new();
        for (name, src) in COMMAND_FILES {
            for (i, line) in src.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                if matches(&squeezed) {
                    hits.push((*name, i + 1, line.trim().to_string()));
                }
            }
        }
        hits
    }

    #[test]
    fn no_command_takes_a_bare_db_as_managed_state() {
        let hits = code_hits(|line| declares_state(line, bare_db_tail()));
        assert!(
            hits.is_empty(),
            "a command took `Db` as managed state again — `Db` is managed nowhere, \
             so this refuses on every machine. Take `State<'_, DbHandle>` and call \
             `require()` (refuse) or `optional()` (degrade). Found: {hits:#?}"
        );
    }

    /// The scan's own control, in both directions at once.
    ///
    /// Without this, `no_command_takes_a_bare_db_as_managed_state` would pass
    /// just as happily if `code_hits` were looking at nothing — a mis-joined
    /// path, an over-eager comment filter, a shape match whose lifetime rule
    /// never fires. So: the SAME scanner, on the SAME files, must find the
    /// parameter the commands actually take.
    #[test]
    fn the_scan_reads_real_code_rather_than_finding_nothing_everywhere() {
        let handles = code_hits(|line| declares_state(line, db_handle_tail()));
        assert!(
            handles.len() > 20,
            "the scan should see every `State<'_, DbHandle>` parameter — 28 when this \
             was written, the 27 migrated commands plus `state_store_status`'s own, and \
             it reports 30 because two of this test module's own assertion strings spell \
             the parameter out. It found {}: {handles:#?}",
            handles.len()
        );
        assert!(
            handles.iter().any(|(f, ..)| *f == "commands.rs"),
            "commands.rs holds most of the command surface and must be among them"
        );
    }

    /// The prose stays, and does not defeat the scan.
    ///
    /// Six comment lines across three files in this crate spell out
    /// `State<'_, Db>` while explaining why no command may take one — this very
    /// line among them. They are the documentation of the property; a guard that
    /// could be satisfied by deleting them would be guarding the wrong thing.
    #[test]
    fn the_scan_is_not_fooled_by_comments_that_name_the_forbidden_parameter() {
        let spaced = concat!("State<'_, ", "Db>");
        let prose: Vec<&str> = COMMAND_FILES
            .iter()
            .filter(|(_, src)| src.contains(spaced))
            .map(|(name, _)| *name)
            .collect();
        assert!(
            prose.len() >= 3,
            "the comments documenting this property have gone missing (found {prose:?}) — \
             either they were deleted, or this test is no longer proving anything"
        );
        assert!(
            code_hits(|line| declares_state(line, bare_db_tail())).is_empty(),
            "…and yet the scan reported a hit, so it is counting prose as code"
        );
    }

    /// The scanned list is the registered list.
    ///
    /// `COMMAND_FILES` is hand-maintained, which makes every guard above only as
    /// complete as it is: a new `*_pkg.rs` full of commands is not scanned, and
    /// nothing else notices — the likeliest way this whole block quietly stops
    /// covering the command surface. So tie it to the one place a command must
    /// ALSO appear to exist at all, `lib.rs`'s `collect_commands!`: a registered
    /// command in an unscanned file makes these two counts disagree.
    ///
    /// Equality, not `>=`, so it fails in both directions — a command defined
    /// but never registered is dead code, and worth hearing about too.
    ///
    /// `db_state.rs` scans itself, so both needles are split across `concat!`
    /// and the failure message interpolates `attribute` rather than spelling it:
    /// a literal in either place counts as one more definition, which is exactly
    /// how this test first failed at 51 against 50.
    ///
    /// Splitting the literal removed that instance without closing the class.
    /// While the scan merely asked whether a line CONTAINED the attribute, any
    /// ordinary code line carrying it anywhere in `COMMAND_FILES` counted as a
    /// definition — and the message below then blamed an unlisted file, which is
    /// the wrong diagnosis for a self-match and costs the next person more than
    /// the failure itself does. So the line must now START with the attribute:
    /// rustfmt always puts it on its own line, neither a comment nor a string
    /// literal can begin with `#[`, and dropping the closing bracket keeps a
    /// `#[tauri::command(…)]` with arguments counted as the definition it is.
    #[test]
    fn every_registered_command_lives_in_a_scanned_file() {
        let attribute = concat!("#[tauri::", "command");
        let defined: usize = COMMAND_FILES
            .iter()
            .map(|(_, src)| {
                src.lines()
                    .filter(|l| l.trim().starts_with(attribute))
                    .count()
            })
            .sum();

        let lib = COMMAND_FILES
            .iter()
            .find(|(name, _)| *name == "lib.rs")
            .expect("lib.rs is scanned")
            .1;
        let at = lib
            .find(concat!("collect_", "commands!["))
            .expect("lib.rs must register the command surface");
        let registered = lib[at..]
            .lines()
            .skip(1)
            .take_while(|l| !l.trim_start().starts_with("])"))
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with("//")
            })
            .count();

        assert_eq!(
            defined, registered,
            "{defined} lines starting with `{attribute}` across COMMAND_FILES but \
             {registered} entries in `collect_commands!`. Either a command lives in a \
             file COMMAND_FILES does not list — in which case none of the guards above \
             are looking at it — or one is defined and never registered."
        );
    }
}
