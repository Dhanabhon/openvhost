<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The five seconds go to XProtect, not to the machine being busy

**Status:** design, ready to plan.
**Date:** 2026-08-09.

## 1. What was actually measured

`PROBE_TIMEOUT = Duration::from_secs(5)` (`crates/openvhost-conf/src/inspect.rs:78`) bounds
`run_bounded` and surfaces as `ConfError::ValidatorTimeout`. Two tests in different crates have failed
against it at least five times in two days, and both pass in isolation on unchanged code.

**Every microsecond of the budget is spent between `posix_spawn` returning and `/bin/sh` executing the
script's first line.** Instrumented replica, 16 concurrent probes:

| phase | fresh fixture | same file pre-executed |
|---|---|---|
| `spawn()` | 0.61 ms | 0.15 ms |
| **spawn → child's first command** | **p50 2.67 s, p95 4.59 s** | ~0 |
| the child's own work | 0.19 ms | ~0 |
| child exit → future resolved | p50 0.60 ms | ~0 |
| **timeouts** | **307/320** | **0/320** |

With the cap raised to 60 s to uncensor the distribution: p50 **6.20 s**, max 6.75 s.

## 2. The cause is XProtect, and it serializes

Daemon CPU over 400 execs: warm (one file, 400 times) costs `syspolicyd` 0.08 CPU-seconds; fresh (400
new files) costs **`XprotectService` 38.57**.

Cache semantics, measured:

| variant | cost |
|---|---|
| fresh file, content seen before | 125 ms |
| **fresh file, unique content** | **396 ms** |
| copy of a warm file, new inode, identical bytes | 124 ms — keyed by **inode** |
| warm file renamed to a new path | 5.4 ms — **not** keyed by path |
| a fresh file's **second** exec | 5.6 ms |
| `/bin/sh <freshfile>` (never exec'd directly) | 3.4 ms — the cost is on **exec**, not on read |

Dose-response against concurrent first-execs: 1 → 0.40 s, 4 → 1.54 s, 8 → 3.12 s, 12 → 4.67 s,
16 → 6.20 s. **~+390 ms per concurrent first-exec** — one serializing XPC service. The budget is
crossed at roughly **13**.

## 3. D1 — CPU load is not the axis, and assuming it was would have produced the wrong fix

At load average **135** with ~1571 % of 1600 % CPU consumed, both failing tests passed **9/9 in
0.14 s**. Sequential fresh-exec cost under that load was 383 ms against 397 ms idle — unchanged. The
observed failures happened at load average **3–5**.

Reactor starvation is also ruled out: child-exit → future-resolution is p50 585 µs at maximum
contention, and the tokio-task heartbeat gap equals the raw-OS-thread gap (16.8 vs 16.2 ms), so tokio
is no more starved than the kernel.

**The real trigger is ordinary build churn.** With one `cargo test --workspace --no-run` compiling in
another target dir, the conf test took 4.28 / 4.83 / 3.39 / 2.59 / 1.01 / 0.14 s across six runs — two
within 200 ms of the cliff, then recovering as the build finished. A second concurrent worktree
session tips it over, which is exactly how this project has been running its gates.

## 4. D2 — Fix the fixtures, not the constant

`PROBE_TIMEOUT` is right for the product **by 300–1800×**. Under the same exec pressure that makes the
fixture take 6 s:

- real `nginx -v`, 16-way concurrent: p50 2.7 ms, p95 15 ms, **max 17 ms**
- the real `check_settings` render-and-validate path against brew nginx: **10–20 ms**

Raising the constant would also degrade the contract it exists for — a wedged binary becomes an
N-second UI spinner — and it cannot be raised correctly anyway, because the fixture's requirement grows
~390 ms per concurrent first-exec, an unbounded target. Any new number is a cliff someone finds later.

## 5. D3 — Pay the first-exec cost outside the bounded window

After `set_permissions`, run the fixture once and discard the result. The XProtect evaluation is then
already done when the timed call runs.

**Validated under maximal contention**: identical fresh-per-iteration fixture, 16 concurrent runtimes,
5 s cap → **0/320 timeouts, total p50 6.4 ms, max 10.4 ms**, against 307/320 and 5.00 s without it.

Measured alternatives that also work — a hardlink (5.1 ms) or symlink (3.3 ms) from one process-wide
warm template — are rejected as needing shared state for no gain.

## 6. D4 — Virtual time is not the answer, and would be worse than the bug

`openvhost-conf` already carries `tokio = { features = ["test-util"] }` precisely so `inspect.rs`'s
timeout test can advance the clock instead of burning 5 s. **It does not transfer.**

Measured: a `current_thread` runtime with `start_paused(true)`, running a probe whose child must
genuinely run and exit, timed out in **187–408 µs, 3/3**. Tokio auto-advances the virtual clock to the
next timer deadline the moment the runtime parks on the child's I/O — so `start_paused` converts a
load-sensitive red into a **deterministic** red.

The existing test works only because it *wants* the timeout and deliberately blocks the runtime thread
so the clock cannot auto-advance; its own comment says so. The desktop crate has no tokio
dev-dependency at all, and adding one would be pointless for the same reason.

## 7. D5 — The blast radius is ten files, not two

The two tests that have bitten are the ones that got unlucky. Put the warm-up in the fixture helpers
(`fake_cli`, `fake_bin`, and `inspect.rs`'s equivalent) rather than in the two call sites, and sweep
every file whose tests write a fresh executable and run it through a `PROBE_TIMEOUT`-bounded call:

```
crates/openvhost-conf/src/{webserver.rs, inspect.rs, mysql.rs, settings/check.rs}
crates/openvhost-core/src/{mariadb/init.rs, site/apply/commit.rs}
crates/openvhost-core/tests/macos_stack.rs
apps/desktop/src-tauri/src/{mysql_admin.rs, commands.rs, clitool/shell.rs}
```

## 8. D6 — One production caveat, recorded rather than fixed

The single product path that touches this cost is golden rule 6's runtime-download model: the **first**
probe of a freshly extracted binary. Measured — nginx (6.1 MB) at a new inode 463 ms, mariadbd
(28.4 MB) 586 ms; twelve such first-execs concurrently, slowest **3.23 s**.

It is safe today because production probes are **sequential** (`block_on` per binary in `stack.rs` and
`commands.rs`), so only the ~0.5 s figure applies. It becomes reachable only if discovery probes are
ever parallelised across a freshly extracted package. Record it next to `PROBE_TIMEOUT` so whoever
parallelises them meets it there rather than discovering it in the field.

## 9. What this slice must prove

1. **The failure reproduces on demand before the fix** — the exact reported errors, under exec
   pressure rather than CPU pressure — and **stops** after it. This is the whole slice; a fix that
   cannot be shown to fix anything is a guess.
2. The warm-up runs **outside** every `PROBE_TIMEOUT`-bounded call, in every one of the ten files.
3. **No production code changes.** If any is proposed, `inspect.rs:220-233`'s drop ordering is
   load-bearing: the `.await` must stay in the `match` scrutinee so the group leader is alive and
   unreaped when `-pgid` fires. The comment names the exact refactor that breaks it.
4. The suite is no slower in aggregate — one extra exec per fixture, against a 5 s cliff avoided.
5. D6's caveat is recorded at `PROBE_TIMEOUT`.

## 10. Out of scope

Changing `PROBE_TIMEOUT` (D2) · making the timeout injectable · parallelising discovery probes (D6) ·
any attempt to disable or work around XProtect, which is neither ours to configure nor something a
user's machine will have done.
