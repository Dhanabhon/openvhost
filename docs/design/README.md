# OpenVHost UI mockups

Static HTML design mockups — open any file directly in a browser. **Light theme only** (owner decision, 2026-07-21); note that brand guidelines §4.3 still specify a dark theme shipping at launch, so a `[data-theme='dark']` block gets added back to the tokens when that work starts. These pages prototype the future `apps/desktop/src/lib/tokens.css`: `tokens.css` here is the token source of truth candidate, `mock.css` is throwaway chrome.

| File | Screen | Shows |
|---|---|---|
| `main-window.html` | Sites + services overview | Row-density app shell, all four service states incl. an expanded mysql failure with stderr tail + forward actions |
| `site-editor.html` | Edit-site drawer | Per-site PHP version switch (headline feature), `.localhost` domain affordance, delete with consequence copy |
| `log-viewer.html` | Log viewer | JetBrains Mono log surface, per-source tabs, level colors, follow-tail pinned to newest line |
| `diff-preview.html` | Config diff gate | Validate → diff → apply flow with honest reload note |

Design decisions and constraints come from `../OPENVHOST_BRAND_GUIDELINES.md`, `/PRODUCT.md`, and `/DESIGN.md`.

**Documented exceptions** (impeccable detector): Space Grotesk is flagged as an overused face generally — here it is the brand-committed wordmark face (guidelines §5) and is used for the wordmark only; the "numbered section markers" hit in `log-viewer.html` is a false positive on log timestamps.

## What these screens demand from the supervisor (input for the P0-3 spec)

Derived from what the UI actually renders — the concrete consumer contract for `openvhost-proc` v0:

- **Per-service snapshot:** `name`, `version`, `endpoint` (host:port or socket path), `state` (`stopped | starting | running | failed`), and for `failed` a bounded **stderr tail** (the UI shows the last ~3–10 lines verbatim).
- **State-change events** pushed to the UI (site rows, service rows, titlebar aggregate, future tray all react); aggregate state = failed > starting > running > stopped.
- **Log stream events** per source: timestamp (ms precision), level (`info | warn | error`), message; sources are both services and sites; the UI also needs line count and the on-disk log path (statusline).
- **Supervisor-authored log lines** for lifecycle transitions ("state Stopped → Starting (requested by user)", "port 3306 still bound by pid 3391 (mysqld, not managed by OpenVHost)") — failures must carry evidence and next-step context, not just codes.
- **Actions:** start / stop / retry per service, stop-all; starting state must be cancel-safe; failed state keeps the service addressable (Retry).
- **Config-apply flow hooks:** validate (native validator output surfaced verbatim), unified diff of generated files, apply-then-reload with graceful/interrupting distinction the UI can state honestly.
