<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sites row: an overflow menu for the secondary actions — design

**Status:** design, approved by the owner from a side-by-side mockup, ready to build.
**Date:** 2026-08-05.
**Follows:** PRs #45–#48, which fixed the row's *width* with container queries. This fixes
its *density*, which is a different complaint and was not solved by those.

## 1. Goal and the boundary

The Sites row carries five controls. Three of them move behind a `⋮` button:

| stays visible | moves into `⋮` |
|---|---|
| Enable/Disable toggle · Open in browser | View logs · Edit · Delete |

**Out:** any change to what the actions *do*, the Databases or Languages rows (they have
their own density story and are not crowded), and the delete confirmation's own behaviour.

## 2. D1 — Split by frequency, not by tidiness

The failure mode of an overflow menu is burying the action people actually use. On this page
that is the **enable toggle** — a state control, pressed far more often than Edit — and
**Open**, which is the reason the app exists. Both stay one click.

Edit becomes two clicks. That is the cost, stated plainly, and it is the right trade only
because Edit opens a drawer for considered work rather than a quick toggle. Delete becomes
three interactions, which for a destructive action is a feature.

## 3. D2 — Portal to `<body>`, and why `position: fixed` is not enough

The panel sets `overflow: hidden` to clip rows against its rounded corners, so a menu
positioned inside a row is clipped.

`position: fixed` looks like the cheap escape and **probably is not one here**: PR #45 added
`container-type: inline-size` to `.rowlist`, and containment makes an element a containing
block for fixed-position descendants — which would leave the menu inside the clipped subtree
after all.

**Verify that claim before relying on either half of it.** It is written here as the reason
for the decision, not as an established fact, and a wrong "why" in a comment is worse than
no comment. Either way the decision stands: **portal the menu to `<body>`** and position it
from the trigger's `getBoundingClientRect()`. That is correct whether or not containment
bites, it is immune to any future ancestor gaining `transform` or `filter`, and — unlike the
Popover API, which would also escape via the top layer — it is testable in jsdom.

Close the menu on scroll and on resize rather than trying to keep a portalled element glued
to a moving trigger.

## 4. D3 — The menu reveals; the existing inline confirm still commits

Delete already has a two-step inline confirm in the row (`confirming = true`). It does not
move and it is not replaced by a dialog. Choosing Delete in the menu closes the menu and
puts the row into that same confirm state.

Two deliberate surfaces for a destructive action, and **no new confirmation code** — the
safest change is the one that adds no second way to delete a site.

## 5. D4 — A DOM test project, because this feature is the trigger the config named

`vite.config.ts:54` already says it: *"A test that needs a live DOM — user events, focus,
measurement — would fail loudly here; that is the right time to add a browser/jsdom
project."* A `⋮` menu is all three at once, and every risk it carries is an interaction risk.

So this slice adds a second vitest project on `jsdom` before it adds the menu. Be precise
about what that buys, because it is not everything:

| covered by jsdom | not covered |
|---|---|
| focus returns to the trigger on close | menu position on screen |
| Escape closes; click-outside closes | whether it is visually clipped |
| `aria-expanded` / `aria-haspopup` correctness | anything about paint |
| arrow-key movement, tab order | |

Position and clipping are inherently visual and go on the human click-list. Saying so here
stops a green suite from being read as "the menu works."

The existing `server` project keeps every test it has. A component rendered through
`svelte/server` is still the right tool for markup assertions, and nothing already passing
moves.

## 6. What this slice must prove

1. Every one of the row's five actions is still reachable, and still does the same thing.
2. The menu is operable by keyboard alone: open, move, choose, Escape, and focus lands back
   on the trigger.
3. Delete from the menu reaches the **existing** confirm — no second delete path exists.
4. The menu is not clipped by the panel (human, with a screenshot).
5. Every existing Sites test passes **unmodified**. If one needs editing, the row's
   behaviour changed and that is a finding.

## 7. Out of scope

The Databases and Languages rows · replacing the delete confirm with a dialog · the Popover
API (it would work in the app and cannot be tested here) · CSS anchor positioning · any
change to the container queries from PR #45–#48, which stay as the narrow-width answer.
