<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# `set -e` inside a substitution does not survive being inside another one

**Status:** design, ready to plan.
**Date:** 2026-08-10.

## 1. The fix shipped yesterday does not work

PR #72 changed `build.sh` to `digest="$(set -e; prefix_digest "$prefix")"`, on a proof run at **top
level**. The real call site is nested one deeper, and there the fix is inert. Measured against the
verbatim `json_dependencies` text:

| inner `set -e` | outer `set -e` | run rc | recorded `tree_sha256` |
|---|---|---|---|
| no | no | **0** | digest over an intact stream |
| no | yes | **0** | digest over an intact stream |
| **yes — as committed** | no | **0** | **digest of a stream truncated at the first failure** |
| yes | yes | **1** | aborts, nothing recorded |

`set -e` restores errexit for *that subshell*, so `prefix_digest` does abort early — but
`json_dependencies` runs inside `dependencies="$(json_dependencies)"` with errexit cleared, so the
nonzero status of the `digest=` assignment is **discarded** and the truncated digest is printed
anyway. A well-formed 64-hex value, recorded as provenance, exit 0.

The comment at `build.sh:969-976` states its premise correctly — *"`json_dependencies` is only ever
reached through `dependencies="$(json_dependencies)"`"*, confirmed, exactly one call site — and then
draws the opposite conclusion. **That fact is why the inner `set -e` cannot abort, not why it can.**

## 2. Three mechanism facts, all measured on bash 3.2.57

- **`set -e` does not reach a nested substitution.** `extra="$(set -e; outer)"` where `outer` contains
  `"$(inner)"` runs `inner` with errexit cleared again.
- **A process substitution's failure is unobservable**, even with errexit fully armed:
  `while read … done < <(find …)` cannot fail when `find` fails. **No errexit fix reaches this shape.**
- **`bp_die`'s explicit `exit` unwinds every enclosing substitution subshell**, at any depth,
  independent of errexit. That is why `json_pipeline` is fail-closed and `json_dependencies` is not —
  in the same file, by the same hand, writing to the same manifest.

A method note worth carrying: the triage's first probe wrapped each case as `( … ) || printf`, which
puts the code under test in an AND-OR list and **disables errexit inside it**, reporting the opposite
result. One case per process, no AND-OR list.

## 3. D1 — `set -e;` at the call site is not the policy

It is not merely a discipline someone can forget. **It does not work** where the caller is itself
inside a substitution (§1), and **does not reach nested substitutions** (§2). It is the shape that
produced the false confidence being corrected here.

What composes instead:

- **`bp_die`** where the value is produced by a function — an explicit `exit`, immune to nesting.
- **An explicit `|| { … }` at the call site** where the value comes out of a pipeline, as
  `prefix_digest`'s does, since `exit` inside its `( )` only kills that subshell. A checked status is
  fail-closed regardless of the caller's errexit state, and cannot be silently defeated by a future
  change to how the enclosing function is called.

Rejected: a lint or grep test. The shape to detect is not "missing `set -e`" but "unchecked status of
a value that becomes a record", which greps badly and shellcheck has no check for. For two sites that
fails this project's own cost-versus-yield test.

## 4. D2 — Record the failure, do not abort

`build.sh`'s `prefix_missing` arm already reasons: *"Recorded rather than fatal: by now the artifact
is packed and audited, and a manifest that does not exist is worse than one that says it does not
know."*

That applies verbatim to a digest that failed to compute — same point in the run, same trade. So a
third arm: `tree_sha256: null` plus a `digest_failed` reason, matching the `not_observed` and
`prefix_missing` shapes beside it. Aborting is defensible but contradicts the neighbouring arm and
throws away a completed, audited build over a `stat` hiccup.

**The choice is not between abort and today.** Today records a lie.

## 5. What actually needs fixing — 4 edits, 2 of them load-bearing

The triage inventoried 133 substitution assignments, 51 calling a project function, 20 multi-command.
Two matter.

**A1 — `build.sh:977` + `:1157`.** Check the status where it is produced, falling through to the new
`digest_failed` arm. Keep the inner `set -e;` — it is what makes the status nonzero at all. Rewrite
the `:969-976` comment, which asserts an abort measured not to happen.

Reachable: the failure errexit governs is the *middle* streams (`xargs -0 stat`, the symlink loop) —
the last stream's failure is already caught by inherited `pipefail`. `/opt/openvhost-build` is shared
across four engines and `stage_install` does `bp_rm_tree "$BUILD_PREFIX"` before `mkdir`, so a second
build staging a prefix while this one digests it is a real concurrent mutation.

**A2 — `mariadb.sh:683-684`.** `recipe_manifest_extra` *is* reached through the protected
`extra="$(set -e; recipe_manifest_extra)"`, but does its work inside two **nested** substitutions,
`"$(_mariadb_vendored)"` and `"$(_mariadb_vendored_on_disk)"`, which the `set -e` does not reach.
Hoist both into checked variables, mirroring what `stage_manifest` already does for its three.

Separately, `_mariadb_vendored_on_disk`'s `find "$BUILD_OBJ/extra" … 2>/dev/null` runs in a **process
substitution**, so a missing directory records `[]` — indistinguishable from "nothing was vendored".
That is the observed-empty versus not-observed ambiguity `tree_sha256`'s `null`-plus-reason shape
exists to remove, reintroduced one field over, and it fires on **every `--from pack` run**. No errexit
fix reaches it; it needs its own honest shape.

**Cheap, and both one-liners:**

- `build.sh:760` and `:1119` — `shasum -a 256 <"$TARBALL"` plus the 64-hex assertion. The basename is
  charset-validated, but `OUT_DIR` comes from `--out` with **no validation**, so a backslash in the
  directory yields a 65-character value. Recorded, never compared, so nothing is subverted — but it is
  the last surviving instance of the shape PR #72 fixed in `json_file_digests`.
- `build.sh:299` — `bp_assert_under` fails **open**: with `real_root=""` the pattern
  `"$real_root" | "$real_root"/*` becomes `"" | /*`, matching any absolute path. Unreachable through
  the driver's own flow (the root is asserted mode-700-and-ours first) and recipes are already
  arbitrary code, so this is defence in depth. `[ -n "$real_root" ] || bp_die` closes it.

## 6. What to leave alone, and why touching it would be worse

The triage verified these fail closed already, several for reasons a sweep would be likely to disturb:

- `json_pipeline` — `bp_die` throughout; the working precedent.
- `build.sh:516` signing-key parse — failure yields an **empty** `primary`, and empty means *skip this
  key*; a truncated `--show-keys` can only remove matches, never create one.
- `_mariadb_sql` — already `|| true`, and the output is compared against exact literals with
  `return 1` on mismatch. No false pass is expressible.
- **The write-then-echo family** (`_nginx_write_conf`, `_php_write_pool_conf`, `_php_write_nginx_conf`)
  — the shape is exactly as suspected, a failed `cat >` followed by a trailing `printf` of the path.
  But the consumer is a real server, which fails to start. Cost is a confusing message, not a false
  pass.
- The long tail: single-`printf` path helpers, `count_lines` feeding report text, `_php-pins-refresh.sh`
  (a maintenance tool, off the build path). **No `local x="$(…)"` sites exist anywhere.**

## 7. What this slice must prove

1. **The A1 defect reproduces before the fix and not after** — a truncated-stream digest recorded with
   rc 0, then aborted or recorded as `digest_failed`. Revert-and-watch-it-fail; the harness is four
   lines.
2. **One case per process, no AND-OR list** around anything under test. The triage's first probe got
   the opposite answer that way, and this spec would have been written backwards from it.
3. **A2's nested substitutions are checked**, and `_mariadb_vendored_on_disk` stops reporting `[]` for
   a directory it never looked at.
4. The two one-liners, each proven in both directions.
5. `shellcheck` stays at zero on `build.sh` and every recipe.
6. **Nothing in §6 is touched.**

## 8. Out of scope

A lint or grep gate for the shape (D1) · changing `PROBE_TIMEOUT`-style budgets · anything in §6 ·
the pins-file split still deferred from PR #72.
