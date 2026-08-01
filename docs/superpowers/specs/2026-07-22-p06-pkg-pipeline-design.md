# P0-6 — Download → SHA-256 Verify → Extract Pipeline — Design

- **Date:** 2026-07-22
- **Status:** Approved in brainstorming (4 sections). Three consultations folded in verbatim-by-requirement: **platform-macos-specialist** APPROVE-WITH-CHANGES (empirical, on the dev Mac), **platform-windows-specialist** APPROVE-WITH-CHANGES (knowledge-based; runtime items on the backfill list), **security-auditor** APPROVE-DESIGN-WITH-REQUIRED-CHANGES (23 required findings, all folded into §5). The security-auditor re-audits the real diff at merge time — **this slice is MERGE-BLOCKED without that written APPROVE** (CLAUDE.md golden rule 2).
- **Source of truth:** `docs/OPENVHOST_MASTER_PLAN.md` v1.2 — row **P0-6**: "Download→SHA-256 verify→extract pipeline", owner rust-core-engineer, exit criterion "One real PHP archive installed to `packages/` on both OS". Tech stack fixed by plan §2: `reqwest` + `sha2` + `tar`/`zip` (zstd deferred until real manifests exist).
- **Owner decisions (2026-07-22):** live proof = **php.net source tarball** (official, stable published SHA-256); approach = library + hermetic tests + one env-gated live test (no CLI, no UI, no manifest layer).
- **SCOPE — macOS-first v1 (owner, 2026-07-22):** the per-major `current` link ships for unix/macOS only; the Windows `update_current` half of **S22** is an explicit-error stub in v1, the `junction` dependency and the `x86_64-pc-windows-msvc` cross-check are dropped from this slice, and Windows build+runtime verification (plus P0-5's Windows pool) move to a later **Windows-enablement phase**. The junction design (verify-reparse-point → `fs::remove_dir` → `junction::create`, never `remove_dir_all`) is preserved verbatim in §6.2 for that work. Everything else in §5 — all download, integrity, and **extraction hardening** invariants — is unchanged and platform-agnostic: reserved Windows device names, ADS/`:`, backslash, drive-prefix, and case/normalization collisions are rejected on macOS too, because an archive is untrusted regardless of host OS (security-auditor-required).

## 1. Context

`openvhost-pkg` is a stub. This slice gives it the real pipeline. The future signed-manifest layer is explicitly out of scope: callers hand a pinned `(url, sha256)`; the manifest slice will read the signed index and produce `InstallRequest`s without changing this API. Windows runtime verification is deferred under the standing §2.3 policy (CI billing-blocked): stand-in evidence = full local gates + clean `cargo check/clippy --target x86_64-pc-windows-msvc -p openvhost-pkg`; the Windows backfill checklist is §6.3.

## 2. Goals

1. `install_package()` downloads over HTTPS, verifies SHA-256 **before any archive parsing**, extracts with hardened manual entry walks, and atomically installs to `packages/<name>/<major>/<version>/` with a per-major `current` link.
2. Hostile input cannot write outside the staging root, cannot smuggle entry types, and cannot collide paths on case-insensitive filesystems — the extractor is treated as trusted-computing-base for a future malicious-manifest scenario.
3. Hermetic adversarial test suite runs in every `cargo test --workspace`; one env-gated live test installs a real php.net tarball (the exit-criterion evidence).

## 3. Non-goals

Manifest index/signing · uninstall/upgrade/disable · zstd · resume/Range/fallback URLs · CLI/UI · at-rest integrity after install (first execution slice re-audits; see §6.1 quarantine/ad-hoc-sign note) · junction self-heal (needs `state.db` — deferred with the Windows specialist's blueprint recorded in §6.2) · crash-durability fsync hardening beyond §5-S24 (RECOMMENDED tier).

## 4. API & module layout

```rust
// crates/openvhost-pkg/src — one responsibility per module:
// error.rs      PkgError (thiserror)
// request.rs    InstallRequest, ArchiveFormat, Progress, InstalledPackage, validation
// download.rs   streaming fetch + hash + caps + redirect policy
// extract.rs    tar.gz + zip hardened manual walks
// layout.rs     staging, atomic install, sweep, current-link orchestration
// platform/     unix.rs (symlink swap) · windows.rs (junction)
```

```rust
pub struct PackagesRoot(PathBuf);      // minted ONLY from openvhost-core home
impl PackagesRoot { pub fn from_home(home: &Path) -> Self /* home.join("packages") */ }

pub enum ArchiveFormat { TarGz, Zip }

pub struct InstallRequest { name, major, version, url: url::Url, sha256: String, format: ArchiveFormat }
impl InstallRequest { pub fn new(...) -> Result<Self, PkgError> }   // ALL validation here

pub enum Progress { Started { total: Option<u64> }, Downloaded { bytes: u64 }, Verified, Extracted, Linked }

pub struct InstalledPackage { pub dir: PathBuf, pub current_link: PathBuf, pub name: String, pub major: String, pub version: String }

pub async fn install_package(
    req: &InstallRequest,
    root: &PackagesRoot,
    progress: impl FnMut(Progress) + Send,
) -> Result<InstalledPackage, PkgError>
```

**Boundary validation in `InstallRequest::new` (F20 — normative here, enforced at the merge audit like the S-items):** `name`/`major`/`version` each match `[a-z0-9._-]{1,64}`, are not `.`/`..`, do not start with `.` or `-` (kills `.staging` collision and flag-alikes), are not Windows reserved device basenames, no trailing dot/space. `sha256` exactly 64 **lowercase** hex (uppercase rejected, not folded). URL passes the shared validator (S3). `PackagesRoot` is a newtype minted from core's `resolve_home()` only — never from IPC/webview input (a future Tauri command physically cannot pass an arbitrary path).

**Dest exists → `Err(AlreadyInstalled)`** at pre-check, AND rename-time `EEXIST`/`ENOTEMPTY` maps to the same variant (pre-check alone is racy; a concurrent identical install completing first is success-shaped, surfaced as `AlreadyInstalled`).

**New deps** (all must clear `cargo deny`): `reqwest` **`default-features = false, features = ["native-tls", "stream"]`** (no gzip/brotli/zstd/deflate/cookies/http3), `sha2`, `tar`, `flate2` (default miniz_oxide backend — no zlib-ng/C feature), `zip`, `tempfile`, `hex`, `url`, `fd-lock`, `unicode-normalization`, and on Windows the `junction` crate (SPDX verified via `cargo deny` at vendoring — Windows-specialist action item). `tracing` for audit logs (S27).

## 5. Pipeline & security invariants (the merge-audit checklist)

Numbered S-items; (F*) = security-auditor finding id, (M*) macOS, (W*) Windows.

**Transport**
- S1 (F1): ONE shared URL validator applied at request build AND per redirect hop (custom `redirect::Policy`, max 5): `scheme == "https"`, non-empty host, **reject non-empty userinfo**. Same function both places.
- S2 (F5): hermetic-test bypass = plain `http` accepted only for loopback hosts (`127.0.0.0/8`, `[::1]`) and only under `cfg(debug_assertions)` — compiled OUT of release. Deviation note for the merge audit: `cfg(test)` cannot cover integration tests (they build the lib without it) and a non-default feature would silently skip hermetic tests in `cargo test --workspace`; debug-only-loopback is the tightest gate that keeps the suite always-running. Never an env var or runtime flag.
- S3 (F2): transport auto-decompression DISABLED (feature set above) + `Accept-Encoding: identity` — the SHA-256 is over exact wire bytes; a transparent Content-Encoding layer would be a second, uncapped decompressor ahead of the hasher.
- S4 (F3): running byte counter aborts > **1 GiB** regardless of Content-Length (chunked/lying servers); if Content-Length present: early-abort when it exceeds the cap, error if final count differs.
- S5 (F4): timeouts — connect ≈30s, idle-read ≈60s (`read_timeout`), overall wall cap ≈15min.
- S6 (F9): no resume, no Range, no fallback URLs. Transport-error retries (if any) start a fresh staging file + fresh hasher. Hash mismatch NEVER retries — delete staging, fail with expected/actual.
- S7 (F7 RECOMMENDED, adopted): system proxy honored (CONNECT keeps TLS end-to-end; the pin protects integrity) — documented; `min_tls_version(TLS_1_2)`; no accept-invalid-certs knob anywhere. (F6 RECOMMENDED, adopted: reject IP-literal hosts in the shared validator — loopback-http debug carve-out aside.)

**Integrity**
- S8 (F8): verify and extract on the SAME open file handle — stream to the staging file while hashing, `sync_all`, compare, rewind, hand the handle to the extractor. Never re-open by path. Windows: staging file opened denying `FILE_SHARE_WRITE|FILE_SHARE_DELETE`. (Unix same-user fd aliasing accepted per threat model §8.)
- S9 (F10): hash-of-compressed-archive is sufficient; no content hashing. tar/flate2/zip parsing is TCB — S10..S19 are the mitigation.

**Extraction (manual entry walk — never any unpack-all API; both formats)**
- S10 (F11): entry-type whitelist, reject-the-ARCHIVE on violation: Regular, Directory, Symlink (tar only), Hardlink (tar only). Explicitly rejected: char/block devices, FIFOs, sockets, GNU/PAX sparse. PAX/GNU longname metadata is resolved by tar-rs — all checks run on post-resolution `path()`/`link_name()`.
- S11 (F12): entry-name rules, identical on all OSes: valid UTF-8 (zip via `name_raw()` + our validation, never lossy names), no NUL, no empty/`.`/`..` components, no leading `/`, no drive/UNC prefix, no `:` anywhere (subsumes ADS (W-D)), reject Windows reserved basenames `con|prn|aux|nul|com0-9|lpt0-9` (with or without extension) and trailing dot/space per component (W-D), depth cap 32, relative-path cap 240 bytes (deterministic reject instead of mid-extract MAX_PATH/`ENAMETOOLONG` surprise; macOS ENAMETOOLONG still surfaces as a typed error, never a panic (M4)).
- S12 (F13, M2, W-E): collision rejection — set keyed on NFC-normalized + case-folded full relative path; ANY duplicate rejects the archive (covers zip duplicate-name last-wins, tar dupes, APFS/NTFS case-insensitivity, NFC/NFD tricks — APFS silent-overwrite empirically proven). Sole exception: an explicit directory entry may coincide with an auto-created parent (applies its clamped mode once). Cross-platform rule, not cfg'd — both platform specialists signed off.
- S13 (F14): fail-closed creation — files via `create_new` (O_EXCL); never unlink-and-replace; parents must be real directories.
- S14 (F15): symlink discipline (tar only): defer ALL symlink creation to a final phase after regular entries; reject any entry path traversing a symlink ancestor; targets must be relative, UTF-8, with NO `.`/`..` components (sibling/descendant only), lexically inside root, and must not point to/through another deferred symlink (no chains in v0). This closes the lexical bypass (`d -> .` then `L -> d/../x`). Zip: symlink external-attr entries skipped entirely (not honored, not materialized).

  > **AMENDED 2026-08-01 — the ban is replaced; the reasoning above was right and still holds.**
  >
  > The blanket "NO `..`" rule made the pipeline reject every relocatable macOS payload. Upstream's MySQL tarball carries 34 symlinks, **22 of them containing `..`**, and they are not decoration: they are what lets a Mach-O binary find its dylibs through `@loader_path`. Dropping them yields a clean `Ok` and a `mysqld` that SIGABRTs in dyld. Measured, not argued — `crates/openvhost-pkg/tests/live_net.rs`.
  >
  > **What replaced it** (security-auditor ruling, 2026-08-01): `..` may appear **only as a leading contiguous run**, of length `k`, where the link's own directory has `d` components and `k <= d`; the resolved path must be non-empty and must satisfy the entry-name size caps. The auditor first **rejected** a naive lexical-containment predicate and demonstrated the escape on disk, so the accepted rule rejects the *primitive* (`..` after a named component) rather than trying to reason about compositions.
  >
  > **The bypass this clause was written to close is still closed.** `..` is never applied to a path whose last component was a symlink, so chains cannot launder an escape — and the adversarial test §7 mandates for it, `rejects_symlink_chain_escape`, still passes unchanged under the new rule. The "no chains in v0" clause is therefore no longer the mechanism, but the property it bought is preserved.
  >
  > **Zip**: no longer "skipped entirely". A silent skip produces the same failure shape as the bug above — clean `Ok`, dead binary — so zip now **rejects** a symlink entry with a named reason. Our zip payloads are Windows PHP builds, where symlink creation is refused anyway.
  >
  > **S18's "all S11–S14 checks run on POST-strip paths" was already correct and the implementation had drifted from it.** The symlink check was running in pass 1, before the strip, which is an escape in its own right: `root/bin/x -> ../../etc/passwd` passes a pre-strip depth bound and lands above the destination once the root is stripped. Fixed by deleting the depth-free entry point so the miswired call cannot be written again.
- S15 (F16): hardlinks — target must be an already-extracted regular file inside root; materialized as a copy (also the Windows story: hardlinks need no admin (W-H0), so inside-only is enforced, not assumed).
- S16 (F17): full mode clamp — file 0o755 if any exec bit else 0o644; dirs 0o755; staging 0o700; kills setuid/setgid/sticky/group-world-write. Windows ignores archive modes. No xattrs/ACLs/PAX attrs ever applied from archives.
- S17 (F18): caps on REAL bytes written via bounded streaming copy (fixed buffer, never `read_to_end`): total 4 GiB decompressed, per-entry counted from output (zip64/local headers lie). Entry cap 100k counts every entry incl. rejected/metadata. Zip: iterate central directory only; reject any encrypted entry.
- S18 (F19): single-top-dir strip: only when exactly one top-level component exists AND it is a directory entry (not file/symlink); all S11–S14 checks run on POST-strip paths. (php.net Windows zips are flat → strip is a no-op there, correct (W-G).)
- S19 (M3): after extraction, recursively strip `com.apple.quarantine` and non-essential `com.apple.*` xattrs from the tree before the final rename (quarantine empirically rides through archives); the chosen crates must not restore PAX/SCHILY xattrs (tar-rs default `unpack` doesn't — we hand-walk anyway).

**Install & layout**
- S20 (F21): staging = `tempfile::Builder::tempdir_in(<root>/.staging)` (random name + RAII cleanup) holding an exclusive `fd-lock` on `<dir>/.lock` for the install's lifetime; the sweeper deletes only entries whose lock it CAN acquire AND whose mtime > 24h (a slept laptop mid-download is a real concurrent-old install).
- S21: `fs::rename(staging_root, final_dir)` — same volume by construction; `EXDEV` fails clean (no silent copy — empirically confirmed (M-D)); dest-must-not-exist is STRUCTURAL on Windows (dir rename cannot replace — (W-F)); EEXIST/ENOTEMPTY → `AlreadyInstalled` (§4).
- S22 (F22, M1, W-A/B): `current` link update:
  - unix: verify any existing `current` IS a symlink (error on real file/dir), create temp-named symlink with **bare relative sibling target** (`"8.4.8"`, not `../8.4.8`, not absolute — survives home relocation/Time Machine; swap via `rename` empirically atomic on APFS), rename over `current`.
  - Windows: junction via the `junction` crate (absolute target — junctions cannot be relative); replace = verify existing path has `FILE_ATTRIBUTE_REPARSE_POINT` (tag `IO_REPARSE_TAG_MOUNT_POINT`) → `fs::remove_dir` (removes ONLY the reparse entry) → create. **Module review rule: `remove_dir_all`/recursive delete against `current` is forbidden** — the blast radius is the real package payload. Non-atomic window documented; missing-on-first-install = skip the remove.
- S23: atomicity doc note distinguishes namespace-visibility atomicity (rename: guaranteed) from crash durability (no fsync guarantee; not a v0 blocker — worst case reinstall) (W-F).
- S24 (F24 RECOMMENDED, adopted-lite): disk-free preflight (archive size × 3 heuristic) + one bounded retry with short backoff on Windows `ERROR_SHARING_VIOLATION`/`ACCESS_DENIED` at the final rename (Defender scan-lock (W-H1)); fsync/completion-witness deferred.
- S25 (F25 RECOMMENDED, adopted): in-process install semaphore (1 permit) — unbounded parallel installs from a future UI are a local DoS.

**Dependencies & audit trail**
- S26 (F26): manual-walk rationale is normative, in code comments: tar-rs `unpack` had RUSTSEC-2021-0080 (link-target traversal); zip pre-0.6 mishandled `../`/symlinks and duplicate-name last-wins remains; `enclosed_name()` used only as a secondary assert. flate2 stays on miniz_oxide. zip crate changelog reviewed on every lockfile bump (2024 maintainer transition).
- S27 (F27): `tracing` audit logs — url, final post-redirect url, byte count, dest, expected/actual sha256 on mismatch, every rejected-entry reason. Typed thiserror errors; no unwrap outside tests; SPDX headers.

## 6. Platform notes

### 6.1 macOS (consult empirical, 2026-07-22)
- Real `php-8.4.8.tar.gz`: 25,402 entries, 0 symlinks/hardlinks, non-ASCII (Japanese) filenames present — symlink policy is exercised ONLY by synthetic fixtures; collision keys must be true normalization/case equivalence, not "has non-ASCII".
- Downloads by our own process do NOT acquire `com.apple.quarantine` (only `com.apple.provenance`); quarantine CAN arrive via archive xattrs → S19.
- Future execution slice (recorded now, out of scope): strip quarantine + **ad-hoc-sign unsigned Mach-O before first exec** — Apple Silicon kills unsigned binaries (SIGKILL, AMFI) and quarantine blocks even direct `./binary` exec on both architectures.
- Do not relocate `~/.openvhost` into iCloud Drive/cloud-synced folders (doc note).

### 6.2 Windows (consult knowledge-based; runtime deferred)
- Junctions never require elevation; `std::os::windows::fs::symlink_dir` correctly rejected (needs privilege/Dev Mode).
- Junction targets are baked absolute → the durable source of truth is the logical (name, major → version) record; **self-heal on relocation needs `state.db` and is deferred to that slice** with this design recorded: on start, recompute expected target, verify, delete+recreate on mismatch.
- Rust std transparently applies `\\?\` for long paths on absolute, dot-free paths (which S11 guarantees); `longPathAware` manifest is defense-in-depth only, never load-bearing.
- MOTW/`Zone.Identifier` is NOT set by a bare Rust writer — no SmartScreen surprises; Defender on-access scanning is the real interference → S24 retry.

### 6.3 Windows-matrix backfill checklist (deferred with the CI policy)
1. Junction create/delete/repair round-trip (incl. simulated relocation). 2. Deep-path (>260) extraction/rename through the pinned crates, no LongPathsEnabled. 3. Tauri v2 Windows manifest `longPathAware` default. 4. Live php.net zip layout across NTS/TS × x64 variants + MariaDB/MySQL zip (strip-rule true-positive). 5. `junction` crate SPDX via `cargo deny` (at vendoring, not deferred). 6. First-install Defender scan-lock behavior.

## 7. Testing

**Hermetic (every `cargo test --workspace`):** local HTTP server on `std::net::TcpListener` (no new dep) + fixtures generated in-test. Adversarial set (S-mapped, from F28): bad hash · truncated stream · oversize via chunked (no Content-Length) · https→http downgrade redirect · userinfo URL · IP-literal URL · device-node & FIFO tar · sparse tar · duplicate-name zip · case-collision pair · NFC/NFD pair · zip-slip `../` · absolute path · symlink-outside · the S14 chain escape (`d -> .` + `L -> d/../x`) · symlink-ancestor traversal · hardlink-outside · setuid mode → clamped · encrypted zip · zip64 lying sizes · entry named `con` · trailing-dot name · depth/path-length overflow · single-root strip true-positive AND false-positive (flat zip) · sweep-vs-locked-staging · `AlreadyInstalled` both at pre-check and rename race. Happy paths: tar.gz and zip install end-to-end with progress-event order asserted and `current` link verified (unix).

**Live proof (exit criterion, run once, evidence in PR):** `OPENVHOST_NET_TESTS=1 cargo test -p openvhost-pkg --test live_net -- --nocapture` installs the real php.net source tarball (URL + published SHA-256 pinned in the test) into a temp `PackagesRoot`, asserting layout + `current`. Not part of the default suite.

**Gates:** full local suite (fmt, clippy `-D warnings`, `cargo test --workspace`, `cargo deny check licenses advisories` — new deps!, SPDX, pnpm suite untouched-but-green) + `cargo check/clippy --target x86_64-pc-windows-msvc -p openvhost-pkg` + **security-auditor final audit of the real diff → written APPROVE (merge gate)**.

## 8. Threat-model assumptions (accepted, from the security consult)

Local same-user tampering OUT of scope (relied on for: unix fd aliasing residual, staging under user home). Other local users IN scope → S16 mode clamps. A future compromised manifest IS in scope → extractor hardened as TCB (S10–S18). Compromised mirror/MITM reduces to DoS given pin+TLS. No at-rest integrity after install — first execution slice re-audits.

## 9. Delivery

Branch `feat/p06-pkg-pipeline` → SDD per-task (rust-core-engineer implements; platform link code per ownership map) → final whole-branch review → **security-auditor diff audit APPROVE** → PR with live-proof evidence → local gates → merge. Conventional Commits + DCO; SPDX on all new files.
