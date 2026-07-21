# Design

> Visual system for OpenVHost product UI. Canonical source: `docs/OPENVHOST_BRAND_GUIDELINES.md` v1.1 — this file operationalizes it for implementation; on conflict the brand guidelines win. The app's `tokens.css` does not exist yet; the mockups in `docs/design/` prototype it.

## Theme

Two themes, both first-class, following the OS (`prefers-color-scheme`) with a manual override later. Light mode: Plaintext surfaces with Terminal Ink text and Evergreen accents. Dark mode: Terminal Ink surfaces, Panel cards, Plaintext text, Evergreen Bright accents. Working surfaces stay quiet — color means either brand (one accent) or service state (four semantic colors), never decoration.

## Color

Token prefix `--vh-` (brand §7.1). Values are the brand's committed hex; convert to OKLCH at `tokens.css` implementation time without perceptual drift.

Core palette:

| Token | Value | Light mode role | Dark mode role |
|---|---|---|---|
| `--vh-ink` | `#171A21` | primary text | background |
| `--vh-paper` | `#F7F6F2` | background | primary text |
| `--vh-brand-600` | `#0E6E5C` | primary accent: buttons, links, active nav, logomark | — |
| `--vh-brand-700` | `#0A5346` | hover/pressed; body-size links | — |
| `--vh-brand-400` | `#33B79E` | — | primary accent |
| `--vh-brand-100` | `#DDF1EC` | selected rows, subtle fills | — |
| `--vh-neutral-600` | `#5C636E` | secondary text | — |
| `--vh-neutral-400` | `#8A9099` | disabled text, borders | disabled text, borders |
| `--vh-neutral-800` | `#22262F` | — | cards/panels |

Semantic state colors (map 1:1 to the supervisor state machine; never used for anything but state; on light backgrounds use the text-safe value for text):

| State | On dark / as fill | Text-safe on light |
|---|---|---|
| running | `#3FB950` | `#2E8B3D` |
| starting | `#D99A2B` | `#9A6B14` |
| failed | `#E5534B` | `#C13832` |
| stopped | `#8A9099` | `#5C636E` |

Signal Green (`#3FB950`) is deliberately brighter/yellower than Evergreen so "brand" and "running" never read as the same thing. If a composition makes them ambiguous, add the state label — color is never the only carrier.

## Typography

| Role | Family | Notes |
|---|---|---|
| UI & body | IBM Plex Sans (400/500/600) | Thai companion: IBM Plex Sans Thai Looped |
| Display / page-level headings only | Space Grotesk (500/700) | never body, tables, or logs |
| Code / logs / configs / ports / paths | JetBrains Mono (400/700) | ligatures OFF; tabular-nums for metrics and ports |

Fixed rem scale (product register — no fluid clamp): 12 caption · 13 dense-table · 14 body (default) · 16 section title · 20 page title · 28 display. Line-height 1.5 body, 1.2 headings. Sentence case everywhere; no Title Case; ALL CAPS only for tiny 11px eyebrow labels (+6% tracking, Slate) used sparingly. Logs render at 12.5–13px.

## Spacing, radius, elevation

- 4px base grid. Density is welcome in tables and service rows; prose keeps 65–75ch measure.
- Radius: 6px controls · 10px cards/panels · 999px status pills. Nothing else.
- Elevation is minimal: 1px borders (`--vh-neutral-400` at reduced alpha) over shadows; one soft shadow tier for overlays (dialog/drawer) only.
- Focus ring: 2px `--vh-brand-400` outer ring on every interactive element, visible in both themes.

## Components (vocabulary)

- **Status pill**: 999px radius, state dot + label, never color-alone. The one place the dot color changes meaning is the tray icon (brand §7.3).
- **Service row / site row**: dense rows in a list, not card grids. Monospace for ports, versions, domains, paths.
- **Buttons**: primary = Evergreen fill (light) / Evergreen Bright (dark); secondary = quiet border; destructive = failed-red only when the action destroys something, with consequence copy.
- **Failed state block**: state color + stderr tail (JetBrains Mono) + forward actions ("View log · Change port"). Never a bare error.
- **Diff preview**: unified diff, monospace, add/remove tints; approve/cancel is an explicit gate before any config apply.
- **Empty states** teach the interface ("No sites yet. Add your first site to map a domain to a project folder.") — never jokes.
- Every interactive component defines: default, hover, focus-visible, active, disabled, loading, error.

## Motion

150–250ms, ease-out (quart/expo family), state-conveying only: state transitions, feedback, reveal of panels/drawers. No page-load choreography, no decorative motion. Every animation has a `prefers-reduced-motion: reduce` fallback (crossfade or instant).

## Voice in UI

Microcopy follows brand §6 verbatim where examples exist: `Start service`, `nginx running on :8080`, failure copy that names the cause and the next action. Technical nouns, service names, and paths stay in English in both languages (TH arrives Phase 2).
