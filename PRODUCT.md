# Product

> Distilled from `docs/OPENVHOST_MASTER_PLAN.md` (v1.2) and `docs/OPENVHOST_BRAND_GUIDELINES.md` (v1.1) — those two documents are canonical; if this file ever disagrees with them, they win.

## Register

product

## Platform

web

(Tauri 2 desktop app rendering web technology on macOS and Windows — platform conventions come from the brand guidelines, not HIG/Material.)

## Users

Web developers — PHP-first at launch — on macOS (Apple Silicon) and Windows (x64). The app runs all day next to an IDE and a browser; users drop in briefly to start/stop services, create a site, switch a PHP version, read a log, or approve a config change, then go back to their real work. They are fluent in the category's tools (ServBay, Laragon, XAMPP, MAMP, DDEV) and in terminals; they distrust magic and accounts.

## Product Purpose

OpenVHost is an open-source local dev environment orchestrator: native service binaries (PHP multi-version, MySQL, MariaDB, Nginx, Apache), no Docker, no admin rights for the MVP path. Success for Phase 1 is a PHP developer replacing XAMPP with it. Non-negotiables: idle RAM under 100 MB; deleting a site never touches project files; generated config is strictly separated from user config and always previewed as a diff before apply.

## Positioning

The local dev environment that can never be closed — GPL-3.0-or-later plus DCO-only contributions make it structurally impossible for anyone, including its own maintainers, to take it proprietary. Free, no account, no telemetry.

## Brand Personality

A senior dev on your team: direct, calm, technically honest, never salesy, quietly funny at most. Trustworthy before impressive. Fast, light, transparent, community-owned. Voice rules live in brand guidelines §6 (dev-to-dev plain verbs; errors explain and point forward; honest about consequences; no hype vocabulary — "blazingly/supercharge/magical/seamless/revolutionary" are banned; sentence case everywhere).

## Anti-references

- Not corporate, not gamified, never mysterious about errors, never hungry for accounts or data (brand §1.1).
- Not blue. The category is saturated with it (ServBay navy, Laragon blue, DDEV blue/purple); OpenVHost owns deep green (Evergreen) instead (brand §1.2).
- Not "Success! 🎉" software — no emoji toasts, no "Oops! Something went wrong", no empty-state jokes (brand §6.2 don't-column).

## Design Principles

1. **Failed is never silent.** Every failure states what happened, shows the evidence (stderr tail), and offers the next action.
2. **Honest about consequences.** Destructive dialogs say exactly what is and isn't affected — "Your project files are not touched" is product law.
3. **State is sacred.** The four semantic state colors (running/starting/failed/stopped) mean state and nothing else, and state is never carried by color alone.
4. **Logs and configs are first-class surfaces.** They render in JetBrains Mono with ligatures off; config text must be unambiguous.
5. **Quiet surfaces, deliberate boldness.** Evergreen is the only brand accent; spend boldness on the mark and typography, keep working surfaces calm.

## Accessibility & Inclusion

WCAG 2.1 AA: ≥4.5:1 body text, ≥3:1 large text; use the brand's "text-safe on light" state colors for text. State always pairs color with a label or icon shape. Light and dark themes both ship and follow the OS; every token is defined in both modes — no ad-hoc hexes in app code. Reduced motion is respected on every animation.
