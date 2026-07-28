<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# php-fpm service state on the Languages page — Design

**Date:** 2026-07-28
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 — brings the Languages row up to what the Web server row does

## 1. Goal

A php-fpm pool that fails to start says so, in php-fpm's own words, and offers Retry.
Nothing on the row asserts a state it has not checked.

Success criterion:

1. Break a pool's config so it cannot start.
2. Press **Start** on its Languages row.
3. The row shows **Retry** and php-fpm's own error — not a **Start** button that looks
   exactly like it did before the attempt.

## 2. Starting position

Start and Stop already work on this page, and the install surface around them is thorough:
brew's exit code, the "exited 0 but the version was not found" case, and a live log pane are
all rendered and were audit-hardened during the PHP versions slice.

The *running* surface has none of that.

## 3. Root cause: a boolean where a state belongs

`isRunning` in `routes/languages/+page.svelte` maps four supervisor states onto two values:

| state | `isRunning` |
|---|---|
| `running` | true |
| `starting` | true |
| `stopped` | false |
| **`failed`** | **false** |

A failed pool is therefore indistinguishable from a stopped one. The row shows **Start**; the
user presses it; it fails; the row shows **Start** again. The only feedback the product gives
is that nothing changed.

It is also structurally unfixable in the row as written: `LanguageRow` receives
`running?: boolean`, so it cannot see `stderrTail` even if it wanted to.

Everything in this slice follows from undoing that collapse.

## 4. The prop change

`running?: boolean` becomes `serviceState: ServiceStatus['state'] | null`, the same shape
`WebServerRow` took in commit `3741d49`. `isRunning` is **deleted rather than repaired** — it
is the collapse, not a faulty implementation of something worth keeping.

`null` means the supervisor snapshot has not arrived, or this row has no pool (not installed).

## 5. The control

| State | Control |
|---|---|
| `stopped` | Start |
| `failed` | **Retry** |
| `running`, `starting` | Stop |
| `null` | **nothing at all** |

The `null` row is the one that changes behaviour beyond the failure case. Today it renders
**Start**, which claims the pool is stopped before the supervisor has answered — the first
frame of every visit. The Web server row already refuses to do this; this one now matches.

### 5.1 The not-installed branch still comes first

`null` arises for two unrelated reasons: the row has **no pool** (PHP is not installed), and the
snapshot **has not arrived** for a pool that does exist. Both render no service control, but they
are not the same row.

The existing `{#if !row.installed}` branch, which renders **Install**, keeps its place ahead of
everything in the table above. So a not-installed row is unaffected by this slice, and the table's
`null` case only ever describes an **installed** row whose state is not yet known. Reversing that
order would replace the Install button with nothing on exactly the rows a new user needs it.

## 6. The backstop

A `failed` pool renders `stderrTail` **verbatim**, and **still announces the failure when the
tail is empty** — a pool killed by a signal must not render identically to a healthy one.

Verbatim, because a php-fpm startup error names the pool file and the directive that broke;
summarising it would throw away the part that fixes the problem.

### 6.1 Two different things called "failed"

The row already binds `failed`, and it means **brew's install failed**. Adding a service
failure with the same name in the same component is how the two get crossed — and they render
in different places, so crossing them would put an install error where a runtime error belongs.

- The install one is renamed **`installFailed`**.
- The service one is read off `serviceState.kind` at the point of use and is **never given a
  bare `failed` binding**.

## 7. The status pill

The row has no status pill at all today — state can only be inferred from the button's verb.
`StatusPill` is added in its own track, the same component Services and Web server use, and
renders nothing when `serviceState` is `null` (the same `{#if}` guard `WebServerRow` uses).

Without it, `starting` is invisible and a running pool looks the same as one nobody has
checked.

## 8. The full-version column is removed

That column shows an em dash on **every** row, installed or not, because no patch-level prober
exists. To a reader that is not "absent data" — it looks like data that failed to load.

Its track is replaced by the pill's. Track **count** is unchanged, so the grid keeps its shape;
the width goes from `90px` to `120px`, matching the pill track `ServiceRow` and `SiteListRow`
already use, so the three rows line up rather than each choosing a number.

**The `fullVersion` field itself stays.** The install-success message still reads
`Installed PHP {row.fullVersion ?? row.major}`, where it is genuinely useful and degrades
honestly. Only the column goes.

## 9. What this slice deliberately does not touch

The install surface — brew exit codes, the not-detected case, the log pane, the install button's
re-entrancy guard. It is already thorough, it was hardened by a security audit, and this slice
adds the running surface *beside* it rather than reworking it.

## 10. Error surface

| Condition | What the user sees |
|---|---|
| Snapshot not yet arrived | no control, no pill |
| Not installed | the Install button, as today |
| Pool stopped | Start |
| Pool failed | Retry, plus php-fpm's stderr verbatim |
| Pool failed with no stderr | Retry, plus a statement that it failed |
| Start/Stop rejected by the supervisor | the existing app-level error banner |

## 11. Testing

**The control.** Each state renders its own verb, and `null` renders no control at all — that
last one is the regression a future refactor is most likely to reintroduce, because "not
running" and "not known" read as the same thing in a diff.

**The backstop.** A failed pool's stderr **content** reaches the DOM — asserting only that a
failure block exists would pass for an empty `<pre>`, which tells the user nothing. And the
empty-tail case still announces.

**The two failures stay apart.** A brew install failure and a service failure render their own
messages and do not appear in place of each other. This is the test that pins §6.1.

**The pill.** Present for a known state, absent for `null`.

**Every guard gets a vacuity check** — break it, watch the specific test fail, restore.

## 12. Out of scope

Uninstalling a PHP version · a patch-level prober (§8) · starting pools from the Web server
page, which deliberately only names them · anything about the install flow (§9) · per-site pool
control.
