# Web Server page — design

**Status:** approved by the owner 2026-07-25. Phase 1, macOS-first.

**Goal.** A rail entry and page that answer four questions about what is actually on
the machine: which web servers OpenVHost knows, which binary it would run, from which
config, and whether that config is valid. Read-only.

**Why now.** The owner asked for "a separate Web Server menu for configuring each
brand's service details". Exploration showed the configuring half cannot be built yet —
see §2 — so this slice delivers the honest, useful half: the facts. It is also the first
slice to add IPC commands since Sites CRUD, and the first where the webview can cause a
process to be spawned, so it carries a security-auditor gate (CLAUDE.md golden rule 2)
and lands in its own PR rather than mixed into UI work.

**Be precise about when a spawn happens:** `list_web_servers` probes the version, so
merely *navigating* to this page spawns `nginx -v` — it is not only the Validate button.
That is bounded (one short-lived probe per supported brand, fixed argv, timeout) and is
called out here because "only on click" would understate the surface to the auditor. The
alternative — a separate lazy `probe_web_server_version(id)` so the page paints before
any spawn — was considered and rejected as not worth a second round trip for a two-row
page; revisit if the brand list ever grows.

---

## 1. Scope

**In.** nginx: supervised status, resolved binary path, version, hot-reload support, the
live config path, the live config's text, and a Validate action. Apache: listed, with a
plain statement that OpenVHost cannot serve it yet.

**Out, deliberately.**

- **Editing anything.** There is no settings storage: `RenderCtx` is per-site *render
  input* (`server_name`, `docroot`, `listen_addr`, `php_major`, …), not a settings
  record, and `state.db` has only a `sites` table. Editable knobs need a persistence
  surface + IPC of their own.
- **The generated-vs-live diff.** See §2 — that is the apply/diff slice, and
  `docs/design/diff-preview.html` already mocks it.
- **An `ApacheAdapter`.** Templates on disk are nginx + php-fpm only;
  `grep -rl 'apache|httpd' crates/` matches exactly one file (`site/model.rs`, the enum
  that parses the word). `openvhost-conf` has `NginxAdapter` and a `WebServerAdapter`
  trait but no Apache implementation.

---

## 2. The finding that shaped this slice

**The running nginx is not using the config the templates generate.** Two unconnected
paths exist today:

| | path | written by |
|---|---|---|
| **live** — what the supervised nginx runs | `<home>/conf/nginx.conf` | `provision_macos_demo_stack` (P0-4) |
| **generated** — what the templates produce | `<home>/config/generated/nginx/nginx.conf` | `NginxAdapter::generate_main_config` (P0-7) |

The nginx `ServiceSpec` in `apps/desktop/src-tauri/src/stack.rs` spawns with
`-c <home>/conf/nginx.conf`. The apply pipeline that would reconcile the two is deferred.

**Decision: this page shows the LIVE config.** It answers "what is nginx running right
now", which is what a page you open to debug should tell you. Showing the template output
and labelling it "your nginx config" would be false while the apply pipeline is missing.
The side-by-side comparison is the apply/diff slice's job.

**Consequence for Validate.** `materialize()` carries an explicit contract:

> `ctx.home` MUST be a throwaway validation home — this writes files into it
> NON-ATOMICALLY (plain writes, no tmp+rename). It must never be pointed at a live home;
> the apply/swap pipeline (deferred) owns atomic installation.

So `WebServerAdapter::validate` — which materializes main + site before running the
validator — **cannot** be pointed at the live home. Rather than validate a synthesized
hypothetical, this slice runs `nginx -t -c <live conf>` directly: no `materialize`, no
writes to any config, and it validates the bytes nginx is actually using.

---

## 3. Architecture

### 3.1 Domain logic — `openvhost-conf/src/inspect.rs` (new)

That crate already owns web-server knowledge (`WebServerAdapter`, `find_brew_binaries`,
native-validator plumbing) and is tauri-free, which `openvhost-core`/`-conf` must remain
(golden rule 4).

```rust
/// Version string as the binary reports it, or None when the probe fails for any
/// reason (missing binary, non-zero exit, unparseable output). A missing version is
/// not an error worth failing a whole page over.
pub async fn probe_version(bin: &Path) -> Option<String>;

/// `<bin> -e <err_log> -t -c <conf>` — validate an EXISTING config file in place.
/// Writes nothing to `conf` and never calls `materialize`.
pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError>;
```

Reuses the existing `ValidationReport { ok, stderr }`, whose `ok` is derived from the
exit code alone — never from stderr emptiness, because nginx writes informational lines
to stderr on success.

**`-e <err_log>` is mandatory, not optional.** The P0-7 validator carries the comment
*"MANDATORY: without -e, nginx leaks to /opt/homebrew/var"* — an un-flagged `nginx -t`
writes into the Homebrew prefix. `validate_live` passes `-e <home>/logs/nginx.error.log`
for the same reason.

**`nginx -v` writes to stderr, not stdout.** The parser reads stderr and extracts the
version from `nginx version: nginx/1.27.3`.

**Both spawns are bounded by a timeout.** The existing P0-7 validator uses a bare
`.output().await` with no timeout; acceptable for a test/CLI path, but a UI button that
can hang forever is not. On timeout the child is killed and the command returns an error
that renders. *(The P0-7 validator's own missing timeout is a real latent bug and is
recorded as a follow-up rather than fixed here, to keep this diff reviewable.)*

**Golden rule 4 and process spawning.** The rule says all child processes go through
`openvhost-proc`. `openvhost-conf` already spawns the P0-7 validator directly with
`tokio::process::Command` and does not depend on `openvhost-proc` — established in a
security-auditor-approved slice — so the rule is understood as governing *supervised,
long-running services*, not one-shot tool invocations. This slice follows that
precedent. **This reading is flagged explicitly for the auditor to confirm or reject
rather than assumed.**

### 3.2 IPC — `apps/desktop/src-tauri/src/commands.rs`

Three commands, all read-only:

| Command | Returns |
|---|---|
| `list_web_servers()` | `Vec<WebServerDto>` — one row per known brand |
| `read_web_server_config(id)` | `String` — the live config's text |
| `validate_web_server_config(id)` | `ValidationReportDto { ok, stderr }` |

```
WebServerDto {
  id: string                    // "nginx" | "apache"
  displayName: string
  supported: boolean            // false for apache: no adapter, no template
  serviceId: string | null      // correlates with the shared services store for status
  binaryPath: string | null
  version: string | null
  supportsHotReload: boolean
  configPath: string | null
}
```

**Security shape.**

- The client sends **only an opaque brand id**, parsed against a closed list. It never
  sends a path, a filename, or an argument. An unknown id is a validation error, not a
  fallback.
- `binaryPath` and `configPath` are derived **server-side** from `resolve_home()` and the
  supervisor's registered spec. **Nothing client-supplied reaches argv.**
- All three commands are read-only. None writes a file; none calls `materialize`.
- `read_web_server_config` is *not* a general file reader: the path is derived from the
  parsed id, so it cannot be aimed at an arbitrary file.

**Unsupported brands.** For a brand with `supported: false` the UI offers neither the
config disclosure nor Validate, so neither command is reachable from it. Both commands
nevertheless reject an unsupported id with a validation error rather than returning empty
output — a UI bug must not be able to make "Apache has no config" look like "Apache's
config is empty".

### 3.3 Two reuse decisions

**Status needs no new command.** Rows correlate to the shared services store by
`serviceId`. That store became global when the supervisor subscription was hoisted to the
layout, so the page reads live status with zero new IPC and no second source of truth to
drift.

**The binary path comes from what was registered, not a fresh probe.** The goal is the
truthful answer — the binary that would actually be spawned — and to avoid a real quirk:
`find_brew_binaries()` requires **both** nginx and php-fpm to exist and returns `None`
otherwise, so a fresh probe would report "nginx not found" on a machine where nginx is
installed but php-fpm is not.

**Correction made while planning.** An earlier draft of this section said to read
`ServiceSpec.spawn.program` back off the supervisor. That is not possible: `Supervisor`
exposes only `register`, `snapshot`, `log_tail`, `start`, `stop` and `subscribe`, and
`ServiceStatus` carries no program path — so there is no accessor, and adding one would
mean changing `openvhost-proc` for a read-only UI page.

Instead, `apps/desktop/src-tauri/src/stack.rs` — which already resolves the binaries and
config paths when it builds the specs — returns those paths alongside the specs, and the
app `manage`s them as Tauri state next to the `Supervisor`. The command then reads managed
state. This guarantees the page reports exactly the paths that were registered (same
values, same moment, no second probe that could disagree), needs no change to
`openvhost-proc`, and keeps the resolution logic in the one place that already owns it.

*(Noted, not fixed here: two separate `find_brew_binaries` implementations exist —
`openvhost-conf::validate` and `openvhost_core::platform::macos::demo_stack` — and the
supervisor uses the core one. Consolidating them is a follow-up.)*

---

## 4. UI

**Rail.** A real "Web Server" item after Services, routing to `/web-server`. Logs and
Settings stay inert placeholders.

**Page.** One row per brand, sharing the panel/row treatment the Services and Sites pages
already use:

- **nginx** — status pill (from the shared store), binary path and config path in the
  mono treatment, version, hot-reload yes/no, a disclosure that reveals the live config's
  text, and a Validate button that renders ok/fail plus nginx's stderr verbatim (that
  text is nginx's own diagnostic and is the useful part — do not summarize it).
- **Apache** — the same row shape, muted, with wording consistent with the site editor's
  existing hint so the product says one thing in both places: OpenVHost cannot serve
  Apache sites yet.

**Every failure renders.** An unreadable config shows an inline message on that row, not
an empty box; a spawn failure names the binary it tried; a non-zero validate shows
stderr. Nothing is logged-and-swallowed.

---

## 5. Testing

**Rust.** Version parsing from a real `nginx -v` stderr line and from junk; `validate_live`
argv shape including the mandatory `-e`; timeout behaviour; brand-id parsing rejects
unknown ids; config-path derivation.

**Frontend.** SSR tests (`svelte/server`, the established DOM-less pattern): both rows
render, Apache shows the unsupported statement, error states render rather than blanking,
the config disclosure exists.

**Not testable in this sandbox** (no GUI automation — see the `sandbox-cannot-verify-gui`
note): the real spawns, the disclosure toggle, and the Validate round-trip. These go on
the PR's owed-click-through list.

---

## 6. Gate

- **security-auditor APPROVE is merge-blocking** (CLAUDE.md golden rule 2): new IPC
  commands, and a UI-triggered process spawn.
- Its own PR, so the security review is not mixed with UI-polish churn.
- Full local gate suite (CI is disabled on this repo; local gates are the merge gate).

## 7. Open follow-ups recorded, not done here

1. The P0-7 validator's missing timeout.
2. The duplicated `find_brew_binaries`.
3. `ApacheAdapter` + `httpd.conf`/vhost template + `httpd -t`, which is what would make
   the site editor's Apache option honest.
4. The generated-vs-live diff, and the apply/swap pipeline it belongs to.
5. Editable per-brand settings, which need a persistence surface first.
