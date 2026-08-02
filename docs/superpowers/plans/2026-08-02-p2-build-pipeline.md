# OpenVHost's own package build pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a checked-in pipeline that turns MariaDB 11.4 source into a **relocatable, signed, audited** macOS tarball, and an end-to-end proof that `openvhost-pkg` can install that tarball into `~/.openvhost/packages/mariadb/11.4/11.4.9/` and run it from there.

**Architecture:** see the spec (`docs/superpowers/specs/2026-08-02-p2-build-pipeline-design.md`) — **READ IT FIRST; D1–D8 and §13–§14 are binding.** Every decision in it was measured on 2026-08-02, and the owner closed all four open questions the same day: single-builder trust accepted, security obligation accepted, **11.4 LTS only**, **static OpenSSL**.

**Programme:** slice 3 of 7. 0 payload proof ✅ · 1 extractor ✅ (#43) · 2 MySQL from tarball ✅ (#44) · **3 MariaDB — the build pipeline** · 4 nginx · 5 PHP · 6 remote manifests · 7 retire brew.

**Scope boundary, stated so it is not discovered halfway:** this slice ends when our own tarball installs through `install_package` and the installed `bin/mariadbd` runs from the package tree. The MariaDB **service** — datadir init, start/stop, credentials, the Databases-page row — is the next slice. Do not build it here. If a task starts needing it, that is a finding to report, not scope to absorb.

## Global Constraints

- SPDX headers on new files; `git commit -s`; Conventional Commits; **no `Co-Authored-By`**.
- Most of this slice is shell, not Rust. Where a Rust rule has no shell analogue, say so rather than pasting it. Where it does, it binds: `set -euo pipefail`, quote every expansion, no `rm -rf` on an unvalidated variable, and every script is runnable standalone with no ambient state.
- **Never write outside `build/`, the neutral build prefix, and an explicit output directory.** Nothing in this slice may touch `~/.openvhost`, a datadir, or the user's Homebrew.
- **Publishing is owner-gated.** The pipeline produces artifacts locally. Creating a GitHub Release that hosts binaries is an outward-facing act — prepare it, show the owner what would be published, and **stop**. Do not publish.
- **This worktree may be shared.** Run mutation experiments in a disposable detached worktree. Stage by explicit path; never `git add -A`. Never leave a mutation on disk when you stop.
- Gates per task: `cargo test --workspace` → `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings` when Rust is touched; `shellcheck` on every script this slice adds. **Gate the commit on the result** — `<gate> && git commit`, not two statements on one line.
- Build artifacts are large (3.4 GB build tree). Nothing under `build/` output paths may be committed; add ignores in Task 1, not later.

## Task 1: the driver and the artifact contract

**Files:** `build/build.sh`, `build/audit.sh`, `build/recipes/` (skeleton), `.gitignore`.

Per D1, D2, D6, D7.

The frame comes first **and so does the gate**, so Task 2 has a standard to satisfy rather than a standard invented to fit what it produced.

`build/build.sh <name> <version>` drives the fixed sequence and calls into `build/recipes/<name>.sh` for the package-specific parts only:

```
fetch → verify signature → extract → configure → build → install
      → normalize → audit → sign → pack → manifest
```

`build/audit.sh <tree>` implements §8's six points and **exits non-zero on any failure**:

1. layout: a single root with `bin/` and `share/`;
2. **linkage**: every `otool -L` entry of every Mach-O is `/usr/lib/*`, `/System/*`, or `@loader_path/...`;
3. **signature**: every Mach-O is signed and `codesign -v` passes;
4. **no builder identity**: the tree contains none of `$HOME`, the current username, or any session/scratchpad path;
5. **runs from two paths**: install to A, run, move to B, run again;
6. **serves and survives**: start, create a table, insert, restart, read back.

Points 5 and 6 need a server, so they are skipped with an explicit `SKIPPED (no server binary)` line for packages that have none — **never silently**. nginx and PHP will exercise that branch.

**The vacuity proof is built in and is not optional.** Today's proof tree at `/private/tmp/.../mariadb-proof/third-location-*` is **known-bad on two counts**: it embeds 13 builder paths (fails 4) and contains Homebrew's dylibs (fails 2 before the `@loader_path` rewrite). Point the audit at a copy of it and **it must reject it, naming both**. An audit that passes that tree is broken, and this is the cheapest possible way to find that out.

- [ ] **Step 1:** Write `audit.sh` first and prove it fails: run it against a known-bad tree and capture which checks fired. Then write `build.sh` and the recipe interface.
- [ ] **Step 2:** `shellcheck` clean; ignores added. Commit: `feat(build): add the package build driver and its artifact contract`

## Task 2: OpenSSL static, then MariaDB, producing a tarball that passes

**Files:** `build/recipes/openssl.sh`, `build/recipes/mariadb.sh`.

Per D2, D3, D5, D8, §13.

**Start from the proven configuration, do not re-derive it.** The reference run lives in the spec §2 and the four failures in §2 are the four things that will happen again if the recipe drops a flag:

- resolve `cmake`/`make`/`bison` to absolute paths **before** scrubbing `PATH`;
- `bison ≥ 3.0` is a build-host dependency (macOS ships 2.3) — and **ServBay's `bison` on `PATH` cannot run at all**, so pin by absolute path, never by name;
- `-DCMAKE_IGNORE_PREFIX_PATH` for `/opt/homebrew`, `/usr/local`, `/Applications/ServBay`;
- every `WITH_*` pinned — `WITH_PCRE=bundled`, `WITH_LIBFMT=bundled`, `WITH_ZLIB=bundled`, `PLUGIN_AUTH_GSSAPI=NO`;
- `INSTALL_LAYOUT=STANDALONE`, `CMAKE_INSTALL_RPATH=@loader_path/../lib`, `CMAKE_BUILD_WITH_INSTALL_RPATH=ON`;
- pass **one concrete OpenSSL prefix** — ours. Never `bundled`: the server and the bundled Connector/C read the same `WITH_SSL` name with different value sets, and anything unrecognised lands in the connector's GnuTLS branch.

**OpenSSL is built first and statically** (owner decision). Success means `otool -L bin/mariadbd` shows **nothing** but `/usr/lib` and `/System` — no `lib/` bundling and no `install_name_tool` step at all. If static proves impractical, the `@loader_path` fallback in D3 is already proven — but **falling back is a reported finding**, not a quiet substitution.

Build under the **neutral prefix** `/tmp/openvhost-build/<name>-<version>` (D8), because 13 files embed it and post-processing them all is fragile. Contract check 4 is what enforces that the owner's real paths never appear.

Output: `mariadb-11.4.9-macos-arm64.tar.gz`, its `.sha256`, and the build manifest (§7: upstream URL + verified sha256, MariaDB's signing-key fingerprint and expiry, every configure flag, toolchain versions, neutral prefix, output sha256).

**Upstream provenance is verified, not assumed.** MariaDB publishes a GPG-signed `sha256sums.txt`. Verify the signature *and* cross-check the key fingerprint against a second host, exactly as the MySQL slice did for Oracle's key. Record key id, expiry and verification date beside the pin.

- [ ] **Step 1:** `openssl.sh` — build static, assert no `.dylib` is produced.
- [ ] **Step 2:** `mariadb.sh` — build, then **`audit.sh` must pass on the result**, including points 5 and 6 running for real. Commit: `feat(build): build MariaDB 11.4 as a relocatable signed tarball`

## Task 3: the artifact installs through the pipeline we already have

**Files:** `crates/openvhost-core/src/mariadb/` (catalogue only), wiring to `openvhost-pkg`.

Per D5.

The catalogue entry is **compiled in and shaped exactly like MySQL's** (`crates/openvhost-core/src/mysql/package/catalogue.rs` — `{ version, url, sha256 }`). Consistency beats novelty; a second shape is a finding, not a design.

Add the two fields §14 requires: the upstream release date and the date we last checked. A stale check must be visible in source rather than remembered.

**Then prove the loop closes:** `install_package` fetches our tarball from a local `file://`-or-fixture source, verifies the sha256, extracts into `packages/mariadb/11.4/11.4.9/`, and the installed `bin/mariadbd --version` runs. That is the whole point of the slice — an artifact nobody can install is worth nothing.

Nothing here touches the Databases page, service supervision, or a datadir. See the scope boundary.

- [ ] **Step 1:** Tests RED first — the catalogue's shape and that an unsupported arch is refused (Apple Silicon only; there is no signature-checked x86_64 pin and this slice does not add one); the recorded-version round trip; a checksum mismatch leaves no partial tree.
- [ ] **Step 2:** Implement. `cargo test --workspace`; fmt/clippy. Commit: `feat(core): install our own MariaDB build through openvhost-pkg`

---

## Phase C — gate, PR, merge

- [ ] **Step 1:** Full gates: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, plus `shellcheck` on every script.
- [ ] **Step 2:** Whole-branch review **and security-auditor**. The audit is not optional here: this slice downloads over the network, verifies signatures, executes a compiler on fetched source, and produces binaries we will ask users to run. New surface for the auditor: a build script that fetches and executes third-party source, and an artifact contract that is the only thing standing between a leaked host dependency and a user's machine.
- [ ] **Step 3:** **Live proof.** Run the real pipeline end to end and paste real output: source signature verified → built → audited → signed → packed → installed through `install_package` into a hermetic `OPENVHOST_HOME` under `/tmp` (**not** `$TMPDIR` — the 103-byte `sun_path` ceiling) → `bin/mariadbd` runs from the package tree. Then confirm the owner's Homebrew is byte-identical before and after.
- [ ] **Step 4:** One fix wave; re-run gates. PR. **Do not publish a GitHub Release** — show the owner the artifact and manifest and let them decide.

## Recorded before it bites

- **The audit is the only real defence.** Configuration expresses intent; today's proof showed leakage arriving from two package managers at once *and* a third route (embedded text) no linker flag addresses. If a task is tempted to relax a contract check to make a build pass, that is the moment the pipeline stops being worth having.
- **Static OpenSSL may hit symbol or version conflicts** with MariaDB's bundled Connector/C. The fallback is proven; taking it is a finding.
- **Ad-hoc signing must be last.** Every `install_name_tool` edit invalidates the signature, and Apple Silicon refuses to execute unsigned code — so an out-of-order recipe produces a package that cannot start, on any Mac.
- **`~3 min compile, 3.4 GB build tree, 465 MB installed`** was measured with heavy plugins disabled. If the recipe re-enables any, that number moves and CI planning moves with it.
- **This slice creates the security obligation** in spec §14 for real. The watch-list entry and the last-checked date land in Task 3; they are not paperwork, they are the only trigger the obligation has.
