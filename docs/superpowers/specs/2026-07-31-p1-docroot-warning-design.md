# P1 Docroot Warning — Design (correction to the scaffold slice)

- **Date:** 2026-07-31
- **Status:** Approved by the owner in-session after hitting the problem on their own machine.
- **Trigger:** A real first-use failure, not a hypothetical. The owner created a site `dhanabhon-web`, picked `~/Downloads` as the Project folder, left the "Create a site folder inside this folder" checkbox at its default (off), and saved. Result: the site's docroot is literally `/Users/tom/Downloads`, the generated nginx config already carried `root "/Users/tom/Downloads"`, and starting nginx would have served **1,071 items** at `dhanabhon.localhost` — with any `.php` dropped there later executed as code. The user's report was "no folder was created"; the folder was the smaller half of the problem.

## What this corrects

The scaffold slice (`docs/superpowers/specs/2026-07-29-p1-site-scaffold-design.md`) chose **default-unchecked** for the create-folder checkbox, reasoning that checked-by-default would silently turn the common "browse to my existing `~/Projects/my-app/public`" flow into `…/public/my-app`.

That reasoning still holds. What it under-weighted is the mirror case: picking a **container** folder — `~/Downloads`, `~/Desktop`, `~/Documents`, or the home directory itself — and having the entire thing become a web root. That outcome is worse than the one the default was protecting against, and the UI gave no signal at all: with the checkbox off, even the live final-path preview does not render, so nothing on screen told the user what the docroot would be.

**The default does not change.** Silently re-pointing a folder the user explicitly chose is the behaviour this project has repeatedly rejected. What changes is that the dangerous case stops being silent.

## D1 — A live warning on the Project folder field, in BOTH create and edit mode

The existing create-folder checkbox renders only in create mode. The warning must render in **both** modes: an edit that re-points an existing site at `~/Downloads` is exactly as dangerous, and the current site was created before this guard existed, so the user's first encounter with the fix will be through Edit.

Warning tiers, by the chosen path (not by its contents — see D2):

- **Home directory itself** (`/Users/<user>`): the strongest case. Everything the user owns.
- **Well-known personal folders** directly under home: `Downloads`, `Desktop`, `Documents`, `Movies`, `Music`, `Pictures`, `Public`, `Library`.
- **System / shared roots**: `/`, `/Users`, `/Applications`, `/System`, `/Library`, `/Volumes`, `/tmp`, `/private`, `/etc`, `/usr`, `/var`.

Copy must state the consequence in the user's terms, not the mechanism: that **every file in that folder becomes reachable at the site's domain**, and that **any `.php` file there will be executed**. It must also offer the fix that is one click away — tick "Create a site folder inside this folder" and the docroot becomes `<picked>/<site name>` instead (create mode), or point at a subfolder (edit mode).

## D2 — Path-based only; no filesystem inspection, no new IPC

The warning is computed from the path string alone. Counting entries would make the copy more vivid ("1,071 items") but requires either a new Tauri command that stats a caller-supplied path — a primitive the security-auditor would rightly scrutinise, and one this project has spent two slices avoiding — or a scan on every keystroke. Neither is worth it: the path alone identifies the dangerous cases with no false negatives that matter, and "your Downloads folder" is already concrete enough to stop someone.

A "this folder has a lot of files in it" heuristic is deliberately NOT shipped. It would fire on legitimate large projects, and a warning that cries wolf is a warning users learn to click past.

## D3 — Warn, never block

The user may have a reason; it is their machine and their loopback. Blocking a save on a heuristic is the kind of paternalism that gets worked around. The warning is prominent and permanent (it does not dismiss), and the save proceeds.

**Rejected alternative: auto-ticking the checkbox when a risky folder is picked.** It would have prevented this exact incident with zero user action, but it silently changes a control the user can see — the same class of surprise the default-off decision exists to avoid. A visible warning plus a one-click fix respects the user's agency and teaches the mechanism; a self-flipping checkbox does neither.

## Scope

**In:** a pure derive helper classifying a path into `None | PersonalFolder | HomeItself | SystemRoot` (or equivalent), its unit tests, and rendering in the Project folder field in both drawer modes, with contrast checked in both themes.

**Out:** entry counting; any IPC change; blocking saves; retroactive warnings on existing sites listed in the Sites panel (worth a follow-up — the site that triggered this is still mis-pointed until the owner edits it); Windows path shapes (macOS-first).

## Verification owed to a human

1. Add site → pick `~/Downloads` → the warning appears immediately, naming the folder and the consequence.
2. Tick the create-folder checkbox → the preview shows `~/Downloads/<name>` and the warning clears (the docroot is no longer the container).
3. Pick a normal project folder → no warning.
4. Edit the existing `dhanabhon-web` (docroot `~/Downloads`) → the warning appears in edit mode too.
5. Both themes: the warning is legible and meets contrast.
