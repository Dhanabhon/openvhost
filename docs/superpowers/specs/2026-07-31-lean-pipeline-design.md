# Lean Delivery Pipeline — Design

- **Date:** 2026-07-31
- **Status:** Approved by the owner in-session, after a data-grounded analysis of why slices were taking so long.
- **Trigger:** The owner asked why the work is slow. The complaint, once narrowed, was **wall-clock from starting a feature to merging it** — not cost, not silence, not check-in frequency. They explicitly accepted more risk in exchange for speed.

## The measurement that decided this

Merge timestamps on `main`:

| Slice | Merged | Wall clock since the previous merge |
|---|---|---|
| #34 site scaffold | 07-29 03:56 | ~3 h |
| #35 index narrowing | 07-29 12:25 | ~8.5 h |
| #36 MySQL lifecycle | 07-30 21:27 | **~33 h** |
| #37 log viewer | 07-31 08:43 | ~11 h |
| #38 docroot warning | 07-31 11:13 | ~2.5 h |

Counting the log-viewer slice's subagent dispatches: **≈28**, of which **≈12 (43%) were rework** — fix waves and re-reviews, not new work. Individual dispatches ran 3–68 minutes. Since the SDD rule forbids parallel implementers (file conflicts), those 28 were almost entirely **serial**: ~28 × ~15 min ≈ 7–8 h of pure agent time, which matches the observed 11 h.

**The bottleneck is the number of serial round trips, not the speed of any one of them.**

## Cost vs. yield, from this session's evidence

| Gate | Cost per slice | What it actually caught |
|---|---|---|
| **Live proof** (real binaries) | 1–2 dispatches | PHP fatals reaching no log at all; mysqld unable to restart on a dot-prefixed datadir; a missing `!includedir` target being fatal; the world-writable `mysqlx` socket during the empty-password window. **Nothing else caught any of these.** |
| **Whole-branch review** | 1 (large) | Ring sources being a dead end from both Services entry points; `$uri` logging the post-rewrite path |
| **Security audit** | 1 | The `mysqlx` regression a later fix wave introduced; `<home>` not actually 0700 |
| **Per-task review, every task** | **12–18** | Case-insensitive path matching; poll reentrancy — mixed in with a large volume of test-quality nits |
| **Dual blind design** | 2 + synthesis | Changed the decision three times (filter placement, staged init, icon-state count) |

Per-task review is **60–70% of the cost** and produced the *least* severe findings. The cheapest gate produced the worst bugs.

## The new pipeline

Per slice:

1. **Design once.** The orchestrator writes the spec directly. **Dual blind design is reserved** for decisions that are both expensive to reverse *and* without precedent in this codebase — schema, credentials, app lifecycle. (MySQL qualified; the docroot warning did not.) The spec itself stays: it is cheap, and every later step reads from it.
2. **Build in ≤3 tasks**, sized to a coherent chunk rather than a minimal one. **No review between tasks.**
3. **Gate once, at the end:** whole-branch review + live proof + security audit (the audit whenever the slice touches the Tauri command surface, credentials, file paths, or child processes).
4. **One fix wave**, then merge.

≈ **8 dispatches** instead of 28.

## What replaces per-task review

Front-load what the reviewers kept finding. Every implementer brief carries these as binding requirements, and the implementer must report against them:

- **Vacuity proof per test group** — RED first, or neuter-and-watch-it-fail (already standard; keep it).
- **Filesystem and locale semantics** — case-insensitive volumes, separator collapsing, symlinks. The `Downloads`/`downloads` miss is the archetype.
- **Reentrancy and lifecycle** — overlapping polls, listeners surviving teardown, intervals outliving their route.
- **Exhaustiveness** — no wildcard arms on project enums; prove a new variant fails to compile.
- **Seams between tasks** — the class per-task review structurally could not see, and which the whole-branch review caught twice.

This is cheaper than reviewing after the fact, but it is **not equivalent**: a self-review is the author grading their own work.

## What we accept losing

Expect **1–2 real defects per slice to reach `main`**. The whole-branch review will catch some — in this session it caught the largest ones anyway — and the rest will surface in the owner's own use, exactly as the `~/Downloads` docroot did. That is the trade the owner chose, with the evidence in front of them.

Two things are explicitly **not** traded away, because they are cheap and they carry the highest yield:

- the **live proof** against real binaries, and
- the **security audit** on any slice touching the audited surfaces.

## How we will know it worked

Record, in the slice ledger, **dispatch count and wall clock** for every slice from here on. The target is 28 → ~8 dispatches and 11 h → 3–4 h. If the next three slices do not move, the diagnosis was wrong and this design gets revisited rather than defended.
