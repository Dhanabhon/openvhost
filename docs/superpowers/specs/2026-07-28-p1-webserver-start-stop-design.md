<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Start/stop on the Web server page — Design

**Date:** 2026-07-28
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 — the follow-up the nginx-settings spec (§11) deliberately left out

## 1. Goal

Press Start on the Web server page and nginx runs. Whatever happens next, the page
never leaves the user at a dead end.

Success criterion:

1. Open **Web server** on a machine where nginx is `stopped`.
2. Press **Start**.
3. Either nginx runs and the pill says so, or the row states — in nginx's own words —
   why it did not.

## 2. Starting position

The page reports `stopped` and offers no way to change that. That is the friction that
put start/stop on the Languages row, and the reason this slice exists.

Nothing new has to be built to make it work. `stack.rs` already registers nginx as a
supervised service at launch, spawning it with `-c <home>/config/generated/nginx/nginx.conf`.
`start_service` / `stop_service` already exist as IPC commands. `servicesStore.start` /
`.stop` already wrap them. The page already receives the shared services snapshot and
derives this row's state through `statusFor`.

**This slice is wiring and guarding, not machinery.** Almost all of its weight is in what
happens when starting does not work.

## 3. The control

The same state-to-verb mapping `ServiceRow` already uses, so a user who has seen the
Services page already knows this control:

| Service state | Control |
|---|---|
| `stopped` | **Start** |
| `failed` | **Retry** |
| `running`, `starting` | **Stop** |

It sits in the row's existing action group, **first — to the left of Show config and
Validate.** Those two are diagnostics; this is the action the page is for.

### 3.1 An unknown state is not a stopped state

`statusFor` returns `null` when the services snapshot has not arrived, which is true for
the first frame of **every** visit to this page — the route fires `store.load()` and the
shared subscription resolves afterwards.

`null` renders **no control at all**, not a Start button. Falling back to Start would be
the page asserting nginx is stopped before it has asked, and the user would be one click
from starting something they were never shown the state of. The pill already handles this
correctly by rendering nothing; the control follows the same rule.

## 4. The guard: no config yet

A fresh install has no generated config. `provision_home` creates directories and seeds
the welcome page but writes no config at all — a fact pinned by
`provisioning_no_longer_writes_any_config`. So `<home>/config/generated/nginx/nginx.conf`
does not exist until the first Apply, and nginx started against a missing `-c` file exits
immediately.

`list_web_servers` gains **`config_exists: bool`** — one `Path::exists()` where it already
builds `config_path`. When it is false, Start renders **disabled with a one-line reason
naming the next step**, rather than hidden:

> No config generated yet — apply your changes first.

This follows the decision already taken for the Phase 3 port and SSL fields: a missing
control looks like an oversight, a disabled one with a reason tells the user the product
knows. It is also the existing rule that this codebase does not render a control for
something that is not wired.

**`config_exists` reports existence, not validity.** It answers one question — is there a
file at the path nginx will be pointed at — and the copy must not imply more.

## 5. The backstop: nginx's own words

§4 is a courtesy based on last-known state. It is not, and cannot be, a guarantee:

- The config can exist and still be refused — a directive nginx rejects, a port already
  bound, a permission problem, a docroot that has since been deleted.
- The file can vanish between the page loading and the user clicking. The page holds
  `config_exists` from a read that has already happened.

So when the service enters `failed`, **the row renders `stderrTail` verbatim**, exactly
as `ServiceRow` already does for a failed service. Without this the user presses Start,
the pill flips to `failed`, and the page says nothing about why — which is the dead-end
shape this project keeps rediscovering.

This is the part of the slice that earns its place. The guard prevents one predictable
failure; the backstop covers every unpredictable one, and there are more of those.

## 6. Closing the 502

A PHP site needs nginx **and** a php-fpm pool. A user who starts nginx here and opens
their site gets a 502 with nothing on screen connecting the two.

The page reads its sites (`list_sites`, already used by the Sites page) to learn which PHP
majors are in use. When **nginx is running** and a pool an **enabled** site requires is
not running, the page states it in one line and points at Languages, where start/stop for
pools already lives.

- Only **enabled** sites count. A disabled site's pool is not needed and warning about it
  would train the user to ignore the line.
- The mapping is `php-fpm-<major>` from `SiteDto.phpVersion`, matching how `php_fpm_spec`
  builds the id. The implementation must confirm `phpVersion` holds the major that id is
  built from rather than assuming it.
- It is **not** a blocker and **not** an automatic start. This row controls the service it
  names; anything else would have the Services page showing state changes with no visible
  cause.

## 7. Decisions taken (owner, 2026-07-28)

1. **Disabled with a reason** when no config exists, over letting the user discover the
   failure by hitting it.
2. **nginx only, plus a warning when pools are down** — over controlling only nginx and
   staying silent, and over starting the whole stack from one row.

## 8. Error surface

| Condition | What the user sees |
|---|---|
| Services snapshot not yet arrived | no control, no pill (§3.1) |
| No generated config | Start disabled, with the reason and the next step |
| Start rejected by the supervisor | the existing page-level error banner |
| Service reaches `failed` | nginx's `stderrTail`, verbatim, on the row |
| nginx running, a required pool is not | one line naming the version, pointing at Languages |

## 9. Testing

**The control.** Each state renders its own verb; `null` renders no control at all — that
last one is the regression most likely to be introduced by a refactor that treats
"not running" and "not known" as the same thing.

**The guard.** `config_exists: false` disables Start and states the reason; `true` leaves
it live. A Rust test covers the field itself: present file → true, absent → false.

**The backstop.** A `failed` service renders its `stderrTail`. This must assert the
stderr **content** reaches the DOM, not merely that a failure block exists — a block with
an empty `<pre>` would pass the weaker assertion and tell the user nothing.

**The 502 warning.** Fires for an enabled site whose pool is stopped; does not fire for a
disabled site, and does not fire when nginx itself is stopped (the user has not asked to
serve anything yet).

**Every guard gets a vacuity check** — break it, watch the specific test fail, restore.

## 10. Out of scope

Restart and Reload — Apply already restarts what it needs to, and nothing has asked for a
manual reload · starting php-fpm pools from this page (§6) · Apache, which is not a
supervised service · the structural gap that `save_web_server_settings` stores values
without asking nginx whether it accepts them, which is its own decision and already
recorded.
