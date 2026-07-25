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
/// `<bin> -e <err_log> -v` — nginx's version as the banner reports it, or None when
/// the probe fails for any reason (missing binary, non-zero exit, unparseable
/// output). A missing version is not an error worth failing a whole page over.
///
/// NGINX-SPECIFIC by contract: it reads STDERR, where nginx writes its banner, and
/// `php-fpm -v` writes to stdout in a different shape. It carries `err_log` for the
/// same reason `validate_live` does — `-e` is mandatory on EVERY nginx invocation
/// (see below), so the signature has to be able to supply it.
pub async fn probe_nginx_version(bin: &Path, err_log: &Path) -> Option<String>;

/// `<bin> -e <err_log> -t -c <conf>` — validate an EXISTING config file in place.
/// Writes nothing to `conf` and never calls `materialize`.
pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError>;
```

*(This block was `probe_version(bin: &Path) -> Option<String>` when the spec was
written. The rename and the added `err_log` happened during Task 1 and were recorded
in the plan's "Task 1 — As built" section but not here, so this section said one
thing and `inspect.rs` another — including omitting the very `-e` the paragraphs
below call mandatory. Both review gates flagged it independently. When an as-built
divergence is recorded in the plan, mirror it here too.)*

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

**Golden rule 4 and process spawning — CONFIRMED reading (security-auditor,
2026-07-26).** The rule says all child processes go through `openvhost-proc`.
`openvhost-conf` already spawns the P0-7 validator directly with
`tokio::process::Command` and does not depend on `openvhost-proc` — established in a
security-auditor-approved slice. That precedent was flagged for the auditor rather
than assumed; the gate **confirmed** the carve-out but **narrowed** it, because the
unconditional form "is a loophole someone will drive a truck through — the day a
slice wants `security add-trusted-cert` or a hosts-file edit, those are also one-shot
tools". The confirmed reading, which `crates/openvhost-conf/src/inspect.rs`'s module
doc carries verbatim:

> Rule 4 governs any child process whose lifetime OpenVHost manages — anything with a
> lifecycle, restart policy, health check, orphan record, or that outlives the call
> that created it. Those go through `openvhost-proc`, no exceptions.
>
> A one-shot tool invocation MAY spawn directly from the crate that owns knowledge of
> that tool if and only if **all** of: (1) it is bounded by a wall-clock deadline;
> (2) it is contained in its own process group and the deadline group-kills that
> group; (3) no component of its argv is client-supplied; (4) it runs with the app's
> own unelevated privileges — anything elevated, anything through the privileged
> helper, and anything that mutates system state (hosts file, trust store,
> launchd/services, privileged sockets) is OUTSIDE this carve-out regardless of how
> short-lived it is; (5) its stdout/stderr are captured, not inherited; (6) its
> environment is explicitly assembled, not inherited.
>
> A spawn that fails any of these goes through `openvhost-proc`.

This slice's probes meet **all six**. Condition 6 was the one gap the audit found —
`run_bounded` inherited the app's full environment while `openvhost-proc`'s
`assemble_env` is a deliberate allowlist — and it is closed by mirroring that
allowlist in `inspect::probe_env`. Two consequences motivated closing it rather than
recording it: the *same nginx binary* was being validated in one environment and
supervised in another, and nginx reads `NGINX`/`NGINX_SHM` as inherited listening
socket descriptors. Note that the P0-7 validator in `webserver.rs` is a *different*
spawn on a different path and still fails conditions 1 and 6; its follow-ups already
stand (§7.1).

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
- `binaryPath` and `configPath` are derived **server-side**, in `stack.rs`, at the moment
  the specs are built — and returned alongside them as `StackPaths`, which the app `manage`s
  as Tauri state. The commands read that managed state. They are **not** read back off the
  supervisor: §3.3 explains at length why that is not possible (`Supervisor` exposes no
  accessor and `ServiceStatus` carries no program path). Same values either way, but the
  mechanism is the managed state, not the registered spec. **Nothing client-supplied reaches
  argv.**
- All three commands are read-only **with respect to configuration**: none writes a config
  file, and none calls `materialize`. They are not read-only with respect to the filesystem,
  and the boundary is worth stating exactly, because `nginx -t` performs a full configuration
  parse and creates the files and directories that configuration declares — its `error_log`
  and `access_log` targets and its `*_temp_path` directories — using the app's own privileges.
  `-e <home>/logs/nginx.error.log` redirects only nginx's *pre-parse* error log; it does
  **not** override an `error_log` directive inside the config (verified empirically against
  nginx 1.27.3: an `error_log /tmp/x;` in the config is created even with `-e` pointing into
  our home). Under the config OpenVHost provisions, every such path is inside `<home>`. That
  is a property of *that config text*, not a property enforced by this slice: the live config
  is a user-editable file, so a hand-edited `error_log`/`access_log`/`*_temp_path` will have
  `nginx -t` create files anywhere the user can write. This is not a privilege boundary — the
  user could run `nginx -t` themselves with the same effect — but it must not be described as
  a containment guarantee.
- **Forward-looking constraint on returning stderr verbatim.** `nginx -t` follows `include`
  directives and echoes offending tokens verbatim, so a config that `include`s a file of
  `KEY=value` lines echoes those values into stderr, which this page renders. Measured by the
  audit: an `include env2.txt` containing `AWS_SECRET_KEY=…` put the secret in stderr.
  **Harmless as built** — the validator runs as the user with no more reach than the user
  already has, an unreadable file yields permission-denied with no content, and there is no
  `{@html}` anywhere so the `<pre>` is escaped. It stops being harmless the moment any
  validator runs elevated or through the privileged helper: "return stderr verbatim to the
  webview" then becomes an arbitrary-file-read primitive driven by a user-writable config.
  So: **a validator whose stderr crosses IPC must not run elevated.** If one ever must,
  the stderr needs filtering at that point, not here.
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

1. The P0-7 validator's missing timeout — and, now, its missing env allowlist: it fails
   conditions 1 and 6 of the confirmed golden-rule-4 reading in §3.1.
2. The duplicated `find_brew_binaries`.
3. `ApacheAdapter` + `httpd.conf`/vhost template + `httpd -t`, which is what would make
   the site editor's Apache option honest.
4. The generated-vs-live diff, and the apply/swap pipeline it belongs to.
5. Editable per-brand settings, which need a persistence surface first.

### Carried out of the review gates (deliberate deferrals, with reasons)

6. **No bound on call rate, response size, or blocking-pool consumption** (audit A2,
   Medium). No rate limit on the two spawning commands, no size cap on the config text or
   the stderr crossing IPC, and `tokio::fs` on a hung mount occupies an uncancellable
   blocking-pool thread. Wants an in-flight guard or semaphore plus a ~1 MiB cap with a
   truncation marker. Deferred because exploiting it requires XSS in a first-party webview
   that loads no remote content.
7. **The two degraded startup paths surface Tauri's raw "state not managed" string** (audit
   A4, Low; verified *not* a panic). One `app.manage` after the match, with every arm
   yielding an `Option<StackPaths>`; `commands::stack_paths`'s doc already records both the
   fix and the no-overwrite trap that rules out the obvious approximation.
8. **Extract ONE shared bounded one-shot runner** (audit A7), so the process-group
   containment *and* the env allowlist each have exactly one home instead of two. This is
   the real fix for the duplication `inspect.rs`'s module doc currently manages with a
   "change both or neither" note, and it inverts the layering objection: the dependency
   becomes one on a mechanism rather than one on the supervisor.
9. `PROBE_TIMEOUT` is re-exported from `openvhost-conf` with no consumer — drop it or use
   it (review M4).
10. Display goes through lossy `.display().to_string()` while the spawn uses the real
    `OsStr`, so a non-UTF-8 `OPENVHOST_HOME` renders a mangled path beside a correct
    invocation (review M6).
11. `StackPaths::nginx_error_log()`: `<home>/logs/nginx.error.log` is written out in three
    places. **Sealing `StackPaths`' fields was ruled AGAINST** by the audit — same crate,
    so field privacy would guard only a module boundary we control, whereas `89471df`
    sealed across crates; and after the `ValidationTarget` change the remaining field reads
    are display-only. Bundle the accessor with this de-duplication instead.
12. Smaller carried items: the tearable test pid-file read (the audit ranked this *less*
    urgent than a follow-up — `echo $!` is a single sub-8-byte `write(2)` and the
    empty-file window is already handled), `ProbeFailure::Io` conflating spawn failure with
    pipe-drain failure, the duplicated error extraction in the store (the
    `[object Object]` path is unreachable today — `IpcError::Simulated` is only produced by
    `core_info`), no `reading[id]` flag, `.panel` CSS triplication, and `Button`'s
    independent `expanded`/`controls` optionals.
