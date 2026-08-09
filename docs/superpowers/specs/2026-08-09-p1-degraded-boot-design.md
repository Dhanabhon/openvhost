<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# When the app never started, say so — instead of leaving 40 commands to say it badly

**Status:** design, ready to plan.
**Date:** 2026-08-09.

## 1. The situation, measured

`lib.rs`'s `.setup()` bootstraps inside two nested matches:

```
match resolve_home() {                                  // :362
    Ok(home) => match InstanceLock::acquire(&run_dir) {  // :365
        Ok(Some(lock)) => { /* the whole app */ }
        Ok(None)  => { eprintln!("another instance holds the run lock…"); }   // :637
        Err(e)    => { eprintln!("failed to acquire the run lock: {e}"); }    // :643
    },
    Err(e) => { eprintln!("cannot resolve OPENVHOST_HOME ({e})…"); }          // :647
}
Ok(())
```

All three bail arms print to stderr and return `Ok(())`. **The window still opens** and almost
nothing is managed, so nearly every command returns Tauri's own error, which the frontend renders
verbatim:

> state not managed for field `db` on command `php_environment`. You must call `.manage()` before
> using this command.

PR #69 removed that sentence from the *store-unavailable* path. It cannot help here: its
`state_store_status` command is itself unmanaged on these arms, and the frontend store treats a
failed ask as silence by design. **The app is quietest exactly when it is most broken.**

Six values *are* managed before the match (`UiReady`, `ApplyLock`, `InstallLock`, `BulkLock`,
`TrayInitiated`, `Quitting`), so four commands still answer — `pending_install`,
`cancel_php_install`, `cancel_mysql_install`, `cancel_mariadb_install` — plus a handful taking no
state. Everything else fails.

## 2. A correction to the premise this slice was filed under

The chip that produced this slice said the ordinary cause of `Ok(None)` is a user double-clicking an
already-running app. **That is wrong, and I verified it rather than repeating it.**

`lib.rs:701` already handles `tauri::RunEvent::Reopen`, calling `reopen_window` (`:745`) to show,
un-minimize and focus. On macOS, LaunchServices activates the running instance for a bundled app —
no second process, no `Ok(None)`.

The real producers are **developer** scenarios: `pnpm tauri dev`, running `Contents/MacOS/OpenVHost`
directly, `open -n`, two copies of the bundle at different paths, or a second GUI session. That
changes the design: the case "focus the running window" would optimise for is one the OS already
handles, and for a developer *"another instance is already running"* is more informative than being
silently redirected.

## 3. D1 — One `BootState`, produced by every path, managed exactly once

```rust
enum BootState {
    Ready,
    AlreadyRunning  { home: PathBuf },
    RunDirUnusable  { run_dir: PathBuf, reason: String },
    HomeUnresolvable{ reason: String },
}
```

A `fn bootstrap(app) -> BootState` whose **every** return path yields one, then a single
`app.manage(boot)` at the top level, outside every arm. The rendering decision lives in a pure
`fn boot_dto(&BootState) -> BootStatusDto` — that is where the tests go, because `AppHandle<Wry>`
cannot be constructed under `mock_builder`.

This borrows the *structural* lesson from `DbHandle` — one value, produced by every path, managed
exactly once — and nothing else. `Manager::manage` does not overwrite, so a "manage a placeholder
early, the real value later" split would silently pin the placeholder.

## 4. D2 — A takeover screen, NOT per-command unavailable values

The obvious move is to extend PR #69's shape: manage every state as an explicit "unavailable,
because…" value and let each command refuse honestly. **That reasoning does not survive here.**

`DbHandle` works because a broken store is a *partial* failure of a *running* app — 20 refuse, 5
degrade, 2 split, and there is a real app behind the banner. Here **nothing** works, so a
per-command classification carries zero information. Worse, the "unavailable" values would be
**indistinguishable from legitimate empty states**: `Option<StackPaths> = None` is documented in this
very file as the *normal* state on a non-macOS target, and an empty `Supervisor` looks exactly like a
machine with nothing installed. That trades a frightening developer string for a **plausible lie**,
which is the failure class this project keeps getting burned by.

So: `+layout.svelte` gates `{@render children()}` on `Ready` and otherwise renders a takeover.

**One rule that must not be copied from the store slice:** on a *failed ask* — the command itself
erroring — render the children **plus a visible banner**, never a blank app. Blanking a healthy app
over an unanswered question is a worse failure than the one being fixed.

## 5. D3 — Two screens for three states

| state | screen |
|---|---|
| `AlreadyRunning` | "OpenVHost is already running", naming the contended home |
| `RunDirUnusable` | the run dir and the OS error **verbatim**, plus Reveal in Finder and Quit |
| `HomeUnresolvable` | the same screen, a different sentence |

`RunDirUnusable` is a user-fixable permissions problem, so the path and the errno *are* the payload —
measured live as `Permission denied (os error 13)` on a read-only home.

`HomeUnresolvable` gets no bespoke UI because it is near-unreachable on macOS: `home.rs` filters an
empty override, so it needs `$HOME` unset *and* a failing passwd lookup, or a deleted cwd. A shared
screen is honest; a designed one would over-serve a state nobody will see.

## 6. D4 — No auto-exit and no auto-focus in this slice

Focus-and-exit is the tempting answer and it **loses on this project's own test — which side fails
quietly.** An ack over the control socket proves *"a live instance heard me"*, not *"a window came to
the front"*; macOS can decline cross-app activation and give a bouncing Dock icon instead. The
failure mode is: user launches, process exits, nothing appears, **no window at all** — strictly worse
than today's broken window, and unverifiable from inside the app.

Combined with §2 — the ordinary path never reaches here — the value is small and the downside is
silent. A **"Bring it to the front" button** on the `AlreadyRunning` screen is the shape that keeps
the benefit without the quiet failure, and it is deferred to a follow-up: the screen is needed
regardless and carries most of the value.

If it is ever added, the unix socket is the right channel — it identifies the *actual lock holder*,
since lock and socket are bound in the same arm off the same home, whereas LaunchServices identifies
a *bundle* and could launch a third process into a relaunch loop.

**The second instance must never take over the lock.**

## 7. D5 — Retry the lock once, because a probe really does hold it

**Corrected in the fix wave, and the truth is better than the claim.** This section originally cited
`lock.rs:147-149`'s fork/exec window. That citation was wrong twice over: it is a `#[cfg(test)]` doc
comment whose own last line reads *"Not a production concern: the CLI process that probes spawns
nothing"*, and mechanically a live holder that forks still genuinely holds the lock, so that window
cannot produce a *spurious* `Ok(None)` at all.

The real producer is in this product. `InstanceLock::probe` (`lock.rs:105-117`) acquires the lock and
**immediately drops it** to answer "is a supervisor live?", and `openvhost status` calls it — so for
the microseconds a probe holds the `flock`, a launch racing it reads `Ok(None)` and would conclude
another instance owns this home.

Measured against the real `InstanceLock`, one thread probing while another launched repeatedly:
**27 spurious `Ok(None)` in 27,000 launches** over 195,981 probes without the retry, **0 with it**,
and **0 in 9,000 launches with no prober running** — the control that ties the two together.

Retry `acquire` once after ~100 ms before concluding `AlreadyRunning`. Cheap, and it removes a
false takeover screen that would be maddening precisely because it disappears on the next try.

## 8. D6 — Two lifecycle details that make the screen a trap if missed

- **Hide-on-close only when `Ready`.** The tray is built inside the `Ready` arm only, so a closed
  degraded window with hide-on-close becomes a hidden zombie with nothing to bring it back.
- **Reuse `TitleBar.svelte`.** The window is `titleBarStyle: "Overlay"` with `hiddenTitle`, so the
  takeover must reserve the traffic-light strip and provide a drag region. A hand-rolled `<div>`
  makes the window unmovable.

## 9. What this slice must prove

1. **No page shows Tauri's `.manage()` string in any of the three states.**
2. Each state renders its own screen, naming the real path and the real error.
3. **A failed `boot_status` ask renders children plus a banner**, never a blank app.
4. The degraded window is **movable and quittable**, and closing it does not strand a hidden zombie.
5. The lock retry suppresses a spurious `Ok(None)` and does **not** mask a genuinely held lock.
6. The second instance never acquires or releases the first's lock, and `perform_quit` on a degraded
   instance cannot damage the running one.
7. `boot_status` in a new `boot.rs` must be added to `db_state.rs`'s `COMMAND_FILES`, or the guard
   test `every_registered_command_lives_in_a_scanned_file` fails — by design.
8. Vacuity per group; every branch here is new coverage with no precedent.

## 10. Confidence, and what the gate should second-guess

This design rests on **one** independent analysis plus my own verification of its two load-bearing
claims (the `Reopen` handling at `lib.rs:701`, and the fork window at `lock.rs:147-149` — the second
of which the gate then measured and **falsified**; see §7). A second
opinion was commissioned twice and both runs died on a tooling timeout, so the usual two-take
protocol for an app-lifecycle decision did **not** complete. One partial signal from the incomplete
run agreed with §4.

The gate should treat **D4 (no auto-focus) and D2 (takeover rather than per-command values)** as the
two places where a second view would most likely have differed, and argue them rather than check them.

## 11. Out of scope

The "Bring it to the front" button (D4) · `LSMultipleInstancesProhibited`, which would close the GUI
duplicate-launch path harder but blocks `open -n` and does nothing about direct exec · any change to
`resolve_home`'s fail-closed behaviour, which is a deliberate earlier security fix.
