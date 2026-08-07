<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The Web server page reports which nginx is running (off-Homebrew slice 4C)

**Status:** design, ready to plan.
**Date:** 2026-08-07.
**Follows:** 4A (#57, the build) and 4B (#58, discovery). The app now prefers a packaged nginx
and can say it has none — and cannot tell anyone which one it chose.

## 1. Goal and the boundary

4B introduced `NginxRuntimeSource { Packaged { version }, Homebrew }` and the audit's L2 finding
was blunt: **nothing in production reads it.** The only reader is an `eprintln!`. On a machine
with both a packaged and a Homebrew nginx, a user has no way to learn which one is serving
their sites.

This slice makes the enum a decision rather than a field.

**Out, and this is the agreed slice boundary, not a deferral I am inventing:** installing nginx
from the UI. The programme's slice list has C as *"Start actually runs the packaged nginx; UI
reports the source"* — the first half shipped in 4B, this is the second. An Install affordance
belongs with the release, which the owner has deferred.

## 2. Measured on `53829db`

| Fact | Consequence |
|---|---|
| `WebServerDto` carries `binary_path`, `version`, `service_id`, `config_path`, `supported`, `supports_hot_reload` — and **no source** | The field to add is one, and its shape already exists |
| `MysqlRuntimeSourceDto { Packaged { version }, Homebrew }` (`mysql_pkg.rs:65`) with a `From<&MysqlRuntimeSource>` is 16 lines | Transcribe it; do not invent a second shape |
| MySQL's source **does** reach a DTO (`commands.rs:3849`); nginx's reaches only `stack.rs:795`'s `eprintln!` | This is the asymmetry to close |
| `version` is obtained by **executing the binary** (`probe_nginx_version`), for both sources | For a packaged nginx that spawn is unnecessary — see D2 |

## 3. D1 — `NginxRuntimeSourceDto`, transcribed from MySQL's

`Packaged { version }` and `Homebrew`, with a `From<&NginxRuntimeSource>`, matched
**exhaustively — never a wildcard**. Add `source: Option<NginxRuntimeSourceDto>` to
`WebServerDto`.

`None` means "no nginx", which 4B made representable, and it is also what the Apache row
carries — that row is `supported: false` and has no runtime at all. One `None`, two honest
readings; the row's `supported` flag already distinguishes them for any consumer that cares.

## 4. D2 — A packaged nginx's version comes from the tree, not from a process

This is the substance of the slice, not a side effect.

4B's D1 said the version asymmetry "is the point of the enum, not a detail": for a packaged
runtime the exact version came for free from the catalogue and the directory name, and only a
Homebrew runtime has to be asked. **Nothing consumed that.** Both sources still execute the
binary to learn what it is.

So: when the source is `Packaged { version }`, report that version and **do not spawn**. Probe
only for `Homebrew`.

Two reasons beyond tidiness:

- It removes a process spawn from the page-load path, and *never probing a binary to learn its
  version* is the one property this project measured ServBay as doing better. Our own
  `mysqld --version` probe is what produced audit finding F1.
- A probe can fail — a corrupt binary, a Gatekeeper stall — and produce `version: None` for a
  package whose version we knew all along. Reporting "unknown" about a thing we named is the
  kind of small dishonesty that accumulates.

**Prove they agree.** A test must show that for a real packaged nginx the tree-derived version
and what the binary actually prints are the same string. If they can disagree, the tree is
lying and that is worth knowing now.

## 5. D3 — The badge says where it came from, not how it is doing

Status already has a pill. The source is provenance, not health, and must not be styled as a
second status — a green "Packaged" badge next to a red "Failed" pill would read as a
contradiction rather than two facts.

MySQL's rows already render a source badge; **match that treatment** rather than inventing a
second visual language for the same idea one page over.

## 6. What this slice must prove

1. With a packaged nginx, the row reports `Packaged` and the version **from the tree**, with no
   `nginx -v` spawned.
2. With Homebrew, the row reports `Homebrew` and the probed version.
3. With neither, the row reports no source and no version, and says so honestly.
4. The tree-derived version and the binary's own answer agree for a real packaged nginx.
5. Apache's unsupported row is unchanged.
6. **Nothing else on the page moves.** Status, start/stop, paths, config and settings behave
   exactly as before.

## 7. Out of scope

Installing nginx from the UI · retiring the brew paths (slice 7) · PHP (slice 5) · the
symlinked-version-directory hardening recorded against the shared MySQL/MariaDB/nginx helper ·
the GPG-extraction precondition recorded against the PHP recipe.
