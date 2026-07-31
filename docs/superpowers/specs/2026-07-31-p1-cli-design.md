# P1 `openvhost` CLI v1 — Design

- **Date:** 2026-07-31
- **Status:** Approved under the owner's standing delegation. Two owner calls are surfaced at the end; neither blocks the build.
- **Roadmap line:** "`openvhost` v1: `start|stop|restart|status|list` with `--json` (mirrors servbayctl verbs; add `reload`, `kill`, `stop-all` for parity)" — master plan §4, Phase 1.
- **Design process:** dual blind (deep-reasoner + Codex), synthesized here. Reserved for this slice under the lean pipeline's rule — a local control channel into a process supervisor is expensive to reverse *and* without precedent in this codebase. Agreements adopted as-is; divergences resolved with rationale.
- **Plan:** `docs/superpowers/plans/2026-07-31-p1-cli.md`

## What ships (end-to-end demonstrable)

With the app running: `openvhost list --json | jq` prints the same service table the GUI shows; `openvhost start nginx` blocks until nginx is genuinely `Running` and exits 0; `openvhost stop-all` tears the stack down; a failure exits non-zero with the real stderr. With the app **not** running: `openvhost status` says so and exits **0**, while control verbs exit 69 and say what to do.

## Decisions

### D1 — One app-owned unix socket, bound by whoever holds the instance lock (both designers agreed)

`<home>/run/control.sock`, `SOCK_STREAM`, bound **inside** the `Ok(Some(lock))` arm of the app's startup — so the socket exists **iff** a supervisor exists. The degraded-boot arm (lock unavailable) must not bind. Protocol: one bounded JSON request → one JSON response → close. No session state, no multiplexing.

**Headless CLI lost, decisively.** `Supervisor::with_orphan_cleanup` reaps identity-matched records at construction and `InstanceLock` is fd-scoped: a short-lived CLI supervisor that starts nginx, records it, and exits leaves a record **the next app launch kills** — `openvhost start nginx` followed by opening the app would silently stop nginx. Making it coherent requires a real daemon, which then holds the lock and permanently demotes the GUI to "not starting the supervisor". That is a different product.

**Read-only CLI lost:** it drops seven of eight roadmap verbs, and the status it could synthesize from `supervised.json` is differently shaped from the GUI's — no `display_name`, no `endpoint`, and no way to distinguish `Stopped` from `Failed` (records are removed on terminal state). That is the boolean-collapse-where-a-state-belongs error this codebase has hit three times. It survives only as the "app not running" answer inside D3.

**Stale socket:** after taking the lock, unlink the path **only** when `symlink_metadata` proves it is a socket; refuse a symlink or any other file type. Holding the exclusive flock is what makes that unlink provably safe rather than a racy connect-probe. Remove it on orderly shutdown. Socket mode `0600`.

**Socket path length:** macOS caps `sun_path` at 104 bytes including the NUL. `<home>/run/control.sock` is fine, but a tempdir `OPENVHOST_HOME` in hermetic tests can approach it — the php-fpm and mysqld sockets already hit this. Fail with a typed `SocketPathTooLong` at bind time rather than an `EINVAL` surprise.

The existing constant is `openvhost_core::site::apply::MAX_SOCKET_PATH_BYTES = 103`, and **`openvhost-core` depends on `openvhost-proc`, not the reverse** — so proc cannot reuse it, and inverting the dependency for a constant is absurd. `openvhost-proc::control` declares its own `MAX_SOCKET_PATH_BYTES = 103` with a doc comment naming core's and stating why they are separate. Deliberate duplication, not an oversight: 103 is a fixed Darwin ABI constant that cannot drift, and de-duplicating it means editing the audited apply path and the MySQL datadir guard for zero behavioural gain.

### D2 — Authorization: 0700 + 0600 + peer UID equality, no token (both agreed independently)

Directory `0700` (already enforced), socket `0600`, and `UnixStream::peer_cred()` checked for effective-UID equality with the server **before reading the request**.

Threat model, stated plainly so its absence is not read as an oversight:
- **Another local user** — blocked by `<home>` at 0700, the same gate already protecting `state.db`'s MySQL root password. This adds no new exposure class.
- **A same-UID rogue process** — *cannot be stopped at this layer at all*. It can read `state.db`, read any token from the same 0700 directory, or simply run `nginx` itself. A token would be security theatre. Peer credentials are kept anyway as defence in depth against a future permission regression.
- **A compromised webview** — unaffected: no new Tauri command, and the tray audit already narrowed the capability set.

Use `tokio::net::UnixStream::peer_cred()` rather than raw `LOCAL_PEERCRED` — **no `unsafe` on a security-sensitive path**. Verify the API against the resolved tokio version and name the version checked. Cap ingress at 64 KiB (mirroring `FileRegistry::MAX_BYTES`) with a 2 s read deadline, so a connected-and-silent peer cannot pin a task.

### D3 — With no app: answer truthfully, never auto-launch (deep-reasoner's exit-0 adopted over Codex's uniform 69)

Add `InstanceLock::probe(run_dir) -> SupervisorPresence` (acquire-then-drop; the reap lives in the `Supervisor` constructor, not the lock, so probing is side-effect-free). **Three variants, not a bool:** `Present`, `Absent`, `Indeterminate { reason }` — an I/O error on the lock file is a genuine third answer, and collapsing it would make `status` lie.

- `status` / `list` with no app → **exit 0**, `"supervisor":"notRunning"`, empty service list, and a loud human line. Codex proposed a uniform 69 here; it lost because a script running `openvhost status --json | jq` wants to *learn* the app is down, not to handle an error. Conflating "the answer is no" with "I could not answer" is the same state-as-error collapse D1 rejects.
- Any **control** verb with no app → exit 69, `error.code = "supervisorUnavailable"`, message naming the fix.
- Lock held but socket unreachable → also 69, distinct code `controlChannelUnavailable` ("the app appears to be running but is not accepting control connections — it may still be starting"). Same for an `Indeterminate` probe: both are genuinely "I could not answer".
- Socket unreachable and the lock probe says **`Absent`** → the app is definitively down, so `status`/`list` answer exactly as they do for a missing socket (exit 0, `supervisor:"notRunning"`, empty list) while control verbs still exit 69. This is the force-quit leftover of D1's stale-socket paragraph: no shutdown code can remove a socket the app was `SIGKILL`ed out of, so treating the leftover file as an ambiguity would make `status` exit 69 for an indefinite window — the state-as-error collapse this decision exists to prevent, in the scenario click-list item 7 already anticipates. The connect result stays authoritative about *whether contact was made*; the probe answers the separate question "is an app alive at all", and it decides the outcome only for this one variant.

**Never auto-launch the app.** It is an unrecoverable side effect under ssh/CI, the bundle path is not reliably knowable (dev build vs `/Applications` vs cask), it would make `stop-all` *launch* an app, and it needs a poll-and-timeout loop afterwards. A `--launch` flag is a named deferral.

**Exit codes** (Codex's finer set adopted over deep-reasoner's coarser one — a script wants these distinguished), as one exhaustively-matched enum so docs and behaviour cannot drift:

| Code | Meaning |
|---|---|
| 0 | success, including an explicit "unchanged" result |
| 64 | usage error |
| 66 | unknown service id |
| 69 | supervisor or control channel unavailable |
| 70 | operation or protocol failure (service failed to start/stop) |
| 75 | busy, or a transition timed out |
| 77 | authorization denied |

### D4 — Verbs target registered service ids, and the server waits (both agreed)

Ids are exact registered keys — `nginx`, `php-fpm-8.4`, `mysql-8.4`. **Never sites**: a site has no process, and `start mysite.localhost` would have to mean "nginx plus the right php-fpm major", a different and confusing operation.

- `status [<id>]` — full `ServiceStatus` rows plus a supervisor/home/version header. The only verb that works with no app.
- `list` — the service table alone, id-sorted. Not a silent alias of `status`.
- `start|stop|restart <id>` — **the server waits, not the client.** The handler subscribes to `SupervisorEvent` *before* calling `start`, waits for the terminal state, and responds once. This puts the deadline where the specs already live (MySQL's readiness deadline is 15 s and its grace 15 s — the same trap that forced `STOP_ALL_TIMEOUT` to 18 s) and avoids a CLI polling loop. `--no-wait` opts out. Already in the target state → exit 0 with `"disposition":"unchanged"`, mirroring `Supervisor::start`'s own early `Ok`. `restart` is sequenced server-side so a tray click or an Apply cannot interleave between the two halves.
- `stop-all` — **in v1**, as the third caller of `quit::stop_all` **verbatim**, taking the same `BulkLock` the tray takes and **rejecting rather than queuing** (exit 75). Codex deferred this; it is included because it is the single verb where a CLI beats the GUI (tearing a stack down in CI), and because D6's trait seam lets the desktop-side handler call the existing primitive rather than relocating audited code. If the `BulkLock` coordination turns out to be more than a thin call, the implementer stops and reports rather than refactoring.

**Deferred with reasons** (both designers, independently, reached the same cut):
- **`reload`** — its only honest meaning is "run the apply pipeline", which today lives inside `apply_config` behind several pieces of Tauri managed state. Reaching it means extracting that body into a free function: a mechanical but audit-heavy refactor of the code that writes nginx config and restarts services. Two authorization-relevant changes in one PR is one too many. As a synonym for `restart` it would falsely claim gracefulness.
- **`kill`** — needs a `StopMode` on the service task's control channel, in the crate carrying the orphan reaper. Deciding argument: **the existing stop path already escalates to SIGKILL after the grace**, so `kill` only saves the wait. Low value, real risk. When it lands: identity-matched to one registered id, never a pid, never consulting `supervised.json` — this is not the headless group-kill P0-8 forbade, and the spec must say why.

### D5 — A versioned envelope reusing `ServiceStatus` verbatim (both agreed)

```
{"schemaVersion":1,"ok":true,"command":"list","result":{…}}
{"schemaVersion":1,"ok":false,"command":"start","error":{"code":"unknownService","message":"…"}}
```

One single-line JSON object on stdout and **nothing else on stdout, ever**; in `--json` mode stderr stays empty. **Errors go to stdout too** — a script piping to `jq` must get parseable JSON on failure; the exit code remains the primary signal.

Service objects **reuse `ServiceStatus`'s existing serde shape verbatim** (`id`, `displayName`, `endpoint`, `pid`, tagged `state` with `failed.exit` and `failed.stderrTail`). This is the highest-leverage reuse decision in the slice: the CLI and the GUI cannot disagree about what a service is, and `Failed` carries the real stderr through both surfaces.

Stability, stated in `--help` and here: within `schemaVersion: 1`, fields are added, never removed or retyped; `error.code` values are added, never repurposed. Consumers must ignore unknown fields, must not rely on key order or human messages, and must treat an unknown code as a generic failure.

### D6 — Control module in `openvhost-proc`; policy stays in desktop (deep-reasoner's trait seam adopted)

- **`openvhost_proc::control`** — wire types + version, `ServiceId` newtype parsed at ingress, a **sync** std client (the CLI is one round trip; an async runtime buys nothing and costs startup), the tokio server, the peer-cred check, caps and timeouts. It belongs here, not in core: the contract is about services, and `ServiceStatus`/`ServiceState` already live here.
- **`ControlHandler` trait in proc, implemented in `apps/desktop/src-tauri/src/control.rs`.** Transport, parsing and authorization in proc; *policy* (the `BulkLock`, `quit::stop_all`, later the apply pipeline) in desktop. Codex proposed putting execution in proc and moving `stop_all` there; that lost because it churns already-audited code, and the trait seam is also what lets the CLI be tested against a fake handler.
- **Nothing moves out of desktop.** `quit::stop_all` stays exactly where it is; the handler calls it.
- **`apps/cli`** gains `openvhost-core` (home resolution only), `serde_json`, and `clap` (derive). Hand-rolling six subcommands is where usage bugs live, and `--help` / `--version` / exit-64-on-bad-args are what a public CLI is judged on. Verify whether `clap` is already in the lock file and say so.

**No path, argv, or pid is expressible in the wire type.** That is the containment invariant: the channel cannot make the supervisor spawn anything `stack.rs` did not already register, because `Supervisor::start` returns `NotFound` for anything else.

### D7 — Testing (both agreed on the shape)

- **Pure:** request/response round-trip including unknown-op → a *typed* error, never a silent no-op; `ServiceId::parse`; the size cap and read deadline against a hand-fed reader; `peer_is_authorized(peer_uid, our_uid)`; `SupervisorPresence` mapping including `Indeterminate`; the exit-code mapping matched exhaustively over every response variant.
- **Real socket, one process:** a real client against a recording fake handler in a tempdir — happy path, oversized request, silent peer, socket mode 0600, and the **containment test**: a request naming an unregistered id is refused and the fake records that no spawn was attempted.
- **Two processes** (`apps/cli/tests/`): the test binds the socket and serves a fake handler, then runs the real `CARGO_BIN_EXE_openvhost` as a child with `OPENVHOST_HOME` set, asserting exit codes and exact JSON. **This is the highest-value test in the slice** — the only one proving the two-process story, and it needs no GUI.
- **Real supervisor:** the handler wired to a real `Supervisor` running `proc_testchild`, driving start/stop/status over the socket, asserting the stderr survives to the client.
- **Human click-list:** the app actually binding at startup, stale-socket unlink after a force quit, `stop-all` against a live nginx/php-fpm.

## Security posture

New local control surface → **security-auditor review is mandatory before merge**. The claims to check: the socket exists only when a supervisor does; 0700 dir + 0600 socket + peer-UID equality; the wire type cannot express a path, argv or pid; ingress is size- and time-bounded; the stale-socket unlink is guarded by both the lock and a `symlink_metadata` type check; and the same-UID limitation is stated rather than papered over.

## Owner calls (shipping with a default; both reversible)

1. **The authorization principal is the logged-in UID.** Both designers reached this independently: nothing at this layer can defend against malware already running as you, and a token stored in the same 0700 directory is theatre. If you want protection against, say, a malicious `npm postinstall`, the answer is the Phase 3 privileged helper — and the CLI should stay read-only until then. Shipping with UID trust unless told otherwise.
2. **How does `openvhost` get onto your PATH?** Today the binary only exists at `target/{debug,release}/openvhost`. A CLI nobody can invoke is a changelog line, not a feature. Options: bundle it in `Contents/MacOS` with an in-app "Install command line tool" action (the VS Code pattern), or a brew formula. This needs a decision and probably its own packaging slice; **this slice's click-list runs it by absolute path.**

## Recorded, not fixed

`supportsHotReload: true` is already surfaced to the UI and honoured nowhere — the product advertises a graceful nginx reload it does not perform. Either `reload` eventually means SIGHUP (needing a signal surface on `Supervisor`), or that DTO field is misleading today. Flagged rather than silently decided.

## Verification owed to a human (click-list)

1. Start the app; confirm `<home>/run/control.sock` exists and is `srw-------`.
2. `openvhost list --json | jq` matches what the Services page shows.
3. `openvhost start nginx` blocks until it is really up, exits 0; `curl` the site.
4. `openvhost stop nginx`; the GUI's Services page reflects it without a refresh.
5. `openvhost stop-all` while the tray is mid-Stop-all → rejected with exit 75, not queued.
6. Quit the app; `openvhost status` says not running and exits **0**; `openvhost start nginx` exits 69.
7. Force-quit the app (leaving a stale socket), relaunch, confirm the CLI works again.
