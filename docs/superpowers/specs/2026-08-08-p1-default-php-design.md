<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The default PHP is chosen, not inherited from a sort order

**Status:** design, ready to plan.
**Date:** 2026-08-08.
**Owner direction:** follow ServBay's model where it is proven (2026-08-01). This is one of the
places it is proven, and we measured it rather than assumed it.

## 1. Today the catch-all serves your oldest PHP, by accident

`render_set` picks the default upstream with one expression (`site/apply/mod.rs:164`):

```rust
let default_upstream = match input.runtimes.php.first() { … }
```

and `discover_php` ends with `runtimes.sort_by(|a, b| a.major.cmp(&b.major))`.

So **a sort written for display order is being borrowed to make a runtime selection.** With 8.1 and
8.3 installed, `localhost:8080` serves **8.1**. Nobody chose that; it fell out of a `sort_by`.

Two consequences already recorded and never fixed: 5B §7 flagged it as an owner decision, and the
5B audit measured that the sort is a **byte-lexicographic `String` compare**, so once a
double-digit minor exists the order is not even "oldest" — `8.9`, `8.10`, `10.0` sorts to
`["10.0", "8.10", "8.9"]`.

## 2. What ServBay does — measured on this machine, not recalled

```
/Applications/ServBay/package/php/8.2/8.2.30/          <major>/<version>/
/Applications/ServBay/package/php/8.2/current   -> …/8.2/8.2.30
/Applications/ServBay/package/php/current       -> …/8.2/current
```

The per-series `current` is the same shape we designed independently.

**The first draft of this section then got the evidence wrong, and the correction is the useful
part.** I read the second-level `package/php/current` as the model for our web catch-all. It is
not. Its only consumer is `script/alias/servbay-php`, whose own header says it *"provides
project-level PHP version management… uses the specified PHP version for executing PHP
command-line tools like php, composer, php-fpm, pecl… If no configuration is found, it defaults to
ServBay's default PHP installation."* That link is the **CLI** default.

ServBay keeps **three separate mechanisms**, and the first draft collapsed them into one:

| mechanism | answers |
|---|---|
| `package/php/current` + a per-project `.servbay.config` | which `php` you get in a terminal |
| `enable-php-fpm-default.conf` → `unix:…/tmp/php-cgi.sock` | which PHP serves the **web** default |
| `php-cgi-<version>.sock` | per-site |

**Their web mechanism is an unversioned socket**, so the generated config never names a version —
whichever pool is the default binds `php-cgi.sock`.

**We deliberately do not copy that**, and the design below is unchanged because the reasons are
independent of the mistake:

- A config that **names the socket** can be read to learn which PHP serves the catch-all. Theirs
  requires knowing which process bound an unversioned name.
- Changing the default then flows through **diff preview → validate → rollback**, the pipeline this
  app already has and treats as a differentiator. An unversioned socket changes hands with nothing
  to preview.
- An unversioned socket is ambiguous if two pools race for it.

Also measured, and the basis of the whole off-Homebrew programme: ServBay's `php-fpm` has **zero
Homebrew references** — it links only against `/Applications/ServBay/package/common/lib/…` (a
shared 162 MB tree) and `/usr/lib`.

## 2b. Where "follow ServBay" stops — measured, not assumed

Their public build pipeline (`ServBay/ServBay-PKG-Builder-v1`) fetches with
`curl -L --fail -o "$destination" "$url"` — **no `--proto`, no digest comparison, no signature
check anywhere in `build_package`** — and produces **no checksum or manifest** alongside the
artifacts it builds. Their package registry (`runtime/packages.conf`) is four tab-separated
fields: name, version, x86_64 filename, arm64 filename. **There is no field a digest could go in.**

Scope of that claim: I read the *build* link, not the client. It is possible their app verifies on
download. What is directly observable is that this pipeline **produces nothing for a client to
verify against**.

This is the same finding slice 5A made about `static-php-cli`. Two unrelated supply chains in this
domain, neither verifying.

**So the owner's 2026-08-01 direction splits in two.** Follow ServBay's **product** model — own the
package tree, `<major>/<version>/` with `current`, several versions side by side, an explicit
default. Do **not** follow their **supply chain**: we pin 41 sources with digests, GPG-verify
php-src through `--status-fd`, build under a network-denied sandbox, ship a `.sha256` and a
manifest per artifact, and verify before extraction. Golden rule 6 requires it independently.

## 3. D1 — A stored preference, not a symlink

ServBay can use a symlink because it owns every runtime it offers. **We cannot**: our majors may be
Homebrew kegs, and a brew-only machine has no package tree for a link to live in.

So the default is a **singleton row**, mirroring `web_server_settings` exactly — `0006_php_settings.sql`,
`CHECK (id = 1)`, `STRICT`, with `updated_at`.

**Not** a field on `WebServerSettings`. That struct's own doc says it is "editable **nginx**
settings: connection limits, timeouts, upload size, and compression", every field is an nginx
directive, and the apply pipeline carries a comment that the main config is the only file those
settings touch. "Which PHP is default" is none of those things.

**Not** a generic key-value settings table. This codebase has consistently chosen typed rows, and
one precedent plus one new need is not proliferation.

## 4. D2 — The preference is a preference; resolution is explicit and can fail

A stored major can stop being installed — the user uninstalls it, or a keg disappears. **That state
must be representable, not silently collapsed.** This project has now shipped four defects of
exactly that shape: a boolean that could not express `Failed`, an offer union that could not
express `awaitingRelease`, a `fallback_brew()` that invented a path, and a `brewFound` bool
answering a per-major question.

So: store the preference, resolve it separately, and let the resolution report **why** it did not
get what was asked. The catch-all still has to serve something, so an unresolvable preference falls
back — but the fallback is a named state, not an absence nobody can see.

**What the fallback is, is D3.**

## 5. D3 — With no preference, behave exactly as today

Every existing machine has no row. On those, the catch-all must serve **precisely** what it serves
now, including the string-sort quirk. This slice is inert until someone sets a default.

That is deliberate and it is the property to test hardest: the last four slices each claimed "no
real machine changes today" and each proved it three ways.

**The string-sort quirk is not fixed here.** It stops mattering the moment a preference is set, and
changing the no-preference ordering in the same slice would make "nothing changes today" false.
Recorded in §8 as a separate call.

## 6. D4 — The Languages page is where a major is marked default

That page already lists every major with its source, version and install state. Marking one as the
default belongs on the row that represents it — the same reasoning that put the offer on the row in
5C rather than on the page.

A machine with **one** PHP needs no affordance at all: the answer is not in doubt. Whether the
control appears at all when `runtimes.len() == 1` is a judgement for the implementer to make and
report, not a spec decision.

## 7. What this slice must prove

1. With a preference set to an installed major, the catch-all serves **that** major — proven
   through the generated default-site config, not through the store.
2. With **no preference**, the generated config is **byte-identical** to today's, on the same
   inputs. Every real machine right now.
3. A preference naming a major that is **not installed** is reported as such, and the catch-all
   still serves something — no panic, no empty upstream, no silent substitution.
4. Uninstalling the default major leaves the preference **legible**: the app can say "your default
   was 8.4, which is no longer installed", not just quietly serve 8.1.
5. The preference survives a rescan and a restart.
6. Setting a default goes through the **existing apply pipeline** — diff preview, validation,
   rollback — like every other change that rewrites a config. It is not a side-door write.
7. Exhaustive matching on any new state; a throwaway variant must fail to compile.

## 8. Out of scope

Making the no-preference order numeric rather than byte-lexicographic (§5 — it stops mattering
once a preference exists, and doing it here would break claim 2) · a default for MySQL or MariaDB ·
ServBay's shared `common/` tree, which is a build-strategy question and not this ·
slices 6 and 7, both of which are downstream of a release that is deferred.

**Recorded:** ServBay resolves its default *per series as well* (`8.2/current` → a version). We
already do that in the package tree; this slice is only about which **major** the catch-all uses.
