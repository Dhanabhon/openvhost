# OpenServ — Development Master Plan & Claude Code Handoff Document
> **Purpose of this document:** This is the single source of truth for the OpenServ project, written to be consumed by Claude Code (and human contributors). It contains the product definition, confirmed technical decisions, architecture, phased roadmap, and a complete subagent allocation plan with ready-to-use agent definitions.
>
> **Document status:** v1.2 — 2026-07-21 (license decision recorded · brand guidelines linked)
> **Companion documents:** `docs/OPENSERV_BRAND_GUIDELINES.md` v1.0 — brand foundation, color/typography tokens, voice & microcopy, trademark summary; the input spec for tauri-frontend-engineer UI work and for `TRADEMARK.md` (OQ#8).
> **Decision legend:** ✅ DECIDED · 🟡 PROPOSED (default unless overridden) · ❓ OPEN (needs human decision)
---
## 1. Project Overview
**OpenServ** is an open-source, cross-platform local development environment platform for web developers — a free and open alternative to ServBay / Laragon / XAMPP / MAMP.
| Attribute | Value |
|---|---|
| Product name | OpenServ |
| Category | Local dev environment orchestrator (native binaries, no Docker) |
| Platforms (initial) | macOS (Apple Silicon), Windows (x86_64) |
| Platforms (later) | macOS Intel ❓, Windows ARM64 ❓, Linux (Phase 4+) |
| License | **GPL-3.0-or-later** ✅ — owner requires strong copyleft: anyone who distributes OpenServ or a derivative (modified or commercial) must disclose full source under the same license. AGPL-3.0 was considered and rejected (corporate AGPL bans would shrink the contributor pool; realistic distribution vector for a desktop app is binaries, which GPL covers). See §1.3 |
| Distribution model | App = orchestrator only. Service binaries (PHP, MySQL, …) are **downloaded at runtime**, never bundled in the installer (license risk mitigation, esp. GPL MySQL/MariaDB) ✅ |
| Monetization | None. Fully free. No account, no telemetry by default ✅ |
| Phase 1 service scope | PHP (multi-version), MySQL, MariaDB, Nginx, Apache ✅ |
### 1.1 Product Principles (non-negotiable)
1. **Lightweight always-on:** The app runs all day next to an IDE and a browser. Idle RAM budget for the app itself: **< 100 MB**. This is why Tauri was chosen over Electron.
2. **Never destroy user data:** Deleting a site removes config only, never project files. Config changes show a diff preview. "Pause" (comment-out) over "delete" wherever reversible.
3. **Generated vs. user config strictly separated:** Generated files carry a DO-NOT-EDIT header pointing to the correct custom-config location, and are regenerated idempotently.
4. **CLI is a first-class citizen:** Everything the GUI can do, `openservctl` can do — with `--json` output for scripting/CI.
5. **Reproducible environments:** A project-level `openserv.yaml` committed to a repo recreates the same environment for the whole team (our answer to ServBay's `.servbay.config`, but more complete).
6. **Open manifests:** Package definitions live in a public repo (`openserv/manifests`) so the community can add packages without waiting for an app release.
### 1.2 Competitive Position (from ServBay docs analysis, 2026-07)
- ServBay now supports **both macOS and Windows** → "cross-platform" is *not* our differentiator.
- Our differentiators: **open-source, free, no account required, reproducible env files, JSON-scriptable CLI, open package manifests, config diff preview.**
- Closest open-source competitor on Windows: Laragon. Closest Docker-based competitors: DDEV/Lando. Our pitch vs. them: native performance, no Docker daemon requirement.
### 1.3 Licensing Model (✅ decided)
- **App license: GPL-3.0-or-later.** Every distribution of OpenServ or a derivative work — modified or not, commercial or not — must ship corresponding source under GPL. Commercial *sale* remains permitted (GPL never forbids selling); the obligation is disclosure.
- **Known, accepted limits of copyleft (no license can close these):** purely internal/private use without distribution triggers no disclosure; GPL-3.0 does not treat network service (SaaS) as conveying. AGPL would close the SaaS gap but was rejected (see table row above).
- **Inbound contributions: DCO-only 🟡** (`git commit -s`, no CLA). Consequence — and intended feature: copyright stays distributed across all contributors, so **no one, including the project owner, can ever relicense OpenServ to proprietary**. This makes a Laragon-style closed-source pivot structurally impossible and maximizes community trust. Trade-off: no future dual-licensing/commercial-exception model. (Recorded as OQ#1 with DCO-only as default.)
- **Dependency direction is safe:** permissive deps (Tauri MIT/Apache-2.0, Svelte MIT, most Rust crates MIT/Apache-2.0 — all GPLv3-compatible) may be combined into a GPL-3.0 work. CI must run license scanning (`cargo-deny` + npm license checker) to block GPL-incompatible additions.
- **MySQL/MariaDB (GPL-2.0-only) vs app (GPL-3.0):** the two are incompatible *only if combined into one work*. Our architecture runs them as separate processes over sockets/TCP = mere aggregation → no conflict. **Hard rule: never link native MySQL/MariaDB client libraries into the app; use permissively-licensed pure-Rust protocol crates.**
- **Self-hosted GPL binaries (Phase 2+):** distributing our own builds of GPL packages obligates us to publish corresponding source + build scripts alongside every artifact (GPLv2 §3 / GPLv3 §6).
- **Files:** `COPYING` (GPL-3.0 full text) at repo root · SPDX header `GPL-3.0-or-later` in source files · `TRADEMARK.md` asserting the OpenServ name/logo policy (GPL has no Apache-§6-style trademark clause; trademark law applies independently) · in-app "Open Source Licenses" page listing licenses of every installed package.
- Legal counsel review before first public release still required (OQ#6) — this section is engineering-level guidance, not legal advice.
---
## 2. Tech Stack (Confirmed)
| Layer | Choice | Status | Notes |
|---|---|---|---|
| Desktop shell | **Tauri 2.x** | ✅ | Verify latest 2.x minor at implementation time |
| Backend language | **Rust** (stable, workspace) | ✅ | tokio async runtime |
| Frontend | **SvelteKit + Svelte 5 + TypeScript (strict)** | ✅ | React was the fallback; SvelteKit confirmed |
| UI kit | shadcn-svelte + Tailwind CSS | 🟡 | |
| App state store | SQLite via `sqlx` (bundled, no server) | ✅ | Single file: `state.db` |
| Template engine | **Tera** (Rust) for all generated configs | ✅ | |
| IPC (app ↔ frontend) | Tauri commands + events, typed via `tauri-specta` (generated TS bindings) | 🟡 | |
| IPC (app ↔ privileged helper) | macOS: Unix socket/XPC · Windows: Named Pipe with strict ACL | ✅ | Phase 3 |
| Cert generation | `rcgen` (Rust) | 🟡 | mkcert-style local CA; Phase 3 |
| HTTP downloads | `reqwest` + `sha2` verification + `zstd`/`tar`/`zip` extraction | ✅ | |
| Packaging | Tauri bundler → `.dmg` (macOS), NSIS `.exe` (Windows) | 🟡 | |
| Auto-update | Tauri updater (signed) | 🟡 | Phase 3 |
| CI/CD | GitHub Actions (macos-14 arm64 + windows-latest matrix) | ✅ | |
| Binary hosting | GitHub Releases (app + packages) | 🟡 | Cloudflare R2 as overflow option |
**Version caveat for Claude Code:** All specific library versions in this document reflect mid-2026 knowledge. At implementation time, check current versions/APIs (especially Tauri 2.x plugins, Svelte 5, sqlx) before writing code.
---
## 3. System Architecture
```
┌─────────────────────────────────────────────────────┐
│  FRONTEND — SvelteKit (WebView)                     │
│  Site panel · Service panel · Logs · Settings       │
└───────────────┬─────────────────────────────────────┘
                │ Tauri IPC (typed commands + events)
┌───────────────▼─────────────────────────────────────┐
│  CORE — Rust (unprivileged, runs as user)           │
│  ├─ openserv-proc   Process supervisor              │
│  ├─ openserv-pkg    Package manager (dl/verify)     │
│  ├─ openserv-conf   Config generator (Tera)         │
│  ├─ openserv-cert   Local CA + leaf certs (P3)      │
│  ├─ openserv-core   Domain model, state (SQLite)    │
│  └─ openservctl     CLI binary (same crates)        │
└───────────────┬─────────────────────────────────────┘
                │ Local IPC (unix socket / named pipe)
┌───────────────▼─────────────────────────────────────┐
│  PRIVILEGED HELPER — Rust, separate binary (P3)     │
│  hosts file edits · bind 80/443 · trust store       │
│  macOS: LaunchDaemon via SMAppService               │
│  Windows: Windows Service installed by installer    │
└─────────────────────────────────────────────────────┘
```
### 3.1 Component Responsibilities
**openserv-proc (Process Supervisor)** — the heart of the app:
- Spawn/stop/restart/status for every managed service; per-service state machine `Stopped → Starting → Running → Failed`.
- **Graceful shutdown:** SIGTERM→timeout→SIGKILL on macOS; `GenerateConsoleCtrlEvent`/Job Object termination on Windows.
- **Orphan cleanup:** persist PIDs + start timestamps in `state.db`; on app start, detect and kill stale processes from a previous crash (verify PID identity via process start time before killing).
- **Windows Job Objects (mandatory):** every child is assigned to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so services die with the app.
- Real-time stdout/stderr capture into ring buffers, streamed to the UI via Tauri events.
- Health checks (TCP connect / FastCGI ping / `mysqladmin ping`-equivalent) with restart policy.
**openserv-pkg (Package Manager):**
- Reads a signed manifest index from the `openserv/manifests` repo (cached locally).
- Downloads target-specific archives, verifies SHA-256, extracts to the packages directory.
- Layout mirrors ServBay's proven pattern: `packages/<name>/<major>/<full_version>/` with a **`current` link per major version** (symlink on macOS, **NTFS junction on Windows** ✅ — junctions need no admin rights).
- Minor versions within one major **share a config directory** (e.g., PHP 8.3.3 and 8.3.7 both use `config/php/8.3/`). This is a deliberate copy of ServBay's design — upgrades don't lose user config.
- Install / uninstall / upgrade / **disable-without-uninstall**.
**openserv-conf (Config Generator):**
- Tera templates → generated configs, written atomically (temp file + rename).
- Always runs the native validator before applying (`nginx -t`, `httpd -t`, `php-fpm -t`) and shows the user a **diff preview** before restart.
- Web server abstraction (see 4.3) so Caddy can be added later without touching core.
**openserv-core:** domain model (Site, ServicePackage, ServiceInstance, Certificate, HostsEntry), SQLite state, event bus. **Must not depend on Tauri** — this keeps it testable and lets `openservctl` reuse it. ✅
**Privileged helper (Phase 3):** smallest possible surface. Whitelisted operations only: managed-block hosts edits, trust-store install/remove, port 80/443 socket handoff. Every request authenticated (peer credential check) and logged. **MVP workaround (Phase 0–2): ports 8080/8443 + rely on `*.localhost` resolving to 127.0.0.1 in modern browsers/OSes — no privileges needed.** ✅
### 3.2 Runtime Directory Layout (user machine)
🟡 Proposed root: `~/.openserv` on macOS, `%USERPROFILE%\.openserv` on Windows (CLI-friendly, like `.cargo`/`.nvm`). ❓ Alternative: `C:\OpenServ` ServBay-style — decide before beta.
```
~/.openserv/
├── state.db                  # SQLite — sites, services, settings, PIDs
├── packages/
│   └── php/8.3/8.3.7/        # + `current` link per major
├── config/
│   ├── generated/            # DO-NOT-EDIT, regenerated idempotently
│   │   ├── nginx/  apache/  php/  mysql/  mariadb/
│   └── custom/               # user files, included by generated configs
│       ├── sites/<site>.conf
│       └── php/<major>/conf.d/
├── data/
│   ├── mysql/8.4/            # data dir per MAJOR version (ServBay pattern)
│   └── mariadb/11.4/
├── logs/
│   ├── services/<service>/
│   └── sites/<domain>/       # access.log + error.log per site
├── ssl/                      # P3: ca/ certs/
├── run/                      # pid files, sockets (macOS), port map (Win)
├── backup/                   # P3
└── www/                      # default document root suggestion
```
### 3.3 The Web Server Adapter (write in Phase 0, even with one impl)
```rust
#[async_trait]
pub trait WebServerAdapter: Send + Sync {
    fn id(&self) -> &'static str;                       // "nginx" | "apache" | "caddy"
    fn generate_main_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile>;
    fn generate_site_config(&self, site: &Site, ctx: &RenderCtx) -> Result<GeneratedFile>;
    async fn validate(&self, root: &Path) -> Result<ValidationReport>;   // nginx -t
    async fn reload(&self) -> Result<()>;               // hot reload if supported
    fn supports_hot_reload(&self) -> bool;
    fn supports_htaccess(&self) -> bool;
}
```
Phase 1 implements `NginxAdapter` + `ApacheAdapter`. `CaddyAdapter` lands in Phase 3 (auto-HTTPS + admin-API reload make SSL work much cheaper). ✅
### 3.4 PHP Execution Model — the #1 cross-platform challenge
| | macOS | Windows |
|---|---|---|
| Handler | PHP-FPM (upstream binary) | **PHP-FPM does not exist** → `php-cgi.exe` |
| Transport | Unix socket in `run/` | FastCGI over TCP `127.0.0.1:9xxx` |
| Pooling | FPM manages workers | **We build a pool manager in Rust** ✅ |
| Reload | `SIGUSR2` | Rolling restart of pool members |
The Windows pool manager (`openserv-proc::phpcgi_pool`) must: spawn N `php-cgi.exe -b 127.0.0.1:PORT` workers per (site × PHP version) group, allocate ports deterministically from `state.db`, set `PHP_FCGI_MAX_REQUESTS` and recycle workers, health-check via FastCGI ping, register every worker in the Job Object, and expose one upstream address list for the web server template. **This is the single highest-risk item and gates Phase 0 exit.**
---
## 4. Development Roadmap
### Phase 0 — Proof of Concept (2–4 weeks) — *risk burn-down, throwaway allowed*
Goal: prove the four riskiest assumptions. **If any fails, revisit the stack before writing more code.**
| ID | Task | Owner (agent) | Exit criterion |
|---|---|---|---|
| P0-1 | Scaffold monorepo, workspace, CI skeleton | main + ci-release-engineer | `cargo build` + `pnpm build` green on both OS in CI |
| P0-2 | Tauri app shell + SvelteKit page + one typed IPC command | tauri-frontend-engineer | Button in UI triggers Rust fn, result rendered |
| P0-3 | `openserv-proc` v0: spawn/stop/status + log capture | rust-core-engineer | Generic child process managed with state machine |
| P0-4 | macOS: nginx + php-fpm serve `phpinfo()` via unix socket | platform-macos-specialist | `curl http://127.0.0.1:8080` returns phpinfo |
| P0-5 | **Windows: Job Object wrapper + php-cgi pool prototype (3 workers)** | platform-windows-specialist | phpinfo served; killing app kills all workers; no orphans in Task Manager |
| P0-6 | Download→SHA-256 verify→extract pipeline | rust-core-engineer | One real PHP archive installed to `packages/` on both OS |
| P0-7 | Minimal Tera templates: nginx main + one site + php-fpm/php-cgi upstream | config-template-engineer | Generated config passes `nginx -t` on both OS |
| P0-8 | Orphan cleanup: PID persistence + stale-process kill on restart | rust-core-engineer | Kill app hard → relaunch → old services detected & reaped |
| P0-9 | Integration test harness: start→HTTP assert→stop | qa-test-engineer | One E2E test runs in CI on both OS |
**Phase 0 exit gate:** all nine green in CI, and a human has manually run the app on one real Mac and one real Windows machine.
Binary sourcing for P0/P1 (🟡 hybrid strategy): use existing static builds first — `static-php-cli` for macOS PHP, official windows.php.net builds for Windows PHP, official Nginx/Apache/MySQL/MariaDB distributions — and move to our own reproducible CI builds in Phase 2+.
### Phase 1 — MVP (target: replace XAMPP for a PHP dev)
- Package manager UI: install / uninstall / upgrade PHP·MySQL·MariaDB·Nginx·Apache versions from manifests.
- **PHP multi-version with per-site version selection** (the headline feature).
- Site CRUD: name, domain, root dir, web server (nginx/apache), PHP version; auto hosts-block management *within user-approved managed markers* (MVP: instruct user or use `*.localhost`; no privilege escalation yet).
- MySQL + MariaDB lifecycle: init datadir per major version, start/stop, port config, root password set/reset flow.
- Live log viewer (per service + per site), search/filter, tail-follow.
- System tray/menu-bar quick controls: start/stop all, per-service toggle.
- `openservctl` v1: `start|stop|restart|status|list` with `--json` (mirrors servbayctl verbs; add `reload`, `kill`, `stop-all` for parity).
- Config diff preview before every apply/restart.
- Uninstaller that leaves `www/` and `data/` untouched with a clear prompt.
### Phase 2 — Daily Driver (parity with ServBay's convenience layer)
phpMyAdmin/Adminer one-click (runs as a managed PHP site) · per-site custom directives + env vars · non-standard TLD sites · Open-in-IDE / Open-in-browser buttons · hosts-file GUI with pause(=comment)/enable · default CLI version + versioned shims (`php`, `php-8.3`) on PATH · rewrite/htaccess docs+templates · start-on-boot · per-service CPU/RAM metrics · disable-package-without-uninstall · i18n framework with **EN + TH** · own reproducible package build pipeline begins.
### Phase 3 — Competitive Edge
Privileged helper (hosts, 80/443, trust store) with security audit gate · local CA + one-click HTTPS per site (`rcgen`) · CaddyAdapter · dnsmasq/built-in DNS with **wildcard domains** · Mailpit integration · reverse-proxy & CORS UI per site · backup/restore (config+db+ssl+www) · DB major-version upgrade wizard with data migration · `openserv.yaml` project file (env-as-code) · signed auto-update.
### Phase 4+ — Expansion
PostgreSQL, Redis, MongoDB, Memcached · Node.js/Python/Go runtimes · ACME (Let's Encrypt/ZeroSSL) · tunnels (cloudflared/frp/ngrok) · Linux support · community manifest submissions with review pipeline.
---
## 5. Engineering Conventions (all agents must follow)
**Rust:** edition 2021+, workspace-level lints; `cargo fmt` + `clippy -D warnings` gate CI; no `unwrap()`/`expect()` outside tests (use `thiserror` in library crates, `anyhow` at binary edges); `tracing` for all logs (no `println!`); all platform-specific code isolated behind `#[cfg]` inside `platform/` modules with a common trait facade — **core crates never contain inline OS branches**; every spawned process registered with the supervisor (no ad-hoc `Command::spawn` elsewhere); file writes atomic (tmp + rename); paths via `PathBuf` only, never string concat.
**Frontend:** TypeScript strict; ESLint + Prettier gate; all IPC through generated typed bindings — no stringly-typed `invoke("...")` calls; UI must render meaningfully when a service is in `Failed` state (error surfaces, never silent).
**Git:** trunk-based, short-lived branches, Conventional Commits (`feat:`, `fix:`, `refactor:`…), PR template includes platform-test checklist (macOS ✅ / Windows ✅ / n/a).
**Security invariants:** every download SHA-256-verified against the manifest before extraction; manifest index signature verified (minisign/ed25519) 🟡; helper accepts only whitelisted ops from an authenticated local peer; no secrets in logs; CA private key stored via OS keychain/credential manager (P3).
**License compliance (see §1.3):** new source files carry `// SPDX-License-Identifier: GPL-3.0-or-later`; commits are DCO-signed (`git commit -s`); adding any dependency requires the CI license gate to pass (`cargo-deny` licenses check / npm license checker) — GPL-incompatible or unknown-license deps are rejected; never link native MySQL/MariaDB client libraries.
**Definition of Done (any task):** code + tests + docs updated · CI green on both OS · clippy/eslint clean · license gate green · platform specialist sign-off if `#[cfg]` code touched · **security-auditor sign-off if the change touches: helper, cert, download-verification, hosts-file, or IPC-ACL code** · user-visible strings go through i18n layer (from Phase 2).
---
## 6. Subagent Allocation for Claude Code
This project uses **8 specialized subagents** plus the main thread as orchestrator. Definitions below are ready to save as `.claude/agents/<name>.md` in the repo. First task for Claude Code: create these files verbatim.
### 6.1 Coordination Model
- **Main thread = Orchestrator/Architect.** Reads this plan, decomposes work into tasks, delegates to subagents, integrates results, resolves cross-cutting design questions, and owns `docs/` and this plan file. The main thread writes code directly only for small glue changes; anything substantial is delegated.
- **Ownership is by path** (table below). An agent may *read* anything but should only *write* inside its owned paths; cross-boundary changes are split into per-agent tasks by the orchestrator.
- **Mandatory consultations:**
  - Any new cross-platform abstraction → design reviewed by **both** platform specialists *before* implementation (Windows constraints have killed clean designs before — e.g., no FPM, no fork, junction vs symlink).
  - Any change under security-sensitive paths → **security-auditor** review is a merge blocker.
- **Parallelization guide:** platform specialists can work simultaneously on the two sides of one trait; frontend + core can proceed in parallel once the IPC contract (command signatures) is frozen for the task; config-template work parallelizes with proc work once the `RenderCtx` shape is agreed.
- **Escalation to human:** anything in §7 Open Questions, any license/legal judgment, any spend (certs, infra), any decision to bundle (rather than download) a third-party binary.
### 6.2 Ownership Map
| Path | Owner | Reviewer(s) |
|---|---|---|
| `crates/openserv-core/`, `crates/openserv-proc/` (common), `crates/openserv-pkg/`, `apps/cli/` | rust-core-engineer | platform specialists for `#[cfg]` parts |
| `crates/*/src/platform/windows*`, php-cgi pool, Job Objects, junctions, named pipes, NSIS | platform-windows-specialist | rust-core-engineer |
| `crates/*/src/platform/macos*`, launchd, SMAppService, keychain, notarization | platform-macos-specialist | rust-core-engineer |
| `apps/desktop/src/` (SvelteKit), `apps/desktop/src-tauri/src/commands/` | tauri-frontend-engineer | rust-core-engineer for command signatures |
| `templates/**`, `crates/openserv-conf/` | config-template-engineer | platform specialists for path/socket differences |
| `.github/workflows/**`, `packaging/**`, manifests tooling | ci-release-engineer | security-auditor for signing/verification steps |
| `crates/openserv-helper/`, `crates/openserv-cert/`, download-verification code, hosts-file code | rust-core-engineer *implements*, **security-auditor gates** | — |
| `tests/**`, test harnesses, fixtures | qa-test-engineer | — |
| `docs/**`, `CLAUDE.md`, this plan | main thread | — |
### 6.3 Agent Definitions (save each as `.claude/agents/<name>.md`)
#### `.claude/agents/rust-core-engineer.md`
```markdown
---
name: rust-core-engineer
description: >
  Core Rust engineer for OpenServ. Use PROACTIVELY for any work in
  crates/openserv-core, openserv-proc (cross-platform parts), openserv-pkg,
  openserv-conf glue, or the openservctl CLI: domain model, SQLite state,
  process-supervisor state machine, download/verify/extract pipeline,
  event bus, error handling. Not for platform-#[cfg] internals (delegate
  to platform specialists) and not for UI.
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the core Rust engineer for OpenServ, an open-source local dev
environment orchestrator (Tauri 2 + Rust workspace + SvelteKit).
Hard rules:
- openserv-core must NEVER depend on tauri. It is consumed by both the
  desktop app and the openservctl CLI.
- Supervisor state machine: Stopped → Starting → Running → Failed, with
  restart policy and health checks. Every child process in the entire
  codebase is spawned through openserv-proc — reject ad-hoc spawns.
- Persist PID + process start-time in state.db; on boot, reap orphans only
  after verifying PID identity via start-time (PIDs get reused).
- Package layout: packages/<name>/<major>/<full>/ with a `current` link
  per major (symlink on macOS, NTFS junction on Windows — call the
  platform facade, never create links inline). Config is shared per MAJOR
  version by design; never key config paths on the full version.
- Downloads: stream to temp, verify SHA-256 against the manifest BEFORE
  extraction, extract to temp dir, atomic rename into place.
- Errors: thiserror in lib crates, anyhow only in binaries. No unwrap()
  outside tests. tracing for logs.
- All platform-specific behavior goes through traits in a platform/ module;
  you define the trait, the platform specialists implement it. When you
  need a new platform capability, write the trait + a stub and hand off.
- File writes are atomic (tmp + rename). Paths are PathBuf, never strings.
Security-sensitive paths you also implement (helper, cert, hosts-file,
download verification) are MERGE-BLOCKED until security-auditor approves —
say so explicitly in your task summary when you touch them.
Definition of done: unit tests included, cargo fmt + clippy -D warnings
clean, doc comments on public items, CI-relevant notes surfaced.
```
#### `.claude/agents/platform-windows-specialist.md`
```markdown
---
name: platform-windows-specialist
description: >
  Windows platform expert. MUST BE USED for any code touching
  #[cfg(windows)], Windows Job Objects, the php-cgi FastCGI pool manager,
  NTFS junctions, Named Pipes + ACLs, Windows Services, console control
  events, NSIS/installer behavior, PATH/shim handling on Windows, or
  Windows-only bugs. Also consult BEFORE finalizing any cross-platform
  abstraction to verify Windows feasibility.
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the Windows platform specialist for OpenServ.
Context you must never forget:
- PHP-FPM does not exist on Windows. PHP runs as php-cgi.exe workers that
  YOU pool: spawn N workers per (site × PHP version), bind
  127.0.0.1:<port> with ports allocated deterministically and recorded in
  state.db, set PHP_FCGI_MAX_REQUESTS and recycle workers, FastCGI-ping
  health checks, rolling restart for config reload (no SIGUSR2 here).
- EVERY spawned process must be added to the app's Job Object created with
  JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, so services die with the app. A PR
  that spawns outside the Job Object is a bug.
- Graceful stop order: try GenerateConsoleCtrlEvent / service-specific
  clean shutdown (e.g. mysqladmin-equivalent) → wait → TerminateJobObject.
  Databases must get a clean shutdown path; document any forced-kill risk.
- Use NTFS junctions (no admin needed) for the packages/<major>/current
  link — never symlinks (they require Developer Mode/admin).
- Named Pipe IPC (Phase 3 helper): create with an explicit ACL restricted
  to the current user + the service SID; reject default ACLs.
- Long-path awareness (\\?\ prefix where needed), spaces in %USERPROFILE%,
  Defender/AV false-positive mitigation notes for release docs.
- Implement the platform traits defined by rust-core-engineer inside
  platform/windows modules; keep everything else OS-agnostic.
When a clean cross-platform design conflicts with Windows reality, say so
loudly and propose the Windows-compatible alternative rather than
silently degrading behavior.
```
#### `.claude/agents/platform-macos-specialist.md`
```markdown
---
name: platform-macos-specialist
description: >
  macOS platform expert. MUST BE USED for any code touching
  #[cfg(target_os = "macos")], unix sockets, signal handling
  (SIGTERM/SIGUSR2), launchd/LaunchAgents, SMAppService privileged-helper
  registration, Keychain storage, trust-store (`security` cmd) operations,
  app sandbox/entitlements, .dmg packaging, codesigning/notarization, or
  Apple Silicon specifics. Also consult BEFORE finalizing any
  cross-platform abstraction.
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the macOS platform specialist for OpenServ.
Context you must never forget:
- PHP runs under upstream php-fpm listening on a unix socket in
  ~/.openserv/run/; config reload via SIGUSR2; stop via SIGTERM → grace
  period → SIGKILL. Reap children correctly; no zombies.
- Process-group management: spawn services in their own process group so
  a whole tree can be signaled; on app start, reap stale processes
  recorded in state.db (verify PID + start time first).
- Phase 3 helper: LaunchDaemon registered via SMAppService (macOS 13+),
  IPC over unix socket with peer-credential (SO_PEERCRED-equivalent /
  audit token) checks; helper does ONLY whitelisted ops: managed hosts
  block, trust-store add/remove, 80/443 socket handoff.
- Trust store: `security add-trusted-cert -d -r trustRoot -k
  /Library/Keychains/System.keychain` requires the helper; user-keychain
  fallback documented for MVP.
- CA private key (Phase 3) lives in Keychain, never on disk in plaintext.
- Notarization/codesigning constraints affect how we spawn downloaded,
  unsigned service binaries — verify quarantine-attribute handling
  (com.apple.quarantine must be cleared on extracted packages or
  Gatekeeper will block execution) and document the chosen approach.
- Implement platform traits from rust-core-engineer in platform/macos
  modules; Apple-Silicon-first, note anything Intel-specific separately.
```
#### `.claude/agents/tauri-frontend-engineer.md`
```markdown
---
name: tauri-frontend-engineer
description: >
  Frontend + IPC engineer. Use for all SvelteKit UI work in
  apps/desktop/src, Tauri command/event definitions in
  apps/desktop/src-tauri/src/commands, typed-binding generation
  (tauri-specta), UI state stores, tray menu, and UX flows (site panel,
  service panel, log viewer, diff preview, package manager UI).
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the frontend engineer for OpenServ (SvelteKit + Svelte 5 +
TypeScript strict + Tailwind + shadcn-svelte, inside Tauri 2).
Hard rules:
- All IPC is typed: define Tauri commands thinly (validate + call into
  openserv-core), regenerate TS bindings, and consume ONLY the generated
  client. No raw invoke("string") calls.
- Commands stay thin — business logic belongs in Rust crates. If you find
  yourself writing logic in a command handler, hand the logic to
  rust-core-engineer and call it.
- Long-running state (service status, log lines) arrives via Tauri
  events into Svelte stores; UI must render every ServiceState including
  Failed (show stderr tail + suggested action), and must stay responsive
  while services start/stop.
- Destructive/irreversible actions (delete site, uninstall package,
  reset DB password, apply config) always show a confirm with a diff or
  consequence summary. Deleting a site NEVER deletes project files — the
  copy must say so.
- Log viewer: virtualized list, follow-tail toggle, per-site and
  per-service tabs, filter box. Performance target: 10k lines smooth.
- Keep the UI functional at 380px width panels and honor OS light/dark.
- i18n: from Phase 2 every user-visible string goes through the i18n
  layer (EN + TH first); until then, keep strings centralized to ease
  extraction.
- Design tokens, colors, typography, and microcopy follow
  docs/OPENSERV_BRAND_GUIDELINES.md (tokens.css becomes the single
  source of truth once it lands) — read it before any user-visible work.
```
#### `.claude/agents/config-template-engineer.md`
```markdown
---
name: config-template-engineer
description: >
  Owner of all generated-config work: Tera templates under templates/**,
  the openserv-conf crate, config validation/diff/apply pipeline, and
  per-service config knowledge (nginx.conf, httpd.conf, vhosts, php.ini,
  php-fpm pools, my.cnf). Use for adding a new service's config surface,
  changing generated output, or fixing template/OS-path issues.
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the configuration/template engineer for OpenServ.
Hard rules:
- Every generated file begins with the standard DO-NOT-EDIT banner that
  names the exact custom-config path the user should edit instead, and
  generated configs `include` the user's custom files where the format
  allows (nginx include, Apache IncludeOptional, php.ini scan dir).
- Generation is a pure function of (state.db snapshot + templates):
  same input ⇒ byte-identical output. Never read previous generated
  output as input. Write atomically; apply = validate → show diff →
  swap → reload/restart.
- Always run the native validator before apply: nginx -t, httpd -t,
  php-fpm -t (macOS), and surface its stderr verbatim on failure.
- PHP upstream differs by OS and MUST come from RenderCtx, never be
  hardcoded: unix socket path on macOS, 127.0.0.1:port list (php-cgi
  pool) on Windows. Same for path separators, log paths, pid paths.
- Config directories are per MAJOR version (php/8.3, mysql/8.4) shared
  across minors — templates must not embed full versions in paths.
- MySQL vs MariaDB my.cnf have diverged; keep separate template trees,
  do not share includes between them beyond truly common fragments.
- Follow the WebServerAdapter trait boundaries; adding Caddy later must
  not require touching nginx/apache trees.
- Each template ships with a golden-file test (rendered output snapshot
  per OS) maintained with qa-test-engineer.
```
#### `.claude/agents/ci-release-engineer.md`
```markdown
---
name: ci-release-engineer
description: >
  CI/CD, packaging, and release owner. Use for GitHub Actions workflows,
  the macOS+Windows build matrix, caching, Tauri bundling (.dmg/NSIS),
  codesigning + notarization pipelines, the signed auto-updater (P3),
  release automation, and tooling for the openserv/manifests package
  index (schema validation, checksum generation, signing).
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the CI/release engineer for OpenServ.
Hard rules:
- CI matrix: macos-14 (arm64) + windows-latest. Every PR runs fmt,
  clippy -D warnings, cargo test, eslint, frontend build, and the
  integration suite on BOTH OSes. A PR is not mergeable with one OS red.
- Cache aggressively (cargo, pnpm) but never cache across toolchain
  bumps incorrectly; keep cold-build time documented.
- Secrets discipline: signing keys/notarization creds only via GitHub
  encrypted secrets + environment protection rules; forks must not
  receive signing secrets (use pull_request, not pull_request_target,
  for untrusted code). Release workflows are tag-triggered and
  environment-gated.
- Artifacts: unsigned dev builds on every main commit; signed .dmg +
  NSIS .exe on tags. Checksums (SHA-256) published next to every
  artifact. Updater manifests signed.
- Manifests tooling: JSON-schema-validate every package manifest, verify
  the referenced archive's checksum, and (P2+) sign the index
  (minisign/ed25519). Reject unsigned/unverifiable entries in CI.
- License compliance gate (project is GPL-3.0-or-later, see plan §1.3):
  run cargo-deny (licenses) + an npm license checker on every PR; block
  GPL-incompatible or unknown licenses; enforce SPDX headers and DCO
  sign-off checks.
- GPL source-offer duty: from the moment we distribute OUR OWN builds of
  GPL packages (MySQL/MariaDB etc., Phase 2+), every release must
  publish the corresponding source tarball + build scripts alongside
  the binary artifact. Until then, manifests must point at official
  upstream downloads so we are not the distributor.
- Own the Phase 2+ package-build pipeline design (reproducible builds of
  PHP/nginx/etc. for our 3–4 targets) — start a docs/build-pipeline.md
  ADR before implementing.
- Any workflow step that downloads-and-executes third-party code must be
  version-pinned by commit SHA.
```
#### `.claude/agents/security-auditor.md`
```markdown
---
name: security-auditor
description: >
  Read-only security reviewer. MUST BE USED before merging any change to:
  crates/openserv-helper (privileged helper), crates/openserv-cert,
  download-and-verify code in openserv-pkg, hosts-file editing code,
  IPC endpoints/ACLs (named pipes, unix sockets, Tauri command surface),
  installer scripts, and CI signing/release workflows. Produces a
  written verdict: APPROVE or BLOCK with required changes.
tools: Read, Grep, Glob, Bash
---
You are the security auditor for OpenServ. You review; you do not write
feature code. Your BLOCK is a merge blocker.
Threat model highlights:
- The privileged helper is the crown jewel: it must expose ONLY a fixed
  whitelist (managed hosts block, trust-store add/remove, 80/443 socket
  handoff), authenticate the local peer (audit token / SO_PEERCRED on
  macOS, pipe ACL + client PID→image verification on Windows), validate
  every argument (e.g., hosts entries restricted to 127.0.0.1/::1 and a
  managed marker block), and log every operation. Any generic
  "run this command" capability is an automatic BLOCK.
- Supply chain: package archives must be SHA-256-verified against a
  (signed, once available) manifest BEFORE extraction; archive
  extraction must be zip-slip/path-traversal safe and must strip or
  refuse unexpected symlinks; downloads over HTTPS only with cert
  verification on.
- Local CA: private key in OS keychain/credential store, never plaintext
  on disk; leaf certs only for local/dev domains; UI must warn about the
  risk of installing a root CA; uninstall must offer CA removal.
- IPC surface: every Tauri command is reachable from the webview — audit
  for path traversal (root dirs, log paths), command injection into
  spawned processes, and unbounded resource use.
- Config generation: template inputs are user-controlled (site names,
  paths, custom directives) — check escaping so a site name cannot
  inject nginx/apache directives outside its scope.
- Secrets: no tokens/passwords in logs or state.db plaintext where the
  OS keystore is available.
Review output format: risk summary, findings ranked
(Critical/High/Med/Low) with file:line, concrete fixes, and final
APPROVE/BLOCK.
```
#### `.claude/agents/qa-test-engineer.md`
```markdown
---
name: qa-test-engineer
description: >
  Test engineer. Use for designing and implementing unit/integration/E2E
  tests, the cross-platform process-supervision test harness, golden-file
  tests for generated configs, CI test reliability (flake hunting), and
  regression tests for every fixed bug. Invoke after features land and
  proactively when a plan lacks test coverage.
tools: Read, Edit, Write, Bash, Grep, Glob
---
You are the QA/test engineer for OpenServ.
Testing strategy:
- Unit tests live beside code (Rust #[cfg(test)], Vitest for TS logic).
- The hard, valuable layer is integration: a harness that installs a
  pinned PHP+nginx fixture into a temp OPENSERV_HOME, starts services
  through the real supervisor, asserts HTTP responses (phpinfo, vhost
  routing, per-site PHP version), then verifies clean shutdown — and
  CRUCIALLY asserts zero orphan processes afterward on both OSes
  (enumerate by Job Object on Windows, process group on macOS).
- Crash-recovery test: SIGKILL/TerminateProcess the app mid-run,
  relaunch, assert stale services are detected and reaped.
- Golden-file tests: rendered template output per (service × OS)
  snapshot-compared; update requires explicit snapshot review.
- php-cgi pool tests (Windows): worker recycling after
  PHP_FCGI_MAX_REQUESTS, port-conflict handling, health-check restart.
- Never sleep-and-hope: poll with timeouts; make tests hermetic via
  OPENSERV_HOME env override; every bug fix ships with a regression
  test named after the issue.
- Track and fix flaky tests immediately; a flaky suite is a broken suite.
```
### 6.4 Repo-root `CLAUDE.md` (create verbatim)
```markdown
# OpenServ — Claude Code Project Instructions
Read docs/OPENSERV_MASTER_PLAN.md before any non-trivial task. It is the
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
4. openserv-core must never depend on tauri. All child processes go
   through openserv-proc. All file writes are atomic. No unwrap outside
   tests.
5. Both OSes green in CI or it doesn't merge.
6. Never bundle service binaries into the installer — runtime download
   with SHA-256 verification only (license + security).
7. License is GPL-3.0-or-later (plan §1.3): SPDX headers on new files,
   DCO sign-off, license-gate must pass for any new dependency, never
   link native MySQL/MariaDB client libs (separate processes only).
8. Anything listed in plan §7 (Open Questions) needs a human decision —
   stop and ask.
## Commands
- Build all: `cargo build --workspace && pnpm -C apps/desktop build`
- Test: `cargo test --workspace` · `pnpm -C apps/desktop test`
- Lint gate: `cargo fmt --check && cargo clippy --workspace -- -D warnings`
- Run app (dev): `pnpm -C apps/desktop tauri dev`
- Conventional Commits required.
```
### 6.5 Example Task Flows
**"Implement per-site PHP version switching" (Phase 1 flagship):**
1. Orchestrator freezes the contract: `Site.php_major` field + `set_site_php_version` command signature + `RenderCtx` addition.
2. rust-core-engineer: model + state migration + supervisor mapping (site → pool/FPM instance).
3. In parallel — platform-macos-specialist: per-version FPM pool files + socket naming; platform-windows-specialist: per-(site×version) php-cgi pool groups + port allocation.
4. config-template-engineer: nginx/apache upstream blocks consuming RenderCtx on both OSes.
5. tauri-frontend-engineer: version dropdown, restart prompt with diff preview.
6. qa-test-engineer: E2E — two sites, two PHP versions, assert each phpinfo reports the right version on both OSes.
7. Orchestrator integrates, updates docs.
**"Add the privileged helper" (Phase 3):** design ADR by orchestrator → security-auditor reviews the *design first* → platform specialists implement per-OS registration/IPC → rust-core-engineer implements whitelisted ops → security-auditor code review (BLOCK authority) → ci-release-engineer wires signing → qa-test-engineer adds ACL/abuse tests.
---
## 7. Open Questions Requiring Human Decision (❓)
> ✅ Resolved 2026-07-21 — **License = GPL-3.0-or-later** (strong copyleft per owner's requirement; AGPL rejected — see §1.3). This replaces former OQ#1.
| # | Question | Default if unanswered | Deadline |
|---|---|---|---|
| 1 | Inbound policy: DCO-only (license permanently locked to GPL — no one, including owner, can relicense) vs CLA (keeps dual-licensing option, adds contributor friction)? | **DCO-only** (aligns with owner's always-open intent) | Before first external PR |
| 2 | Intel macOS support in v1? | No (Apple Silicon only) | Before Phase 1 |
| 3 | Windows ARM64 in v1? | No | Before Phase 1 |
| 4 | Install root: `~/.openserv` vs `C:\OpenServ`-style? | `~/.openserv` both OSes | Before beta |
| 5 | Who holds Apple Developer ($99/y) + Windows signing cert (~$100–400/y)? | Unsigned dev builds until resolved | Before first public release |
| 6 | Legal counsel review of licensing model: GPL-3.0-or-later app + runtime-download of GPL-2.0 DB binaries (mere aggregation) + Phase 2+ source-offer plan (§1.3) | Proceed per §1.3, flagged for counsel | Before first public release |
| 7 | Caddy promoted into Phase 1? | No — adapter in P0, Caddy impl in P3 | Phase 1 planning |
| 8 | Brand/name check ("OpenServ" trademark/domain availability) + publish `TRADEMARK.md` | Proceed provisionally | Before public announcement |
## 8. Reference Notes (from ServBay docs analysis, 2026-07)
Patterns deliberately adopted: per-major shared config dirs · `current` version link · data dir per DB major version · pause-by-comment for hosts entries · generated-config warnings (we improve with in-file banners + diff preview) · `servbayctl` verb set (`start|stop|reload|restart|kill|status|stop-all`) mirrored in `openservctl` with added `--json`.
Patterns deliberately rejected: account requirement · closed manifests · bundled binaries in installer.
— End of document —
