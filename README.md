<!-- Per brand guidelines §7.4: replace this text header with the horizontal
     lockup (light/dark <picture> swap) once the logomark assets exist. -->

# OpenVHost

Your friendly local host.

![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-0E6E5C)

Open-source, cross-platform local development environment for web developers — a free and open alternative to ServBay / Laragon / XAMPP / MAMP. Native binaries, no Docker daemon. GPL-licensed: no one can ever close this source — including us.

## Status

Early development — Phase 0 (proof of concept). There is nothing to install yet. What exists today: a Tauri 2 + SvelteKit app shell with a typed IPC seam, a Rust workspace laid out for the real components, and a license-gated build pipeline. The full roadmap lives in [docs/OPENVHOST_MASTER_PLAN.md](docs/OPENVHOST_MASTER_PLAN.md).

## What it will do

Phase 1 target: replace XAMPP for a PHP developer.

- PHP multi-version, with a per-site PHP version switch
- Nginx / Apache sites on `*.localhost` — no admin rights required
- MySQL and MariaDB lifecycle: init, start/stop, ports, root password
- Live log viewer per service and per site
- Config diff preview before every apply — generated config never surprises you
- `openvhost` CLI with `--json` output for scripting and CI
- Deleting a site removes config only. Your project files are never touched.

Later phases add one-click HTTPS with a local CA, wildcard DNS, Mailpit, backups, and more runtimes (PostgreSQL, Redis, Node.js). Service binaries are downloaded at runtime with SHA-256 verification — never bundled in the installer.

## Platforms

macOS (Apple Silicon) and Windows (x64) first. Linux is planned for a later phase.

## Development

Toolchain: Rust stable (pinned in `rust-toolchain.toml`), pnpm + Node LTS (pinned in `apps/desktop/.nvmrc`), Tauri 2.

```bash
cargo build --workspace && pnpm -C apps/desktop build   # build everything
cargo test --workspace && pnpm -C apps/desktop test     # run tests
cargo fmt --check && cargo clippy --workspace -- -D warnings   # lint gate
pnpm -C apps/desktop run tauri dev                      # run the app
```

Before a change merges, the full gate suite must pass: fmt, clippy `-D warnings`, tests, `cargo deny check licenses advisories`, the SPDX header check (`scripts/check-spdx.sh`), eslint, svelte-check, vitest, and the frontend build. Commits follow Conventional Commits and are DCO-signed (`git commit -s`).

Read `CLAUDE.md` and the master plan before non-trivial changes — architecture, ownership, and conventions are decided there.

## License

GPL-3.0-or-later — see [COPYING](COPYING). Contributions are accepted under the Developer Certificate of Origin: sign your commits off with `git commit -s`.

## Docs

- [docs/OPENVHOST_MASTER_PLAN.md](docs/OPENVHOST_MASTER_PLAN.md) — source of truth: architecture, roadmap, conventions, agent ownership
- [docs/OPENVHOST_BRAND_GUIDELINES.md](docs/OPENVHOST_BRAND_GUIDELINES.md) — name, voice, color, typography
