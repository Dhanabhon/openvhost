<!-- Per brand guidelines §7.4: replace this text header with the horizontal
     lockup (light/dark <picture> swap) once the logomark assets exist. -->

# OpenVHost

Your friendly local host.

![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-0E6E5C)

Open-source, cross-platform local development environment for web developers — a free and open alternative to ServBay / Laragon / XAMPP / MAMP. Native binaries, no Docker daemon. GPL-licensed: no one can ever close this source — including us.

## Status

Early development — Phase 0 (proof of concept). There is nothing to install yet.

Done so far: the first 2 of 9 Phase 0 tasks — a Tauri 2 + SvelteKit app shell with a typed IPC seam over a tauri-free Rust core, the full workspace layout, and a license-gated build pipeline. Next up: the process supervisor (`openvhost-proc`), the heart of the app. Running the dev app takes one command: `./scripts/dev.sh`.

## Roadmap

- **Phase 0 — proof of concept** (in progress): burn down the risky bits — process supervision, PHP on both OSes, download pipeline, config generation, E2E harness
- **Phase 1 — MVP**: replace XAMPP for a PHP developer (see below)
- **Phase 2 — daily driver**: phpMyAdmin/Adminer, CLI shims on PATH, metrics, EN + TH interface
- **Phase 3 — competitive edge**: one-click HTTPS with a local CA, ports 80/443, wildcard DNS, Mailpit, backups, `openvhost.yaml` env-as-code
- **Phase 4+**: PostgreSQL, Redis, MongoDB, Node.js/Python/Go runtimes, Linux support

Details and decisions: [docs/OPENVHOST_MASTER_PLAN.md](docs/OPENVHOST_MASTER_PLAN.md).

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
./scripts/dev.sh                                        # run the app (one command; installs deps on first run)
```

## Contributing

- Read `CLAUDE.md` and the master plan first — architecture, ownership, and conventions are decided there, not in PR threads.
- Before a change merges, the full gate suite must pass: fmt, clippy `-D warnings`, tests, `cargo deny check licenses advisories`, the SPDX header check (`scripts/check-spdx.sh`), eslint, svelte-check, vitest, and the frontend build.
- Commits follow Conventional Commits and are DCO-signed (`git commit -s`). New source files carry an SPDX `GPL-3.0-or-later` header.
- Changes touching platform-specific code need both platform stories to hold — Windows has no PHP-FPM and no easy symlinks; design for the constraint.

## License

GPL-3.0-or-later — see [COPYING](COPYING). Contributions are accepted under the Developer Certificate of Origin: sign your commits off with `git commit -s`.

## Docs

- [docs/OPENVHOST_MASTER_PLAN.md](docs/OPENVHOST_MASTER_PLAN.md) — source of truth: architecture, roadmap, conventions, agent ownership
- [docs/OPENVHOST_BRAND_GUIDELINES.md](docs/OPENVHOST_BRAND_GUIDELINES.md) — name, voice, color, typography
