<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Editable nginx Settings — Design

**Date:** 2026-07-28
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 — turns the Web server page from a read-only inspector into a control

## 1. Goal

Let a developer change how nginx behaves — connection limits, timeouts, upload size,
compression — from inside OpenVHost, with the same diff-then-apply safety the Sites page
already has.

Success criterion:

1. Open **Web server**, raise **FastCGI read timeout** from 300 to 900.
2. Press Save, read the diff of `nginx.conf`, confirm.
3. nginx restarts on the new value, and a request paused on an Xdebug breakpoint for ten
   minutes is no longer killed at the old limit.

## 2. Starting position

The page says so itself: *"Read-only — the binary OpenVHost runs, the config it reads, and
whether that config is valid."* Every tunable is currently a literal in
`templates/nginx/main.conf.tera` — `worker_processes 1`, an empty `events {}`, and no
`client_max_body_size`, `keepalive_timeout`, `gzip` or FastCGI timeout at all, so nginx
falls back to its own defaults.

The reference for this slice is ServBay's Web Server panel, which exposes the same knobs as
a form. Two of its fields cannot work here yet, which shapes the scope (§3).

## 3. What is deliberately not in this slice

**HTTP Port and HTTPS Port.** ServBay offers 80 and 443 because it has a privileged helper.
Binding below 1024 requires root, our helper is Phase 3, and that is precisely why every site
is served on 8080 today. A port field now would accept `80` and then fail to bind.

**Everything under SSL** — HTTPS port, SSL protocol, `ssl_prefer_server_ciphers`, and in
practice HTTP/2 and HTTP/3, which need TLS to be useful. The local CA and per-site HTTPS are
Phase 3 as well.

Those fields are **shown on the page as disabled controls with a one-line reason**, not
omitted. A missing field looks like an oversight; a disabled one with "needs the privileged
helper — Phase 3" tells the user the product knows.

## 4. Architecture

| Piece | Crate | Responsibility |
|---|---|---|
| `WebServerSettings` + its newtypes | `openvhost-core::settings` | parsed values, defaults |
| `web_server_settings` table | `state.db` migration | a singleton row, the stored values |
| `generate_main_config(home, &settings)` | `openvhost-conf` | renders them; today it takes only `home` |

The table is a **singleton**: one typed column per setting, a `CHECK`-constrained `id` that can
only ever be `1`, so "which row is the real one" is not a question the code has to answer.

**A fresh install has no row, and that is not an error.** The repository returns
`WebServerSettings::default()` when the row is absent and does not write it — the first *save*
inserts. Seeding on read would mean every launch writes to `state.db` before the user has
touched anything, and a failure at that moment would surface as a startup error for a value
nobody changed.

`WebServerSettings` is a **separate struct passed alongside**, not new fields on `RenderCtx`.
`RenderCtx` means "one site"; these values are global, and mixing the two would leave nobody
able to tell which is which. `generate_site_config` never sees them.

### 4.1 There is no second pipeline

This is the part worth getting right, and it falls out of the architecture rather than needing
to be built.

`render_set` reads the settings and hands them to the main config. From there `plan()` sees
`nginx.conf` as Modified, and the diff, `nginx -t`, rollback and restart all work **unchanged**.
The Web server page is a second entry point to the pipeline the site-apply slice already
shipped, not a new path beside it.

One consequence to expect rather than be surprised by: changing `worker_connections` also
lights the pending-changes banner on the Sites page, because it is the same pending change.

### 4.2 The IPC names stop being true

`plan_site_apply` and `apply_sites` no longer cover only sites. They are renamed to
`plan_config_apply` and `apply_config` in this slice. Mechanical — the DTOs and behaviour are
untouched — but leaving them would mislead the next reader about what the pipeline covers.

## 5. Values, defaults, and why

**Defaults are development-appropriate rather than nginx's own**, and the reason is specific to
this product: **the diff preview makes changing a default safe in a way ServBay's design is
not.** A user upgrading sees `client_max_body_size 1m → 256m` in the diff and presses Apply
themselves. Without that preview the safe choice would be nginx's values everywhere.

| Setting | nginx | ServBay | Ours | Why |
|---|---|---|---|---|
| `worker_connections` | 512 | 1024 | **1024** | 512 is low even for local work |
| `client_max_body_size` | 1m | 2048m | **256m** | 1m dies on the first WordPress database import; 2 GB is theatre |
| `keepalive_timeout` | 75 | 65 | **65** | |
| `tcp_nodelay` | on | on | **on** | |
| `fastcgi_connect_timeout` | 60 | 300 | **60** | connecting should be fast; a long value here hides a dead pool |
| `fastcgi_send_timeout` | 60 | 300 | **300** | |
| `fastcgi_read_timeout` | 60 | 300 | **300** | the one that matters — a request paused on an Xdebug breakpoint routinely exceeds 60s |
| `gzip` | off | off | **off** | local work does not need it, and it makes responses harder to inspect |
| `gzip_comp_level` | 1 | 1 | **1** | only meaningful once gzip is on |
| `gzip_types` | `text/html` | a set | **ServBay's set** | |

### 5.1 Every value is written, even at its default

Consistent with two decisions this project already made — `clear_env = yes` and
`security.limit_extensions` were both stated explicitly rather than inherited, on the grounds
that a generated file should say what it means.

**The cost, stated so nobody is surprised:** the first launch after this ships produces a
pending change the user did not make. Behaviour is identical; the file gains lines. It happens
once, the diff shows exactly what it is, and the alternative — emitting a directive only when
it differs from a default — fills the template with conditionals and leaves the generated file
no longer telling the whole truth.

## 6. Validation

Every one of these values travels from the webview into a config file. That is the same
boundary a `$` slipped through in the site-apply slice, so each gets a newtype with `parse`
and nothing unparsed reaches a template.

| Field | Rule |
|---|---|
| worker connections | integer, 1–65535 |
| timeouts | integer seconds, 1–86400 |
| gzip compression level | integer, 1–9 |
| tcp nodelay, gzip | boolean → `on` / `off`; nothing injectable |
| client max body size | `^\d+[kKmMgG]?$`, nothing else |
| **gzip types** | **the only genuinely dangerous field here** |

`gzip_types` is free text, long, and lands in the config file directly. Passed through, a value
like

```
text/html; } server { listen 9999; root /; } http {
```

becomes real directives and passes `nginx -t` cleanly.

It is therefore **split into tokens and each token validated on its own** against
`^[a-z0-9][a-z0-9.+-]*/[a-z0-9][a-z0-9.+-]*$`, **at most 64 of them, each at most 128 bytes** —
generous next to the ~20 types anyone actually compresses, and small enough that the field
cannot be used to inflate the generated config. One bad token rejects the whole form,
naming the token — never a silent drop, which would leave the user with compression quietly
not covering what they asked for.

## 7. Error surface

| Condition | What the user sees |
|---|---|
| A field fails `parse` | that field marked, with the rule stated in words |
| A gzip type is malformed | the offending token named, the form rejected |
| `nginx -t` rejects the result | the existing rollback, with nginx's own stderr |
| Settings row missing (fresh install) | the defaults, not an error — first read seeds them |

## 8. UI

The Web server page keeps its inspector — binary, version, config path, validate — and gains
a settings form beneath it. Its subtitle stops saying "Read-only".

Layout follows ServBay's grouping, which is sound: connection limits, then timeouts, then
compression. Booleans are switches, numbers are number inputs, `gzip_types` is a textarea.

**Save shows the diff on that page and applies it there** (owner decision, §10). Not a
navigate-to-Sites-and-press-Apply, which would leave a Save button that visibly does nothing.
It reuses `ApplyDialog` rather than growing a second diff renderer.

Fields that need Phase 3 are rendered disabled with their reason (§3).

## 9. Testing

**Core — the newtypes.** Each rejects what it should: a non-numeric worker count, `0`, `70000`;
a body size of `256`, `256mb`, `-1m`, `256m; }`; a compression level of `0` and `10`. And
`gzip_types` rejects the directive-injection string above, naming the token — a test asserting
only "is_err" would pass for the wrong reason if the parser rejected the whole field for a
length rule instead.

**Core — defaults.** A fresh install renders the documented defaults, and that rendering is
byte-stable across two calls.

**Conf — the template.** Every setting appears in the generated `nginx.conf` at the right
scope: `worker_connections` inside `events`, the rest inside `http`. And `gzip_types` renders
only the tokens that were accepted.

**The pipeline is not re-tested.** `plan`, `nginx -t`, rollback and restart are covered by the
site-apply slice; this slice only changes what feeds them. One integration test asserts that a
changed setting produces exactly one Modified file and that it is `nginx.conf`.

**Live nginx.** The generated config with non-default values passes `nginx -t` — the existing
`validate_live` test extended, because a value that parses but nginx rejects is the failure
this whole layer exists to prevent.

**Frontend.** Each field renders its stored value; an invalid value marks its own field and not
the others; the Phase 3 fields render disabled with a reason; Save opens the diff rather than
writing immediately.

## 10. Decisions taken (owner, 2026-07-28)

1. **Only the fields that can work.** Port and SSL wait for the Phase 3 privileged helper and
   local CA, shown disabled with a reason rather than omitted.
2. **Save shows a diff and applies on the Web server page**, rather than deferring to the Sites
   page's Apply.
3. **Settings live in `state.db`**, not a third config file. The `generated/` versus `custom/`
   ownership split was hard-won; a third store with its own rules would reopen it.
4. **A separate struct, not new `RenderCtx` fields** — `RenderCtx` means one site, these are
   global.
5. **Every value is written even at its default**, accepting one behaviour-neutral pending
   change on upgrade (§5.1).

## 11. Out of scope

Port and SSL/TLS fields as working controls · HTTP/2 and HTTP/3 · Apache and Caddy settings ·
per-site overrides of any of these · nginx rewrite rules (ServBay's separate panel) ·
`worker_processes` (left at 1 until there is a reason to expose it).

**Start/stop on the Web server page** is deliberately not here, though it is the obvious next
step: the page reports `stopped` and offers no way to change that, exactly the friction that
put start/stop on the Languages row. It is a service-control concern rather than a settings
one, and this slice is already large enough to want its own review.
