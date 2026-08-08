# OpenVHost — Claude Code Project Instructions

Read docs/OPENVHOST_MASTER_PLAN.md before any non-trivial task. It is the
source of truth for architecture, roadmap, and agent ownership.

## Golden rules
1. Delegate by ownership map (§6.2 of the plan). Platform-#[cfg] code →
   platform specialists. UI → tauri-frontend-engineer. Templates →
   config-template-engineer.
2. Security-sensitive paths (helper, cert, download-verify, hosts, IPC
   ACLs, signing workflows) are MERGE-BLOCKED without a security-auditor
   APPROVE.
3. **Windows is CUT from development for now — build for macOS** (owner
   decision, 2026-08-08, restating and strengthening the macOS-first scope
   of 2026-07-22). Do not seek Windows-specialist confirmation, do not
   chase Windows parity, and do not treat a Windows gap as a blocker.
   Existing `#[cfg(windows)]` code is **deferred, not deleted**: keep it
   compiling, and let a new Windows arm be an honest
   `Unsupported`-style refusal rather than a silent no-op or a guess.
   *(Original: get BOTH platform specialists to confirm feasibility BEFORE
   implementing — Windows has no PHP-FPM, no easy symlinks; design for the
   constraint, don't discover it later.)*

   **Still keep the seam.** Putting platform-specific work behind a facade
   costs nothing now and is what makes the later Windows phase possible —
   it is the *sign-off*, not the *structure*, that is suspended. Measured
   answers to the old rule's own examples are recorded in the
   `servbay-measured-directly` memory (Windows really does use `php-cgi`
   over a TCP port, and `.cmd` shims where macOS uses symlinks), so the
   constraint is documented rather than forgotten.
4. openvhost-core must never depend on tauri. All child processes go
   through openvhost-proc. All file writes are atomic. No unwrap outside
   tests.
5. **macOS green or it doesn't merge** (see rule 3 — Windows is cut for
   now). Note GitHub Actions is disabled to save minutes, so **the local
   gate is the merge gate**: `cargo fmt --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
   `pnpm -C apps/desktop test` and `check`.
   Two traps that have made a gate lie, both measured in this repo:
   **never pipe a gate through `tail`** (you read `tail`'s exit code), and
   **a shared `CARGO_TARGET_DIR` across worktrees silently links a stale
   crate** without invalidating dependents — which can make a gate falsely
   **green**. Run from a clean fingerprint or an isolated target dir.
   *(Original: both OSes green in CI or it doesn't merge.)*
6. Never bundle service binaries into the installer — runtime download
   with SHA-256 verification only (license + security).
7. **DEFERRED until after v1.0.0 ships — do not act on this rule or raise
   licensing as a consideration in the meantime** (owner decision,
   2026-07-28). It was costing deliberation on nearly every slice while the
   shape of the product is still moving, and any call made now would be
   re-made later. Keep adding SPDX headers and `git commit -s` sign-off —
   both are habit, cost nothing, and stripping them would be pure churn —
   but skip the license-gate on new dependencies and do not weigh licence
   implications when choosing an approach. Revisit the whole rule at 1.0.0.
   *(Original: License is GPL-3.0-or-later (plan §1.3): SPDX headers on new
   files, DCO sign-off, license-gate must pass for any new dependency,
   never link native MySQL/MariaDB client libs — separate processes only.
   Note rule 6 still stands on its own: runtime download with SHA-256
   verification is a security requirement independently of licensing.)*
8. Anything listed in plan §7 (Open Questions) needs a human decision —
   stop and ask.

## Commands
- Build all: `cargo build --workspace && pnpm -C apps/desktop build`
- Test: `cargo test --workspace` · `pnpm -C apps/desktop test`
- Lint gate: `cargo fmt --check && cargo clippy --workspace -- -D warnings`
- Run app (dev): `pnpm -C apps/desktop tauri dev`
- Conventional Commits required.
- state.db uses sqlx compile-time-checked queries with committed offline
  metadata (`.sqlx/`). After changing any `query!`/`query_as!` or a migration:
  `DATABASE_URL="sqlite://$PWD/target/_prepare.db" sqlx database create && \
   sqlx migrate run --source crates/openvhost-core/src/db/migrations && \
   cargo sqlx prepare --workspace` — then commit the updated `.sqlx/`. Builds
  and CI run offline against the committed cache (no DB required). If
  `sqlx-cli` can't be installed offline, build the query crate once with a
  live `DATABASE_URL` against a migrated temp DB instead (unset
  `SQLX_OFFLINE`) — sqlx writes `.sqlx/` as a side effect of that build.

## Orchestration workflow

You are the orchestrator. Plan, decompose, delegate, synthesize — don't do the
work yourself.

- **Reasoning-heavy work** (architecture, complex debugging, algorithm design):
  delegate to the `deep-reasoner` subagent.
- **Mechanical work** (boilerplate, tests, formatting, simple edits): delegate to
  the `fast-worker` subagent.
- **Codex** (`/codex:rescue -background`) is a strong engineer, roughly on par
  with `deep-reasoner` but reasoning from a different perspective. Treat it as a
  peer, not a reviewer — hand it the problem, not your answer to check.

### Delivery pipeline (owner decision 2026-07-31 — speed over ceremony)

Design: `docs/superpowers/specs/2026-07-31-lean-pipeline-design.md`. The owner
measured the old pipeline at ~28 serial subagent round trips per slice, 43% of
them rework, and chose speed while accepting more risk. Per slice:

1. **Design once** — write the spec yourself. Spec stays; it is cheap and
   everything downstream reads from it.
2. **Build in ≤3 tasks**, sized to a coherent chunk. **No review between tasks.**
3. **Gate once at the end** — whole-branch review + live proof + security audit
   (audit whenever the slice touches the command surface, credentials, file
   paths, or child processes).
4. **One fix wave**, then merge.

Never traded away, because they are cheap and caught this project's worst bugs:
the **live proof against real binaries** and the **security audit**. Per-task
review was 60–70% of the cost and produced the least severe findings — that is
what got cut.

Since per-task review is gone, every implementer brief must carry these as
binding requirements, and the implementer must report against them: vacuity
proof per test group; filesystem/locale semantics (case-insensitive volumes,
separators, symlinks); reentrancy and lifecycle (overlapping polls, listeners
outliving teardown); exhaustiveness (no wildcard arms; prove a new variant fails
to compile); and the seams between tasks.

Record **dispatch count and wall clock** per slice in the ledger. Target: ~8
dispatches, 3–4 h. If three slices do not move, the diagnosis was wrong —
revisit the design rather than defend it.

### High-stakes decisions

**Reserved** for decisions both expensive to reverse *and* without precedent in
this codebase — schema, credentials, app lifecycle. (The MySQL slice qualified;
the docroot warning did not.) Not the default.

1. Task `deep-reasoner` and Codex on the same problem in parallel.
2. Show neither one the other's answer, or your own leaning. Independent takes
   only — the point is to avoid anchoring.
3. Synthesize. Where they agree, confidence is high. Where they diverge, that
   divergence *is* the decision, and it's yours to make. Take the strongest parts
   of both rather than picking a winner.

### Keep your context lean

Delegate the noisy work; keep the main transcript for planning and synthesis.
Don't read large files or run broad searches yourself when a subagent can do it
and report back. Ask subagents for conclusions, not transcripts.
