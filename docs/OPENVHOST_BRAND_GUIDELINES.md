# OpenVHost — Brand Guidelines v1.1

> Companion to `OPENVHOST_MASTER_PLAN.md` (§1.3 licensing, OQ#8 registration actions) and the input spec for `tauri-frontend-engineer` design tokens and `TRADEMARK.md`.
> Status: v1.1 — 2026-07-21 (renamed from OpenServ; tagline & descriptor finalized) · All specified typefaces are SIL OFL licensed (redistributable in a GPL project).

---

## 1. Brand Foundation

### 1.1 Essence

**OpenVHost is the local dev environment that can never be closed.**

The license (GPL-3.0-or-later, DCO-only) makes it structurally impossible for anyone — including the project's own maintainers — to take OpenVHost proprietary. The brand exists to make that promise *felt*, not just stated in a COPYING file.

| | |
|---|---|
| **Tagline** | **"Open. We host."** — the "V" is pronounced "vee", which reads as *we*: openness and community in three words. This is the official tagline for the social preview, website hero, and launch materials. |
| **README descriptor** | **"Your friendly local host"** — double pun (*local* dev environment / welcoming *host*); English-only wordplay, used as the one-line description under the README header and in store/app listings. |
| **Brand narrative** | *The bracket never closes.* Our mark is an opening bracket that is never paired with a closing one — the source stays open, permanently, by design. The letter **V** in the name carries the same story: an open shape that never seals shut. |
| **Personality** | A senior dev on your team: direct, calm, technically honest, never salesy, quietly funny at most. Trustworthy before impressive. |
| **What we are** | Fast, light, transparent, community-owned. |
| **What we are not** | Corporate, gamified, mysterious about errors, hungry for accounts or data. |

### 1.2 Positioning (visual territory)

| Competitor | Their color territory | 
|---|---|
| ServBay | Deep navy blue |
| Laragon | Blue |
| XAMPP | Orange |
| MAMP | Grey / blue |
| DDEV | Blue / purple |

The category is saturated with blue. OpenVHost claims the **deep green** territory — motivated by the product itself (green = *running*, the happiest moment in this product's world) and by the license story (**Evergreen** = open in perpetuity). No major competitor owns it.

---

## 2. Name & Wordmark

- The name is written **OpenVHost** — one word, capitals O·V·H. The casing deliberately mirrors Apache's `<VirtualHost>` directive, the object this product tames. Never "Openvhost", "OpenVhost", "Open VHost", "OpenVHOST", or "openVHost".
- **Pronunciation:** "open vee-host" (4 syllables · TH: โอ-เพน-วี-โฮสต์). The V is said as the letter "vee", never blended into "vost".
- **No abbreviation exists.** Never shorten to "OVH" in any context — it collides with OVHcloud, the French hosting company. Write the name in full; the CLI is already short.
- In prose, no article: "OpenVHost manages your services", not "the OpenVHost".
- Wordmark: **Space Grotesk Medium**, tracking −1%, set in Terminal Ink (light backgrounds) or Plaintext (dark backgrounds). The wordmark is text-only; do not add gradients, outlines, or shadows.
- CLI binary is lowercase by convention: `openvhost` (no `ctl` suffix, no aliases). Config file: `openvhost.yaml`. Data dir: `~/.openvhost`. These are product artifacts, not the brand name, and follow code conventions instead.

---

## 3. Logomark

### 3.1 Concept — "The Open Bracket"

An opening square bracket `[` with rounded terminals, holding a Signal Green status dot. It encodes both halves of the name:

- **Open** — the bracket opens and is never closed. (License story: no one can close the source.)
- **Serv** — the green dot is the universal "service running" indicator, this product's core moment.

The mark must always remain an *opening* bracket. Rendering it mirrored, closed, or paired `[ ]` breaks the brand story and is prohibited.

### 3.2 Reference construction (starting point for final design)

```svg
<svg viewBox="0 0 64 64" xmlns="http://www.w3.org/2000/svg" fill="none">
  <!-- open bracket: Evergreen -->
  <path d="M42 10 H26 A16 16 0 0 0 10 26 V38 A16 16 0 0 0 26 54 H42"
        stroke="#0E6E5C" stroke-width="9" stroke-linecap="round"/>
  <!-- status dot: Signal Green -->
  <circle cx="40" cy="32" r="8" fill="#3FB950"/>
</svg>
```

### 3.3 Variants

| Variant | Bracket | Dot | Use |
|---|---|---|---|
| Primary | Evergreen `#0E6E5C` | Signal Green `#3FB950` | Default on light backgrounds |
| Reversed | Evergreen 400 `#33B79E` | Signal Green `#3FB950` | Dark backgrounds |
| Monochrome ink | Terminal Ink | Terminal Ink | Print, engraving, favicons ≤16px (drop the dot below 16px) |
| Monochrome paper | Plaintext | Plaintext | On photos/brand-color fills |
| Template (macOS menu bar) | System-tinted monochrome | Dot **shape** carries state; no color | Tray icon only — see §7.3 |

### 3.4 Clear space & minimum sizes

- Clear space around the mark = the dot's diameter on all sides.
- Minimum: 16 px (mark alone, monochrome, no dot) · 24 px (mark with dot) · 96 px wide (horizontal lockup: mark + wordmark, gap = ½ bracket height).

### 3.5 Don'ts

Do not close or mirror the bracket · do not rotate · do not recolor the dot to non-semantic colors · do not add more dots · do not place the wordmark inside the bracket · do not apply gradients, bevels, or shadows · do not redraw the bracket with sharp corners.

---

## 4. Color System

### 4.1 Core palette

| Token | Name | Hex | Role |
|---|---|---|---|
| `ink` | Terminal Ink | `#171A21` | Dark backgrounds; primary text on light |
| `paper` | Plaintext | `#F7F6F2` | Light backgrounds; text on dark |
| `brand-600` | Evergreen | `#0E6E5C` | Primary accent (light mode): buttons, links, active states, logomark |
| `brand-700` | Evergreen Deep | `#0A5346` | Hover/pressed on light; small text on light where 600 falls short |
| `brand-400` | Evergreen Bright | `#33B79E` | Primary accent (dark mode) |
| `brand-100` | Evergreen Mist | `#DDF1EC` | Selected rows, subtle fills (light mode) |
| `neutral-600` | Slate | `#5C636E` | Secondary text on light |
| `neutral-400` | Slate Light | `#8A9099` | Disabled text, borders |
| `neutral-800` | Panel | `#22262F` | Cards/panels on dark |

**Restraint rule:** Evergreen is the *only* brand accent. No secondary marketing color. Semantic colors (below) appear exclusively when communicating real state — never as decoration. Spend boldness on the mark and typography, keep surfaces quiet.

### 4.2 Semantic state colors (product-critical)

These map 1:1 to the supervisor state machine in the master plan (§3.1) and must never be used for anything except state:

| State | Token | Hex (dark bg) | Text-safe on light |
|---|---|---|---|
| Running | `state-running` | `#3FB950` | `#2B8139` |
| Starting / pending | `state-starting` | `#D99A2B` | `#9A6B14` |
| Failed | `state-failed` | `#E5534B` | `#C13832` |
| Stopped / disabled | `state-stopped` | `#8A9099` | `#5C636E` |

The running text-safe value was deepened from `#2E8B3D` to `#2B8139` (same hue and saturation, lightness 36.3% → 33.7%): the original measured 4.31:1 on white and 3.98:1 on Plaintext, short of the ≥ 4.5:1 floor §4.3 requires below. The replacement clears AA on every light text surface (white 4.88:1, Plaintext 4.51:1).

Signal Green (`#3FB950`) is deliberately brighter and yellower than Evergreen so brand accent and "running" never read as the same thing. If a composition makes them ambiguous, add the state label text — color alone must never be the only carrier of state (accessibility).

### 4.3 Accessibility rules

- All text meets **WCAG 2.1 AA**: ≥ 4.5:1 (normal), ≥ 3:1 (large/bold ≥ 18.66px). Use the "text-safe" column for text on light backgrounds; `brand-600` on Plaintext is reserved for large text/icons — body-size links use `brand-700`.
- State is always color + label (or icon shape), never color alone.
- Both light and dark themes ship at launch and follow the OS (per frontend spec); every token above has a defined value in both modes — no ad-hoc hexes in app code.

---

## 5. Typography

All families are SIL OFL — free to bundle, embed, and redistribute in a GPL project.

| Role | Latin | Thai companion | Fallback stack |
|---|---|---|---|
| Display / wordmark / marketing headlines | **Space Grotesk** (500/700) | IBM Plex Sans Thai (non-looped) | `"Space Grotesk", "IBM Plex Sans Thai", system-ui, sans-serif` |
| UI & body | **IBM Plex Sans** (400/500/600) | **IBM Plex Sans Thai Looped** | `"IBM Plex Sans", "IBM Plex Sans Thai Looped", "Noto Sans Thai", system-ui, sans-serif` |
| Code / logs / configs / CLI output | **JetBrains Mono** (400/700) | falls back to Plex Sans Thai Looped for Thai strings | `"JetBrains Mono", "IBM Plex Mono", ui-monospace, monospace` |

**Rationale:** IBM Plex is a superfamily with a professionally harmonized Thai counterpart — critical because EN + TH ship together (plan Phase 2) and mixed-script UI must not look stitched together. Looped Thai for UI (legibility at small sizes); non-looped for display. JetBrains Mono is the native tongue of this product's world — logs and configs are first-class brand surfaces, not afterthoughts.

**Rules**

- Space Grotesk is used with restraint: wordmark, page-level headings, marketing. Never for body text, tables, or logs.
- UI type scale (px): 12 caption · 13 dense-table · 14 body (default) · 16 section title · 20 page title · 28+ display. Line height 1.5 body, 1.2 headings.
- Logs and any user-config content render in JetBrains Mono at 12.5–13px with ligatures **off** by default (config text must be unambiguous; `!=` must look like `!=`).
- Numerals in metrics/ports use tabular figures (`font-variant-numeric: tabular-nums`).
- Sentence case everywhere — titles, buttons, labels. No Title Case, no ALL CAPS except tiny eyebrow labels (11px, +6% tracking, Slate).

---

## 6. Voice & Tone

### 6.1 Principles

1. **Dev-to-dev, plain verbs.** "Start PHP 8.3" not "Initialize runtime environment".
2. **Errors explain and point forward.** Every failure states what happened, shows the evidence (stderr tail), and offers the next action. Errors never apologize, never blame, never say just "Something went wrong".
3. **Honest about consequences.** Destructive dialogs state exactly what is and isn't affected — the flagship line, verbatim in the product: *"This removes the site from OpenVHost. Your project files in `~/www/myproject` are not touched."*
4. **No hype vocabulary.** Banned: "blazingly", "supercharge", "magical", "seamless", "revolutionary". Allowed superlative: measured numbers ("starts in 0.4s").
5. **The license is a feature — say it plainly.** Marketing may state: "GPL-licensed. No one can ever close this source — including us." Never mock competitors by name.

### 6.2 Microcopy — do / don't

| Context | ✅ Do | ❌ Don't |
|---|---|---|
| Button | `Start service` | `Go!` / `Submit` |
| Running toast | `nginx running on :8080` | `Success! 🎉` |
| Failure | `MySQL failed to start — port 3306 is in use by another process. View log · Change port` | `Oops! Something went wrong.` |
| Empty state | `No sites yet. Add your first site to map a domain to a project folder.` | `It's lonely in here…` |
| Update | `OpenVHost 1.2 available — changelog · Update` | `A shiny new version awaits!` |

### 6.3 Bilingual (EN / TH)

- English is the source language; Thai is a first-class translation written naturally by a human-level pass, never raw machine output shipped as-is.
- Thai register: สุภาพแบบเพื่อนร่วมงาน — ไม่ใช้ครับ/ค่ะ ใน UI strings (เพศกลาง), ใช้คำเทคนิคทับศัพท์ที่ dev ไทยใช้จริง ("start service", "log", "port" คงรูปอังกฤษ), ประโยคสั้นตรงไปตรงมาเหมือนต้นฉบับ.
- Technical nouns, service names, and paths stay in English in both languages.

---

## 7. Applications

### 7.1 App UI tokens

Frontend implements §4 as CSS custom properties (`--vh-brand-600`, `--vh-state-running`, …) — single source of truth in `apps/desktop/src/lib/tokens.css`; no hex literals elsewhere. Radius scale: 6px controls, 10px cards, 999px status pills. Spacing: 4px base grid. Focus ring: 2px `brand-400` outer ring on all interactive elements, visible in both themes.

### 7.2 App icon

- Canvas: Terminal Ink `#171A21` filled squircle (macOS template) / rounded square (Windows ICO with 16–256px sizes).
- Mark: reversed variant (Evergreen Bright bracket + Signal Green dot) at 60% canvas width, optically centered.
- No wordmark inside the app icon at any size.

### 7.3 Tray / menu-bar icon

- macOS: monochrome **template image** of the bracket (the system tints it); aggregate service state is carried by the **shape** of the dot at the bracket's dot position — filled with a mark (any failed), half-filled (any starting), filled (all running), absent (all stopped). Four glyphs, matching the aggregate precedence `failed > starting > running > stopped` defined in `docs/design/README.md`.
- Windows: same geometry in Plaintext on transparent. State may use the colored dot there — Windows tray icons are not template images.
- **Amended 2026-07-31 (owner decision).** This section previously specified a *color-coded* dot on macOS and called it the one place the mark's dot changes color. That is not achievable: `setTemplate(true)` draws the image as a mask, so every color is discarded and only the alpha shape survives. The options were to encode state as shape, or to fund native AppKit compositing (or ship non-template light/dark asset pairs) and lose the menu bar's automatic tinting. Shape-coding was chosen: it ships now and adapts correctly to light, dark and tinted menu bars. Color-only state communication is also the weaker accessibility choice, so this is not purely a concession.

### 7.4 GitHub & community surfaces

- README header: horizontal lockup on Plaintext (light) with dark-mode `<picture>` swap; badges limited to build status, license (`GPL--3.0--or--later` badge in Evergreen `#0E6E5C`), and latest release — no badge walls.
- Social preview (1280×640): Plaintext background, mark left, wordmark + "Open. We host." in Space Grotesk, nothing else.
- README header: descriptor line "Your friendly local host" sits directly under the lockup, set in IBM Plex Sans, Slate.
- Docs site: Plaintext/Ink themes, Evergreen links, JetBrains Mono code blocks with the state palette for diff/status highlighting.

---

## 8. Trademark & Community Use

Full policy lives in `TRADEMARK.md` (plan OQ#8); brand-level summary:

- **The GPL grants the code, not the name.** Forks are welcome and must ship under a different name and mark once distributed publicly. The bracket mark and the name "OpenVHost" identify this project and its official builds only.
- **Allowed without asking:** "works with OpenVHost", "built for OpenVHost", "powered by OpenVHost" in text; unmodified mark when referring to the project (news, tutorials, package lists).
- **Not allowed:** using the mark/name in a way implying official status or endorsement; modified versions of the mark; the name in a fork's product name ("OpenVHost Pro", "OpenVHost Turbo").
- Community meetups/content may use "OpenVHost Community" with the unmodified mark.

---

## 9. Asset Checklist (design backlog)

| Asset | Format | Owner |
|---|---|---|
| Final logomark (refined from §3.2 reference) | SVG master + PNG exports | designer / main thread |
| Horizontal + stacked lockups, light/dark | SVG | designer |
| App icon | `.icns`, `.ico`, PNG set | ci-release-engineer packages |
| Tray template icons + state dots | PDF/SVG (macOS template), ICO (Win) | tauri-frontend-engineer |
| `tokens.css` implementing §4 + §5 | CSS | tauri-frontend-engineer |
| Font bundle + OFL license files in repo | woff2 + licenses | tauri-frontend-engineer |
| GitHub social preview | PNG 1280×640 | main thread |
| `TRADEMARK.md` | Markdown | main thread (human review before publish) |

— End of document —
