<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# nginx discovery — packaged first, Homebrew as fallback (off-Homebrew slice 4B)

**Status:** design, ready to plan.
**Date:** 2026-08-06.
**Follows:** slice 4A (#57), which builds nginx 1.30.4 into our own package tree. The app can
now install an nginx it never looks for.

## 1. Goal and the boundary

Make the app **find** a packaged nginx and prefer it, falling back to Homebrew. That is the
whole slice.

**Out:** installing nginx from the UI, a Web-server page source badge, retiring the brew
paths (slice 7), and PHP (slice 5). **The download is still deferred** — `availability` stays
`AwaitingRelease`, so on a real machine today discovery will find no packaged nginx and fall
back exactly as it does now. This slice is what makes the *next* one possible, and it is
provable in full against a hand-built package tree.

## 2. Measured on `c87ec6c`

| Fact | Consequence |
|---|---|
| `StackPaths.nginx_bin` and `InstalledRuntimes.nginx_bin` are each assigned at exactly one place — `stack.rs:810` and `:814` | The seam is two lines, not a refactor |
| `find_brew_binaries` requires **both** nginx and php-fpm or returns `None`, but production reads only `brew.nginx`; `brew.php_fpm` is constructed and never read outside tests | The AND-coupling is vestigial. It does not couple anything — it only *degrades* nginx discovery |
| `fallback_brew()` hardcodes `/opt/homebrew/opt/nginx/bin/nginx` | On a machine with no Homebrew, the app already hands out a path to a binary that does not exist |
| `MysqlRuntimeSource { Packaged { version }, Homebrew }` exists and is matched exhaustively everywhere | The pattern to mirror is written, documented, and load-bearing |
| `packaged_mariadb_runtime` resolves `current` through `PackagesRoot`'s facade and structurally refuses a version dir that is not a direct child | Copy it exactly; do not re-spell the layout |
| `nginx -t` never resolves its own binary in production — all three call sites take it as a parameter, sourced from the same choke point | Validation follows discovery for free, with no further edits |

## 3. D1 — `NginxRuntimeSource`, mirroring MySQL's

`Packaged { version }` and `Homebrew`, matched **exhaustively everywhere, never through a
wildcard**, so a third source breaks compilation at every site that has to decide about it.

**The version asymmetry is the point of the enum, not a detail.** For a packaged nginx the
exact version comes for free — we asked the catalogue for it and the tree records it as a
directory name. For a Homebrew nginx there is nothing to ask, so it must be probed by
executing the binary. ServBay's one genuinely superior property is that *it never probes a
binary to learn its version*, and this project's own `mysqld --version` probe is what produced
audit finding F1. Encode which case is which in the type so no caller has to remember.

## 4. D2 — Packaged first, Homebrew second, and brew discovery stays

Resolve in that order. Homebrew stays a supported source through the migration — the owner is
running brew-installed services right now, and slice 7 is where they retire, not here.

**Leave `find_brew_binaries` alone.** Its AND on php-fpm is vestigial for nginx but
`openvhost-conf`'s validator and the e2e tests genuinely still need php-fpm from it. Changing
its contract to fix nginx's discovery would be a change to a function whose other callers did
not ask for one.

## 5. D3 — Absence becomes representable

Today `fallback_brew()` returns a hardcoded `/opt/homebrew/opt/nginx/bin/nginx` when discovery
finds nothing. On an Apple Silicon machine with Homebrew that path happens to be right, which
is why nobody has noticed; on a machine without Homebrew it is **a path to a binary that does
not exist, presented as the nginx path**.

That is the same shape of dishonest model this project has already paid to fix twice — the
boolean that could not express `Failed`, and the offer union that could not express
`awaitingRelease`. Both were found only after they had misled someone.

So `nginx_bin` becomes able to express "there is no nginx", and every consumer decides. There
are six: the supervisor spawn spec, three apply sites, the settings pre-check, and the version
probe.

**Scope discipline:** this slice makes absence *representable and honest*, not *well-handled*.
An honest error at each site is the deliverable. The affordance that offers to install one is
slice C.

**The Services-row consequence, recorded so the next reader does not re-derive it** (4B
fix-wave review, MEDIUM): with no nginx at all, no `"nginx"` `ServiceSpec` registers, so the
Services panel shows nothing for it — whereas the retired `fallback_brew()`'s own doc recorded
the prior contract as "rows still register, and Start yields an honest `Failed` naming the
missing path." This is intentional, and stays. It matches MySQL and MariaDB in the exact same
function (`macos_stack`): neither registers a row for an engine with nothing to start either.
And the Web Server page does not depend on the Services panel to tell the truth — it renders
its nginx row from `web_servers`, not from the supervisor's registered specs, and already
handles `binary_path: None` (this same D3) — so the user still learns the truth there, on the
page whose whole job is reporting it.

## 6. D4 — Pass `-p`, and stop relying on a property someone has to keep true

nginx resolves **relative** config paths against its compiled-in prefix. Nothing the app
generates is relative today, so this is latent rather than live — which is exactly why it
should be closed now, deliberately, rather than discovered later.

Pass `-p` on every invocation, alongside the `-e <err_log>` that is already mandatory for the
identical reason. It converts "no relative path is generated today" from a property a future
template author must not break into one nginx cannot act on. It costs one argument and changes
nothing about today's behaviour, because every generated path is absolute.

*Corrected 2026-08-06 by the security audit, which reproduced it live:* the paragraph above
originally said **`-p <home>`**, and that was wrong in a way that mattered. `<home>` holds
`state.db` — MySQL's and MariaDB's root credentials at rest — and mode 0600 does not help,
because nginx runs as the same user. Same config, same relative `root .;`, measured both ways:

| | relative root resolves to | `GET /state.db` |
|---|---|---|
| before this slice (no `-p`) | `/opt/homebrew/Cellar/nginx/…` | **404** |
| `-p <home>` as first drafted | `<home>` | **served the credential file verbatim** |

My reasoning was that nothing the app *generates* is relative — true, and insufficient. The
generated main config **invites the user to author their own nginx files**
(`main.conf.tera:72`, `include "{{ custom_sites_glob }}"`), and nothing *included* is under our
control. The slice as drafted moved the footgun's muzzle off a secrets-free package prefix and
onto the credential store.

So `-p` points at a **dedicated, empty, provisioned directory** — `<home>/run/nginx-prefix`,
computed once in `nginx::prefix::nginx_prefix_dir` — and the three sites that share a live home
share that one prefix, or `nginx -t` stops testing what actually runs. The two validators that
render into disposable scratch directories keep using those; they hold nothing.

Recording the correction rather than quietly editing it, because the conclusion ("pass `-p`")
survived and only its target changed — and a reader who saw only the conclusion would inherit
the original mistake.

Prove it: a config carrying a deliberately relative path must resolve under that prefix, and a
relative `root` must not be able to serve `state.db`.

**A second correction, from the live proof.** The claim that omitting `-p` "fails loudly" is
true of Homebrew's build and **false of the build we ship**: packaged nginx 1.30.4 exits 0
silently and writes into `/opt/openvhost-build/nginx-1.30.4/logs/`, the build host's staging
prefix baked into the tarball. So the case for `-p` is *stronger* than this section first
claimed, not weaker — the loud failure was never guaranteed.

## 7. D5 — The packaged tree ships a stock `conf/nginx.conf`, and that is fine

The 4A audit noted it: our tarball contains nginx's stock config, which listens on `:80` with
relative paths. It is unreachable because `-c` is passed on every invocation, and D4 closes
the relative-path half independently.

Recorded so the next reader does not rediscover it and assume it is a defect. **Do not delete
it from the package** — an nginx tree without its stock config is a tree that behaves
differently from every other nginx, for no gain.

## 8. What this slice must prove

1. With a packaged nginx present, the app resolves it, records `Packaged { version }`, and
   spawns from a **concrete version directory** — never through `current`, so a later swap
   cannot change which binary a restart brings up.
2. With no packaged nginx and Homebrew present, it resolves Homebrew and records that.
3. With neither, absence is reported honestly and nothing hands out a path to a file that is
   not there.
4. `nginx -t` validation and the settings pre-check follow discovery with no further wiring —
   they take the binary as a parameter today and must continue to.
5. `-p` is passed, and a relative path in a config resolves under our home.
6. **A site still serves.** Sites, apply, logs and the Web-server page behave exactly as before
   on a machine with Homebrew — this slice must be invisible until a package exists.

## 9. Out of scope

Installing nginx from the UI · a source badge on the Web-server page · retiring the brew paths
(slice 7) · PHP (slice 5) · `find_brew_binaries`'s php-fpm AND, which stays until its other
callers are addressed · the GPG-extraction precondition recorded against the PHP recipe.
