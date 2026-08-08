<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The first screen tells you what a site actually needs

**Status:** design, ready to plan.
**Date:** 2026-08-08.

## 1. Half the requirement is announced; the other half is silent

Sites is the landing page — `Rail.svelte`'s own comment says *"`/`, not `/sites`"* — so it is the
first screen a new user sees. With nothing installed it says:

> **No sites yet**
> Add a site to serve a project folder at a `.localhost` domain.

and, since an earlier slice, a banner:

> **No PHP version is installed yet** — Sites need one to run. *Install a version on the Languages
> page.*

That banner's own comment states the intent: *"this is where a first-time user — or one who has
never installed PHP — lands first."* Good. **But the Sites page mentions nginx exactly zero
times**, and serving a site needs both.

Since nginx discovery (slice 4B) made absence representable — `nginx_bin: Option<PathBuf>`, and
`fallback_brew()`'s invented path deleted — **"no nginx" is a real state a real machine can be
in.** On such a machine, with PHP installed, the landing page shows **no banner at all**, invites
the user to add a site, and the site does not serve. Nothing on the page that issued the invitation
explains why.

That asymmetry is the whole slice: **one missing requirement is announced, the other is silent.**

## 2. Measured on `d7b00a2`

| Fact | Consequence |
|---|---|
| `routes/+page.svelte:67` already derives `noPhpInstalled`, gated on `phpEnvKnown` | The discipline to copy: a banner that claims absence **before looking** is worse than none. `phpEnv === null` means "not looked yet", not "nothing there" |
| `routes/+page.svelte:171` renders a **separate** `php-env-error-banner` | A failed read and a genuine absence are different claims, and an earlier fix (I2) exists precisely because they were once conflated. That separation must survive |
| `grep nginx routes/+page.svelte` → **0 hits** | The gap |
| `WebServerDto.binary_path: Option<String>` and `source: Option<NginxRuntimeSourceDto>` are already on the wire (`commands.rs:1204`) | **This slice is frontend-only.** Absence is already expressible; the page simply never asks |
| `nginx_bin: Option<PathBuf>` (`stack.rs:68`), `fallback_brew()` deleted in 4B | "No nginx" is not hypothetical |

## 3. D1 — One readiness banner, not two stacked

The PHP banner is **replaced**, not joined. Two info banners on a first run is noise, and they
would have to be read together to answer the one question the user actually has — *can I serve a
site yet?*

One banner, naming everything missing, with a link per remedy. With only PHP missing it must read
as it does today; the existing wording is good and is not the thing being fixed.

## 4. D2 — Absence and ignorance are different, for nginx too

`phpEnvKnown` exists because `phpEnv === null` is ambiguous between "the read has not returned" and
"nothing is installed", and stating the second while the first is true would flash a false claim on
every page load.

**nginx gets the same treatment.** Until the web-server list has returned, the page says nothing
about nginx. This is not symmetry for its own sake: it is the same defect, and this UI has shipped
the "a value that cannot express *I have not looked*" bug repeatedly.

## 5. D3 — A failed read is not an absence

`php-env-error-banner` exists because a failed `phpEnvironment()` once rendered the same
"nothing installed" claim as an empty one — *"a false claim about the machine, stated as fact"*.

That distinction survives, and extends: if the web-server read fails, the banner must not say nginx
is missing. Missing and unknown are different, and the page has been wrong about this before.

## 6. D4 — What this is deliberately NOT

**Not a setup wizard.** ServBay's answers a twelve-package surface and, at its centre, an
administrator-password prompt that installs their privileged helper. We have neither: our surface
is nginx + PHP + one database, and our helper is Phase 3, blocked on plan §7 OQ#5 (a helper's
`SMPrivilegedExecutables` requirement pins a Team ID, so it cannot be registered without an Apple
Developer identity).

**Not a role picker, and not an install-everything button.** Installing from here would duplicate
the Languages and Databases pages, which already own that flow with live output and error states.
Linking is honest; a second install path is a second thing to keep correct.

**Not a database check.** A site serves without one. Saying otherwise would make the banner cry
wolf on a machine that is fine.

## 7. What this slice must prove

1. **No nginx, PHP installed** → the banner appears and names nginx. Today: nothing at all.
2. **No PHP, nginx installed** → reads as it does today. The existing wording is not the bug.
3. **Neither installed** → **one** banner naming both, not two stacked.
4. **Both installed** → no banner. Every developed machine today.
5. **Before either read returns** → no banner, and no flash of one. The `phpEnvKnown` discipline,
   extended.
6. **Either read fails** → the error banner says so, and the readiness banner does **not** claim
   the failed side is missing.
7. Every existing Sites-page test passes **unmodified**, or the change is a finding.

## 8. Out of scope

Installing anything from this page (D4) · a wizard or role presets (D4) · databases (D4) ·
the privileged helper and everything behind OQ#5 · onboarding for a *second* run — this is about
what the first screen states, not about remembering that it was seen.
