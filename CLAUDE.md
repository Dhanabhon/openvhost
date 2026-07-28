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
3. New cross-platform abstractions: get BOTH platform specialists to
   confirm feasibility BEFORE implementing (Windows has no PHP-FPM, no
   easy symlinks; design for the constraint, don't discover it later).
4. openvhost-core must never depend on tauri. All child processes go
   through openvhost-proc. All file writes are atomic. No unwrap outside
   tests.
5. Both OSes green in CI or it doesn't merge.
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
