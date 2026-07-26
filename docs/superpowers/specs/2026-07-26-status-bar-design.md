<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Status Bar — Design

**Date:** 2026-07-26
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 UI (see §8 for the roadmap deviation this represents)

## 1. Goal

Add a persistent one-line strip along the bottom of the main window showing, at a
glance, what OpenVHost is costing the machine:

```
services 85 MB · 2 processes · ~/.openvhost 1.2 GB
```

**Two figures**, both about OpenVHost's own footprint — the resident memory of the
services the supervisor is running, and the size of the app's home directory —
rendered as **three segments**, because the process count is a qualifier that makes the
memory figure interpretable rather than a figure of its own. This spec uses "figure"
and "segment" in that sense throughout.

## 2. Scope decision: our footprint, not the machine's

The owner's reference screenshot showed machine-wide `RAM / CPU% / Disk (limit …)`.
That was rejected in favour of our own numbers, for two reasons:

- **Machine-wide RAM and CPU duplicate Activity Monitor.** The question no other tool
  answers conveniently is "which of my local stacks is eating memory" and "how much
  disk have my installed PHP/MySQL versions taken". Those are ours to answer.
- **The reference is a container status line.** "Disk: 8.67 GB used (limit 910.74 GB)"
  is a quota readout. On a native macOS app, whole-volume free space is low signal;
  the size of `~/.openvhost` is high signal because it grows with every package
  installed and nobody notices until it is large.

**CPU% is deliberately excluded.** Resident memory is a single read per pid. CPU
percentage requires two samples separated in time plus retained per-pid state, and the
resulting number jitters. The plan already schedules per-service CPU/RAM metrics for
Phase 2; that is the right home for CPU.

**One aggregate, not a per-service breakdown.** Phase 1 still adds MySQL and MariaDB,
and a strip listing four or five services with a figure each both crowds the strip and
duplicates what a Services-page column should show. The strip shows the total; the
per-service detail belongs on the Services page, which is precisely the Phase 2 item.

## 3. Verified platform facts

Everything in this section was checked against the vendored sources and against a
running system on 2026-07-26, not recalled.

### 3.1 Reading resident memory

`libc 0.2.189` already exposes all three pieces needed — no new dependency:

| Item | Location in libc |
|---|---|
| `proc_pidinfo(pid, flavor, arg, buffer, buffersize) -> c_int` | `src/unix/bsd/apple/mod.rs:4992` |
| `struct proc_taskinfo { pti_virtual_size: u64, pti_resident_size: u64, … }` | `src/unix/bsd/apple/mod.rs:585` |
| `PROC_PIDTASKINFO: c_int = 4` | `src/unix/bsd/apple/mod.rs:3746` |

`libc = "0.2"` is already a dependency of `openvhost-proc` (`Cargo.toml:17`), and that
crate already performs raw FFI in five places (`flock`, `kill`, `waitpid`, `sysctl`,
`kill(-pgid)`), so this is the established house style rather than a new practice.

**Measured behaviour** (ctypes probe against `libSystem`, macOS, 2026-07-26):

- **`pti_resident_size` is in BYTES.** For a live process it read `14,254,080` while
  `ps -o rss=` reported `13,952 KB` = `14,286,848` bytes — a ratio of 0.998, i.e. the
  same value sampled a moment apart. It is not pages and not kilobytes; treating it as
  either would put the displayed figure off by 4096× or 1024×.
- **The return value is the number of bytes written**, observed as `96`, which matches
  `sizeof(proc_taskinfo)` = 6×u64 + 12×i32 = 96.
- **`rc <= 0` does NOT distinguish "dead pid" from "not permitted".** A nonexistent pid
  (999999) returned `rc = 0` with `errno = ESRCH`; pid 1 (launchd, which we may not
  inspect) also returned `rc = 0`. Both mean "no number available".

### 3.2 Reading the home directory's size

There is no cheap directory-size call on APFS — a total requires walking the tree.
**Measured:** `du -s` over 6,470 files completed in **40 ms** warm. That is affordable
on a slow tick; it is not affordable on the same tick as the memory read.

**The walk must not follow symlinks.** The package layout in the master plan (§ "Layout
mirrors ServBay's proven pattern") places a `current` link per major version at
`packages/<name>/<major>/current`, pointing at a sibling version directory. A
link-following walk would count every installed version twice.

## 4. Architecture

Three units, each independently testable, each with one responsibility.

### 4.1 `openvhost-proc` — `platform::process_rss`

```rust
/// Resident set size in BYTES for a live process, or `Ok(None)` when no figure is
/// available.
pub fn process_rss(pid: u32) -> io::Result<Option<u64>>
```

macOS implementation: one `libc::proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &mut ti, size)`
call into a `libc::proc_taskinfo`, returning `pti_resident_size`.

Mirrors the conventions of the existing `platform::process_start_time`
(`crates/openvhost-proc/src/platform/unix.rs:59`) exactly:

- guard `pid == 0 || pid > i32::MAX as u32` → `Ok(None)` before any FFI
- a `// SAFETY:` comment stating the invariants the call relies on
- `#[cfg(target_os = "macos")]` for the real body, with a non-macOS unix stub returning
  `Ok(None)` so the crate still compiles on Linux (the P0-8 fix-wave C6 lesson)
- treat an undersized result as no-value rather than trusting a partial struct

**`Ok(None)` and never `Err` for `rc <= 0`.** Per §3.1 the return value cannot separate
a dead pid from a permission failure, and the caller is a status strip that must absorb
the race between "the supervisor listed this pid" and "the process exited". Returning
`Err` would turn a normal, expected race into a visible failure. We only ever read our
own children, so the permission case does not arise in practice.

This is a smaller and cleaner reader than `process_start_time`, which has to read
`p_starttime` at a verified byte offset because libc exposes no `kinfo_proc`. Here
`proc_taskinfo` is a real libc struct, so there is no offset arithmetic.

`platform` is already `pub mod` in `openvhost-proc/src/lib.rs:13`, so the desktop crate
reaches this without any new re-export.

### 4.2 `openvhost-core` — `home_disk_usage`

```rust
/// Total bytes of regular files under the resolved home, not following symlinks.
pub fn home_disk_usage() -> Result<u64, CoreError>
```

Lives beside `resolve_home` in the `home` module. Walks the tree with
`symlink_metadata`, summing `len()` for regular files only. A symlink contributes
nothing and is never descended into (§3.2). A subdirectory that cannot be read is
skipped rather than failing the whole total — a partial figure beats no figure in a
status strip, and an unreadable directory is not an error the user can act on here.

### 4.3 Desktop — two commands, two cadences

```rust
services_memory() -> Result<ServicesMemoryDto, IpcError>   // { bytes, process_count }
home_disk_usage()  -> Result<HomeUsageDto, IpcError>       // { bytes }
```

Deliberately **two** commands rather than one combined `system_stats()`, because their
sampling rates differ by 30× (§5). A single command would force the cheap read to pay
the expensive one's cost or the expensive read to run 30× more often than it needs to.

`services_memory` sums `process_rss` over the live pids in `Supervisor::snapshot()`
(`ServiceStatus.pid` is already on the wire). It uses `try_state` for the supervisor,
matching the quit path's precedent: the supervisor is only managed when the setup
bootstrap succeeded, and the strip must still render without it.

**Pull, not push.** No Rust-side ticker emitting events. The UI owns the cadence, which
is what makes "stop sampling when the window is hidden" (§5) a two-line change instead
of a Rust timer that has to be told about window state.

### 4.4 Frontend

- `StatusBar.svelte` — presentational, takes formatted values plus explicit "unknown"
  states. Styling ported from `docs/design/mock.css`'s `.statusline` (flex row,
  `--vh-space-4` gaps, `--vh-text-2`, `--vh-text-caption`, values in `.num`/`.mono`),
  promoted from the log viewer's pane-level strip to window level.
- `stats.svelte.ts` — an api-injected store owning both figures, their timers, and
  their unknown states. Api-injected so its unit tests hand it a fake, matching
  `services.svelte.ts` and `sites.svelte.ts`.
- `AppShell.svelte` — `.window` becomes `grid-template-rows: auto 1fr auto` and renders
  the strip as the third row, full width beneath both rail and content. It is
  window-level state, not page state; placing it inside `.content` (the one scrolling
  region) would make it read as part of the page and would scroll with it.
- `formatBytes(n)` — pure, in a derive module, unit tested. **Specified exactly, so the
  implementation and its tests cannot disagree:** 1024-based steps labelled
  `B / KB / MB / GB` (the convention `ps` and developer tooling use, not Finder's
  decimal); one decimal place when the mantissa is below 10, none at or above it. So
  `1.2 GB`, `85 MB`, `9.4 MB`, `512 KB`, `999 B`. Zero renders as `0 MB` in the strip so
  the unit does not jump between states.

## 5. Cadence and cost

| Figure | Interval | Why |
|---|---|---|
| services memory | 2 s | one syscall per pid; feels live without churn |
| home size | 60 s, plus once at startup | 40 ms per 6.5k files (§3.2) — cheap enough to repeat, too expensive to repeat often |

**Both timers stop while the window is hidden**, resumed on becoming visible. The
master plan's first principle is "lightweight always-on … idle RAM budget for the app
itself < 100 MB. This is why Tauri was chosen over Electron"; an app left open behind an
IDE all day must cost nothing. This is also why CPU% is out (§2) and why the home walk
is not on the fast tick.

## 6. Failure behaviour

A status strip must never nag. It has no banner, no toast, and no retry button.

| Situation | Strip shows |
|---|---|
| pid exited between snapshot and read | that pid drops out of the sum; `process_count` reports only what was actually read, so the figure and its label cannot disagree |
| `services_memory` fails entirely | `services —` |
| supervisor not managed (bootstrap skipped) | `services —` |
| home walk not yet completed | `~/.openvhost measuring…` |
| `home_disk_usage` fails | `~/.openvhost —` |
| nothing running | `services 0 MB · no processes` |
| exactly one process | `1 process`, not `1 processes` — the count is user-visible copy and must agree in number, same discipline as the quit dialog's "is/are running" |

Errors are not routed to the page-level `ErrorBanner`: a strip that raises a banner
because one sample missed would be worse than a strip showing a dash.

## 7. Testing

Rust:

- `process_rss` against a **live** child — spawn `testchild`, assert RSS > 0. This is
  the test that would catch a units mistake or a wrong struct offset.
- `process_rss` for a dead pid → `Ok(None)`, not `Err`.
- `process_rss` for `pid == 0` and `pid > i32::MAX` → `Ok(None)` without an FFI call.
- `home_disk_usage` over a temp tree with a known byte total.
- **`home_disk_usage` with a symlink pointing at a large file inside the tree — the
  total must not change.** This is the test that catches the `current`-link
  double-count (§3.2); without it the bug ships silently once packages exist.
- `home_disk_usage` with an unreadable subdirectory → still returns a total.

Frontend:

- `formatBytes` at boundaries, asserting exact strings per §4.4: `0` → `0 MB`,
  `999` → `999 B`, `1024` → `1.0 KB`, `10 * 1024` → `10 KB` (the mantissa-10 switch from
  one decimal to none), `1024^3 * 1.25` → `1.2 GB`.
- Store: an api rejection leaves the figure unknown rather than zero (a zero would
  read as "nothing running", which is a different and wrong claim).
- Store: the hidden-window pause actually stops issuing calls.
- `StatusBar` SSR render: all three segments, plus each unknown state from §6.

**Every test gets a discrimination check** — apply a named mutation, confirm the test
fails, restore, confirm it passes. Specifically including: return bytes-as-KB, follow
symlinks in the walk, return `Err` instead of `Ok(None)` for a dead pid, and let a
failed sample render as `0` instead of `—`.

## 8. Roadmap position — this is a deliberate detour

`per-service CPU/RAM metrics` is listed in the master plan under **Phase 2 — Daily
Driver**, not Phase 1. Building the memory half now is a conscious deviation the owner
chose. Recorded here so the plan and the code do not silently disagree.

Phase 1 items still unbuilt: package manager UI, per-site PHP version selection (the
headline feature), MySQL/MariaDB lifecycle, live log viewer, tray quick controls,
`openvhost` CLI v1, config diff preview, uninstaller.

## 9. Out of scope, named

- **CPU%** — needs delta sampling and retained state; Phase 2.
- **Per-service breakdown** — belongs on the Services page; Phase 2.
- **Click `~/.openvhost` to reveal in Finder** — needs a new opener capability, which
  is a security-surface change deserving its own slice.
- **Windows RSS** — macOS-first per the standing project scope. The non-macOS stub arm
  in §4.1 is the seam; `GetProcessMemoryInfo` is the Windows call when that phase lands.
- **Machine-wide figures** — see §2.
