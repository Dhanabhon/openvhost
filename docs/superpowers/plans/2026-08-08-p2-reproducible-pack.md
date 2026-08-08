<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan — four bytes stand between this pipeline and a reproducible artifact

Spec: `docs/superpowers/specs/2026-08-08-p2-reproducible-pack-design.md`. Read it first; it is the
contract and this file does not restate it.

Branch `feat/p2-reproducible-pack`, worktree `.claude/worktrees/repro-build`, based on `fefc13f`.

One task. The change itself is one line; **the repack-and-re-pin is the work**, and the proof is
the point.

## The change

`build/build.sh:694` is

```bash
COPYFILE_DISABLE=1 tar -czf "$TARBALL" -C "$BUILD_ROOT" "$BUILD_NAME-$BUILD_VERSION"
```

Add `--options gzip:'!timestamp'`. Spec D2 explains why the pipe-to-`gzip -n` alternative was
rejected even though `set -euo pipefail` (line 31) makes it safe — do not switch to it.

## The staged prefixes you have

**`/opt/openvhost-build` is READ-ONLY.** It holds the only build output of four engines and there
is no second copy. Repack *from* it; never write into it.

```
/opt/openvhost-build/mariadb-11.4.9/
/opt/openvhost-build/nginx-1.30.4/
/opt/openvhost-build/php-8.4.24/
```

`./build/build.sh <name> <version> --from pack --out <dir>` is the resume form. Note that `--from`
is recorded in the manifest by design — *"an artifact built from partly stale state must not look
like a clean one"* — so say in your report what the manifests now record.

Existing tarballs for comparison (do not modify):
`.claude/worktrees/p1-site-scaffold/build/out/mariadb-11.4.9-macos-arm64.tar.gz` and
`.claude/worktrees/nginx-build/build/out/nginx-1.30.4-macos-arm64.tar.gz`.

## Prove, and report each by name

- **§5.1 — packing twice gives identical bytes**, on a real staged prefix. Do this for all three,
  not one.
- **§5.2 — the raw tar is unchanged by the fix.** `gunzip -c` an old tarball and a new one for the
  same engine and compare. If those differ, this is a content change wearing a reproducibility
  label and you should stop and report. (Measured before this slice: two pre-fix packs of PHP had
  raw tar `df0dfb79c99ad02b6b0abfccdb74167f6ad8e89e08d28239f384a5405c3f63ae` on both sides.)
- **§5.3 — all three re-pinned** in their catalogues to the new hashes.
- **§5.4 — each new pin reproduces.** Pack again *after* re-pinning and get the recorded hash back.
  This is the assertion the whole slice exists for; make it explicit rather than implied by §5.1.
- **§5.5 — `build/audit.sh --execute-artifact` passes 7/7** on each repacked tarball.
- **§5.6 — nothing user-visible changes**; every `availability` stays `AwaitingRelease`.
- **The `include_str!` tripwire.** A catalogue is tied to its recipe file by a compile-time check
  that has fired before. Say whether re-pinning trips it and what you did.
- Vacuity: with the fix reverted, §5.1 must fail. Show it.

## Binding

- Report **against each obligation by name**, including any that come out negative.
- **No sub-agents.** Report conclusions, not transcripts.
- **`/opt/openvhost-build` is strictly read-only.** Verify it is intact when you finish and say so.
- **Never pipe a gate through `tail`** — the exit code you read becomes `tail`'s. This repo hit
  that four times in one week.
- Stage by explicit path; never `git add -A`. Build output goes to `build/out`, which `.gitignore`
  excludes — do not commit a tarball.
- **No browser automation.** Do not kill a process you did not start.
- **Set `OPENVHOST_HOME` to a scratch dir** when running the suite; one existing test provisions
  the real home when it is unset. Never touch the user's real `~/.openvhost`.
- Conventional Commits, `git commit -s`, message via `git commit -F <file>` — a bare `-n` reads as
  `--no-verify` here.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop check`.
  Run from a clean cargo fingerprint or an isolated target dir — a shared `CARGO_TARGET_DIR`
  silently links a stale crate and **can make a gate falsely green**.
- If the task needs a design decision the spec does not make, **stop and report** rather than
  choosing. On each of the last six slices an implementer refused a spec item and was right —
  twice where the item was mine.
