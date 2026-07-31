# P1 `openvhost` on PATH — Packaging & Install Design

- **Date:** 2026-07-31
- **Status:** Approved under the owner's standing delegation. Closes owner call #2 from PR #40.
- **Predecessor:** `docs/superpowers/specs/2026-07-31-p1-cli-design.md` shipped the CLI (PR #40, `c31239e`). It works, and today it is unreachable: the binary exists only at `target/{debug,release}/openvhost`.
- **Design process:** written directly. Not dual-blind — the choice space is small once the machine facts are in hand (below), and the pattern is well-trodden (VS Code, `gh`, Docker Desktop). Dual-blind stays reserved.
- **Plan:** `docs/superpowers/plans/2026-07-31-p1-cli-install.md`

## What ships

The `openvhost` binary rides inside `OpenVHost.app`. An **OpenVHost → Install Command Line Tool…** menu item symlinks it into a directory on the user's PATH and reports exactly what it did. Afterwards `openvhost list` works from any terminal. No admin prompt, no privileged helper, no shell-profile editing.

## The fact that shaped the design

Measured on the owner's machine (Apple Silicon):

| directory | exists | writable | on PATH |
|---|---|---|---|
| `/usr/local/bin` | yes | **no** (root-owned) | yes |
| `/opt/homebrew/bin` | yes | yes | yes |
| `~/.local/bin` | yes | yes | yes |

**A writable directory that is already on PATH exists, so we never need to escalate privileges.** That is the whole design. The privileged helper is Phase 3 and the app is not yet signed; a slice that needed `sudo` would have to wait for both. This one does not.

## Decisions

### D1 — The binary ships inside the app bundle

Tauri `bundle.externalBin`, so `openvhost` lands in `OpenVHost.app/Contents/MacOS/` beside the app binary. A pre-build step builds `-p openvhost --release` and stages it under the target-triple name Tauri requires (`openvhost-aarch64-apple-darwin`).

The install action resolves the source path from **`std::env::current_exe()`'s parent**, never a hardcoded `/Applications`. This has two payoffs: the app works wherever the user drags it, and in a **dev build** `target/debug/openvhost` sits beside `openvhost-desktop`, so the same code path works unbundled — which is what makes the click-list runnable before there is ever a `.dmg`.

If the sibling binary is missing, the action fails with that as the reason. It never falls back to searching PATH for some other `openvhost`.

### D2 — Where it installs, and the one directory we deliberately skip

Candidates, in order. The first that is **writable** wins:

1. **`/usr/local/bin`** — the traditional location, on the default macOS PATH, and *not* managed by Homebrew on Apple Silicon. Used when writable.
2. **`~/.local/bin`** — user-owned, needs no privileges, conventional. Created (0755) if absent.

**`/opt/homebrew/bin` is deliberately not a candidate, even though it is writable and on PATH.** That directory belongs to Homebrew, and `brew doctor` reports unbrewed symlinks there as a warning. Shipping a feature that makes a widely-used diagnostic complain is not a good trade for one saved fallback. Stated here so it reads as a decision rather than an omission.

**Never** offered: `/bin`, `/usr/bin`, `/sbin` (SIP-protected), or anything requiring authorization. If neither candidate is writable, the action reports that and stops — it does not prompt for a password. That case cannot occur if `$HOME` is writable, so it is a defensive arm, not a user-facing path.

### D3 — A symlink, and it never clobbers anything that is not ours

Create a symlink at `<dir>/openvhost` pointing at the sibling binary. Symlink, not a copy: an app update then updates the CLI for free, and a stale copy silently speaking an old protocol version to a new socket is exactly the drift the shared `SCHEMA_VERSION` exists to prevent.

Clobber rules, in order:
- **Nothing there** → create.
- **A symlink already pointing at our target** → already installed, report success, change nothing.
- **A symlink pointing at a *different* `…/Contents/MacOS/openvhost` or a previous build** → ours, from an older install or a moved app. Replace it.
- **A symlink pointing anywhere else, or any regular file, directory or other node** → **refuse**, naming the path and what is there. Never unlink something we did not create. A user with their own `openvhost` on PATH must not lose it silently.

Write it atomically: create the symlink at a temporary name in the same directory, then `rename` over the target. A half-installed PATH entry is worse than none.

### D4 — Find the user's real PATH from their login shell, with a timeout, and a third state

A GUI app launched from Finder inherits a minimal `PATH`, not the shell's. Reading `std::env::var("PATH")` would make the app confidently wrong about whether the install worked.

Resolve it by running the user's login shell once: `$SHELL -l -c 'printf %s "$PATH"'`, following the existing one-shot `tokio::process::Command` practice already used for `nginx -t` and `php -i` (golden rule 4 governs *supervised* processes; this is a query). Bound it with a **2 s timeout** — a slow or interactive-hostile profile is a well-known hang in tools that do this, and hanging a menu action is unacceptable.

The result is **three states, not a boolean** — this codebase has collapsed a state into a bool four times and paid for it every time:

- `OnPath` — the chosen directory appears in the login shell's PATH. Report "you can run `openvhost` now."
- `NotOnPath` — resolved successfully and the directory is absent. Report the exact `export` line to add, and **which file** to add it to, derived from `$SHELL`.
- `Unknown { reason }` — the probe failed or timed out. Show the `export` line **as a precaution**, and say plainly that we could not check. Never render "you're all set" on a guess.

**We do not edit the user's shell profile.** Silently appending to `.zshrc` is how tools break people's shells, and an undo is not offered by anyone who does it.

### D5 — Install state is a state, not a checkbox

```
InstallState {
    NotInstalled,
    Installed { dir, path_status },        // our symlink, target resolves
    Broken { dir, reason },                 // our symlink, target gone — app moved or deleted
    Blocked { dir, what_is_there },         // occupied by something that is not ours
}
```

`Broken` is the case a boolean would hide, and it is not hypothetical: the user drags the app to the Trash, or renames it, and the symlink dangles. The menu item's label reflects the state (`Install…` / `Reinstall…`), and the dialog for `Broken` offers to repoint it.

### D6 — The action lives in the app menu

**OpenVHost → Install Command Line Tool…**, placed after About and before Settings-adjacent items, with a separator. Reasons: the app menu is already ours (built in `quit.rs::app_menu`, because macOS ⌘Q is not interceptable), there is no Settings route today and adding one for a single action is overkill, and "Install helper/command line tool" in the app menu is the idiomatic macOS placement.

Result is reported with `tauri-plugin-dialog`, already wired and already used this way by the tray. The dialog states the directory, the PATH verdict from D4, and the `export` line when one is needed.

**Tradeoff, stated:** a menu item is less discoverable than a Settings page row. Accepted for now; when a Settings route lands, this becomes a row there and the menu item stays as an accelerator.

**No new Tauri command.** The menu handler calls the Rust install logic directly, exactly as the tray's handlers do. The webview gets no new surface.

### D7 — `__testchild` is gated out of release builds

The CLI carries a hidden `__testchild` fixture exposing `--http <port>` (binds a listener), `--spawn-child` (re-execs detached), `--probe-state <path>` (writes a counter and a `.pid` to an arbitrary path) and `--ignore-stop`. Harmless in a dev tree; an undocumented capability the moment this slice puts the binary on a user's PATH. The security audit of PR #40 flagged it as exactly this — a packaging concern.

`#[cfg(debug_assertions)]`-gate the interception so release builds do not carry it. **Verify first:** despite a comment suggesting otherwise, nothing in the workspace spawns `openvhost __testchild` — every supervisor test uses `CARGO_BIN_EXE_proc_testchild`, a separate binary. Check that before removing anything, and check whether the debug-only `demo-ticker` service references it.

This closes the follow-up chip; it is in this slice because this slice is what makes it matter.

### D8 — Not in scope, with reasons

- **Privilege escalation of any kind.** Unnecessary (see the table), and the honest mechanism is the Phase 3 privileged helper.
- **A Homebrew formula or cask.** The right long-term distribution channel, but it needs a signed, notarized, downloadable release. Phase 3.
- **Editing shell profiles.** D4.
- **An uninstall action.** `rm` on a symlink whose path we print is a complete answer; a second menu item for it is noise. Revisit if the Settings route lands.
- **Windows.** Project-wide macOS-first. The non-macOS arm reports unsupported, mirroring the existing `#[cfg]` stub shape.
- **Versioned shims** (`php`, `php-8.3` on PATH) — a separate Phase 2 roadmap line, unrelated to this binary.

## Testing

- **Pure:** candidate ordering; the clobber decision table over every node type (absent / our symlink / foreign symlink / regular file / directory), asserted exhaustively; PATH membership parsing including a trailing colon, an empty element, a duplicate, and a `~`-relative entry; `InstallState` classification for all four variants; the `export` line and profile filename derived from `$SHELL` for zsh, bash and an unknown shell.
- **Real filesystem, tempdir:** install into an empty dir; install twice is idempotent and does not churn the inode; a foreign symlink and a regular file are both refused **and left byte-identical**; a dangling symlink is classified `Broken` and repaired; the temp-then-rename leaves no residue on a failure injected between the two steps.
- **Vacuity proof required per group**, per the standing rule. The refusal tests are the ones to be most suspicious of — assert the *existing file is unchanged*, not merely that a `Result` was `Err`.
- **Live proof:** build the app, run the menu action, then open a **fresh** terminal and run `openvhost list` with no absolute path. Then move the app, confirm `Broken` is detected and repairable. Then confirm a release build's `openvhost __testchild` is gone while `proc_testchild` still works.

## Security posture

This is **the first thing the app writes outside `<home>`**, and it writes into a directory on the user's PATH — a location where replacing a file is code execution. **security-auditor review is a merge blocker** (golden rule 2; `packaging/**` also lists security-auditor as reviewer for signing/verification steps in plan §6.2).

Claims to verify: nothing outside the two candidate directories is ever written; no path traversal is reachable from any input (there is no user-supplied path in this feature at all); the clobber rules cannot unlink a node we did not create; the temp-then-rename cannot leave a partial install; the login-shell probe is bounded, inherits no attacker-controlled environment, and its output is never executed or interpolated into a command; `__testchild` is genuinely absent from a release binary; and the symlink target is derived from `current_exe()` rather than anything a caller supplies.

## Verification owed to a human

1. Menu shows **Install Command Line Tool…**; run it; the dialog names the directory and the PATH verdict.
2. Open a **new** terminal window; `openvhost list` runs with no path prefix.
3. Run the action again — reports already installed, changes nothing.
4. Quit, move the app, relaunch: the item reads **Reinstall…**, and running it repairs the link.
5. Put your own file at the chosen path; the action refuses and names it, and your file is untouched.
