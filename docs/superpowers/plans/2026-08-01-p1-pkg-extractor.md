# Slice 1 — Make the extractor accept a real relocatable macOS payload

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `openvhost-pkg` can install the real upstream MySQL 8.4 macOS arm64 tarball and the result actually runs. Today it cannot — three separate blockers, all proven live by Slice 0.

**Why this is slice 1 and not "MySQL from tarball":** sequencing the extractor fixes behind a UI slice means discovering the symlink rejection with a half-built feature attached. This is a self-contained `openvhost-pkg` change with synthetic fixtures, and nothing about it needs MySQL wired up. The MySQL-from-tarball spec (`2026-08-01-p1-pkg-mysql-tarball-design.md`) becomes slice 2 unchanged.

## The evidence this plan is built on

Slice 0 ran the real 167,977,240-byte `mysql-8.4.11-macos15-arm64.tar.gz` through the real `install_package`. Numbers below are measured, not estimated.

**Everything numeric passed with room to spare** — 375 entries against `MAX_ENTRIES` 100 000; longest relative path 80 bytes against 240; depth 6 against 32; 0.62 GiB uncompressed against the 4 GiB cap; download 7.2 s against the 900 s timeout. **The reserved-device-name rule never fired** — the premise was wrong, this tarball ships no `mysql-test/`. **Do not touch any of those limits or that rule.** They were part of the P0-6 security APPROVE and Slice 0 found no reason to disturb them.

---

## Task 1: the two uncontroversial fixes

**Files:** `crates/openvhost-pkg/src/extract/validate.rs` (+ tests).

### 1a — Duplicate directory headers must not be a collision

Upstream emits `mysql-8.4.11-macos15-arm64/bin/` **five separate times** (raw tar lines 1, 24, 26, 92, 279), and `lib/` four times. The case-folded duplicate check treats that as `path collision` and rejects the archive.

A repeated **directory** header naming the same path is idempotent and benign — tar producers do it routinely. Accept it. **Everything else stays rejected**: a file colliding with a file, a file colliding with a directory, or a case-fold collision between different names. That distinction is the whole fix; do not widen it.

### 1b — `strip_single_root` must never succeed into a wrong tree

The archive has one top-level component (`mysql-8.4.11-macos15-arm64/`) but **no explicit directory entry for it**. Today the strip is skipped, the install returns `Ok`, and every file lands one level too deep — so discovery finds no `bin/mysqld`. Slice 0's control run isolates the cause to that single missing header: adding it, and changing nothing else, produces a correct tree.

This is the only **silent** failure in the set, and it is the same shape as the boolean-collapse pattern already in the ledger — a two-state answer where three states exist. Fix it so that either the strip happens on a genuinely shared top-level prefix regardless of whether the header is present, **or** the archive is rejected with a message naming the reason. Pick one and say why.

**Binding acceptance criterion:** the test asserts on `installed.dir.join("bin/mysqld")` existing, **never** on `Result` being `Ok`. Slice 0 demonstrated both variants returning `Ok`; only the tree tells them apart.

- [ ] **Step 1:** Tests RED first, using Slice 0's four variant shapes as fixtures (they are already in `crates/openvhost-pkg/tests/live_net.rs`, offline-constructible): duplicate dir headers accepted; file/file, file/dir and case-fold collisions still rejected; a single shared root with no explicit header produces a tree with the payload at the package root.
- [ ] **Step 2:** Implement; `cargo test --workspace`; fmt/clippy. Commit: `fix(pkg): accept repeated directory headers and strip an implicit single root`

## Task 2: the symlink containment rule — **auditor decides before this is written**

**Do not start this task until the security-auditor has ruled.** The rule being changed was part of the P0-6 APPROVE.

**The problem, measured.** `validate_symlink_target` rejects any target containing a `..` component. The MySQL tarball has 34 symlinks; **22 contain `..`**. Slice 0 checked every one: **zero escape the archive root** when normalized against the link's own directory.

**And they are load-bearing.** Dropping them yields a clean `Ok` and a `mysqld` that dies at exec:

```
dyld: Library not loaded: @loader_path/libprotobuf-lite.24.4.0.dylib
      tried: '.../bin/libprotobuf-lite.24.4.0.dylib' (no such file)
```

`bin/libprotobuf-lite.24.4.0.dylib` **is** one of the 22. The rejected construct is exactly the mechanism that makes a macOS payload relocatable. This is not a nuisance limit — it is a correctness bug against every relocatable macOS tarball we will ever install.

**Two options, both costed by Slice 0:**

| | Lexical containment | Materialize as copies |
|---|---|---|
| Change | normalize the target against the link's own directory; require the result to stay under the extraction root | keep the rule; replace each symlink with a copy of its target |
| Accepts the 22 | yes | yes |
| Rejects escapes | yes (measured) | yes (rule unchanged) |
| Disk | none | **+31.4 MB** for the 22, +52.4 MB for all 34 |
| Complication | a security-relevant relaxation of an APPROVED rule | needs **symlink-chain resolution** — several targets point at another symlink, not a regular file (`lib/plugin/libfido2.1.dylib -> ../../lib/libfido2.1.dylib`) |

Slice 0 leans containment on the evidence and explicitly did not decide. **Neither do I** — this is the auditor's call, and the brief for them is the table above plus Slice 0's raw output.

- [ ] **Step 1:** Security-auditor ruling on which option ships. Blocking.
- [ ] **Step 2:** Tests RED first — every one of the 22 real target shapes accepted; a target that genuinely escapes (`../../../etc/passwd`, an absolute target, a chain that leaves the root) still rejected; **a fixture proving an escape attempt cannot be laundered through a chain of in-root symlinks.**
- [ ] **Step 3:** Implement per the ruling; `cargo test --workspace`; fmt/clippy. Commit message depends on the option chosen.

## Task 3: pre-warm the binary, and stop lying about slow connections

**Files:** `crates/openvhost-pkg/src/install.rs`, `download.rs`.

### 3a — One warm-up exec in staging

Slice 0's decisive measurement: signature validation **survives `rename(2)`**.

```
warm-up exec IN STAGING:          749.241542ms
1st exec AFTER the atomic rename:  13.551333ms
```

So a single throwaway `--version` in staging, before the atomic rename and behind the existing progress bar, converts the user's first "Start MySQL" from a ~750 ms mystery pause into a warm start. Roughly ten lines.

Two constraints: the binary to warm is **package-specific**, so it belongs in the request rather than hardcoded; and a warm-up failure must **not** fail the install — it is an optimization, and a package whose `--version` misbehaves is still installed. Log it and continue.

Note for the record, since it corrects an earlier assumption: the Gatekeeper cost is **not** the 11.53 s the Homebrew binary paid. It is ~1.9 s once per machine (a network notarization lookup) plus ~750 ms per fresh inode. That figure does not follow us into this model.

### 3b — The 900 s timeout is a bandwidth floor wearing a network error's clothes

`TOTAL_TIMEOUT` is a fixed wall clock on the **whole request**, not an idle timeout. 167,977,240 bytes / 900 s = a hard floor of **~1.5 Mbit/s sustained**. A user below that cannot install MySQL, and what they will see is something indistinguishable from "the network failed".

Either make it an idle timeout (no progress for N seconds) or keep the wall clock and **make the error say what actually happened** — that the download did not finish in time, with the observed rate. A cap that silently redefines itself as a minimum supported connection speed is the kind of thing that generates unreproducible bug reports.

- [ ] **Step 1:** Tests RED first — a warm-up failure does not fail the install; the slow-download error names the rate rather than reading as a generic network fault.
- [ ] **Step 2:** Implement; `cargo test --workspace`; fmt/clippy. Commit: `feat(pkg): warm a freshly installed binary and report slow downloads honestly`

---

## Global constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By`**.
- No `unwrap`/`expect` outside tests. No `unsafe`. No wildcard arms over the extractor's error enums.
- **Do not touch** `MAX_ENTRIES`, `MAX_REL_BYTES`, `MAX_DEPTH`, `MAX_TOTAL_BYTES`, `SIZE_CAP`, or the reserved-device-name rule. Slice 0 measured all of them passing and the rule never firing.
- TDD with vacuity proof per group; **state the method**. For every "still rejected" assertion, prove it can fail — a containment rule that accepts everything passes an acceptance test and fails only in production.
- **Benchmark in release, never debug.** Slice 0 measured the extractor at 15.4 s in debug and **2.5 s in release** on the same 669 MB payload. A debug number here would send someone optimizing a non-problem.
- Gates per task: `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`. The `live_net.rs` tests stay behind `OPENVHOST_NET_TESTS=1` and must not run in a normal workspace test.

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates.
- [ ] **Step 2:** Whole-branch review **and security-auditor** — the auditor is already involved via Task 2; they must also confirm the duplicate-directory relaxation cannot be used to smuggle a file past a collision check.
- [ ] **Step 3:** **Live proof.** Install the real MySQL 8.4 tarball end to end through the real pipeline; assert `bin/mysqld` is at the package root, `otool -L` shows only system paths and `@loader_path`, `codesign -v` still passes after extraction, and `mysqld --validate-config` against our own rendered `my.cnf` exits 0. Then time the first exec after the rename and show it is warm. Hermetic root under `/tmp`; **do not touch the owner's Homebrew or `~/.openvhost`.**
- [ ] **Step 4:** One fix wave; re-run gates. PR; squash-merge on green.

## Recorded, not fixed here

- **MariaDB has no macOS build at all** — not arm64, not x86_64, none, across 10.4→11.8, confirmed via both the archive listing and the REST API. It is a **build target**, not a download target. Remove it from the download roadmap; re-scope as "build from source, or drop".
- **Oracle publishes MD5 + detached PGP for the macOS artifact, no SHA-256.** Our pipeline is SHA-256-pinned, so someone must compute that hash at manifest-build time — and the only real integrity anchor upstream is the `.asc`. **Verifying Oracle's signature once, out-of-band, at manifest build is what makes our pinned hash mean anything.** That belongs in the manifest slice as an explicit step, not an assumption. Slice 0 could not verify it (no `gpg` on the machine).
- **The `macos{N}` tag is version-coupled** — `macos15` for 8.4.10 and 8.4.11; `macos14` 404s. A manifest cannot template it from the MySQL version; pin per release.
- **The installed tree is 639 MB from a 160 MB download** (4×). Relevant to the hosting and disk-budget conversation, not to this slice.
- **`bin/mysqld_safe` carries a hardcoded `/usr/local/mysql/data`** and would need an explicit `--datadir` if we ever use it. We do not today.
