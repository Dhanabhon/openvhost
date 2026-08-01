# Slice 1 — MySQL from the upstream tarball, off Homebrew

- **Date:** 2026-08-01
- **Status:** Every `[S0]` blank is now filled with a measured value — Slice 0 proved the payload, Slice 1 (PR #43) made the extractor accept it, and the provenance chain was closed on 2026-08-01. **Ready to build.**
- **Owner decision, 2026-08-01:** move OpenVHost off Homebrew entirely and adopt ServBay's model — our own package tree, binaries fetched at runtime and verified. Raised cost: we become the trusted publisher, with build, signing and hosting obligations. Owner chose it with that in front of them.
- **Programme:** slice 1 of 7. Slice 0 = prove the payload · **1 = MySQL from tarball** · 2 = uninstall + `current` + installed list · 3 = MariaDB · 4 = nginx (first own build) · 5 = PHP · 6 = remote manifests + signing.

## Why MySQL first, and why not MariaDB

MariaDB was my first proposal and it was wrong. It rests on the least certain fact in the programme — whether MariaDB publishes an **arm64** macOS tarball at all (historically only `osx10.x-x86_64`) — and it is new-service work stacked on a new package source, so a failure would be unattributable. MySQL publishes `mysql-<version>-macos*-arm64.tar.gz` and, crucially, **everything downstream of the binary path is already proven live** by the MySQL lifecycle slice. Any failure here is unambiguously the new package source.

## What ships

`brew install mysql@8.4` is replaced by: fetch the pinned upstream tarball → verify SHA-256 → extract into our own package tree → record the exact version. The Databases page behaves as it does today. **The user's existing brew-installed MySQL keeps working** (D3).

## Decisions

### D1 — The package tree already exists; use it as built

`openvhost-pkg` (Phase 0, PR #5, security-auditor APPROVE) already implements the whole tree and is wired to nothing. Confirmed from the code, not invented — `PackagesRoot::from_home(home)` gives `package_dir(name, major, version)`, `major_dir`, `current_link` and `staging_root`, with `update_current` doing an atomic symlink swap that **refuses to replace a real directory**.

This slice is the first consumer. It adds no new install machinery; if it needs any, that is a finding worth reporting rather than a change to make quietly.

### D2 — A compiled-in catalogue, not the manifest repo

Pinned `(version, url, sha256, format)`, mirroring how `MYSQL_CATALOGUE: [&str; 1] = ["8.4"]` already works. The public `openvhost/manifests` repo is slice 6, deliberately last: its schema should describe four packages that work rather than predict them.

**The catalogue entry, and its provenance — verified 2026-08-01, not assumed:**

```
version  8.4.11
url      https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-macos15-arm64.tar.gz
sha256   b96e00493bc3499b9ffd7f08d65c5d64933af0383a8287d9873b64f94c2d6009
size     167,977,240 bytes  (installs to 639 MB)
```

Oracle publishes **MD5 and a detached PGP signature, no SHA-256 sidecar**, so our pin is computed by us — which is exactly why the signature had to be checked before it could mean anything. It was:

- key `BCA43417C3B485DD128EC6D4B7B3B788A8D3785C` (MySQL Release Engineering), created 2023-10-23, **valid to 2027-10-23**;
- its fingerprint **cross-checked against `dev.mysql.com/doc`** — a different host from the `repo.mysql.com` the key was fetched from;
- `gpg --verify` → **Good signature** on the artifact;
- the signed bytes hash to exactly the SHA-256 above.

So the pin certifies *"the bytes Oracle published"*, not *"the bytes someone downloaded"*. **Re-run that check whenever the catalogue entry changes** — a pin nobody traced back to a signature is the failure golden rule 6 exists to prevent.

Trap for a future entry: the OS tag is **version-coupled** (`macos15` for 8.4.10 and 8.4.11; `macos14` 404s). A manifest cannot template it from the MySQL version — pin it per release.

### D3 — Discovery reads our tree *and* Homebrew, in that order

**The owner is running a brew-installed `mysql@8.4 8.4.11` right now.** Stranding them would be a self-inflicted wound in the first slice of the migration.

So discovery gains a `packages/`-tree walk and keeps the existing Homebrew walk as a fallback. Ours wins where both exist — we know its version exactly, brew's we would have to probe. One `if`; a cheap hedge that buys the whole migration room to be incremental. Brew discovery is retired in slice 7, not here.

**The UI must say where a runtime came from.** A user debugging "which mysqld am I actually running" should not have to guess, and during a migration that question gets asked.

### D4 — The version is recorded at install and never probed again

This is the ServBay property and the structural cure for the defect the uninstall slice's live proof found: after a real `brew install mysql@8.4`, the app reported nothing installed, because the **first** execution of the freshly extracted 55 MB `mysqld` takes **11.53 s** (Gatekeeper first-run scan; second exec 0.039 s) against a 5 s `PROBE_TIMEOUT`.

The research established that this cost **follows us into the new model** — the binary that took 11.53 s was not quarantined, so the trigger is simply "first exec of a new binary", which our pipeline reproduces exactly. A larger timeout only moves the cliff.

We installed it, so we know what it is: `state.db` records the exact version at install time. Probing survives **only** for discovering Homebrew runtimes we did not install.

**Measured, and already solved by Slice 1.** The cost is real but far smaller than the Homebrew figure that started this: **809 ms cold, 16 ms warm** — not 11.53 s. And the signature validation **survives `rename(2)`**, so `install_package`'s staged warm-up (`with_warmup_binary`) pre-pays it: the `Extracted → Linked` window absorbs ~810 ms and the user's first Start is warm. Pass `bin/mysqld` as the warm-up binary — **never `bin/mysqld_safe`**, which carries a hardcoded `/usr/local/mysql/data` and genuinely tries to start a server.

### D5 — Spawn a resolved concrete path, never `current`

`current` is for humans and for future upgrade flows. The supervisor must resolve it to a concrete `packages/mysql/8.4/<version>/` path **at spawn time** and record that path with the process.

Spawning *through* the symlink means a `current` swap silently changes which engine a restart brings up — the running process and the one the UI describes would diverge with nothing in between to notice. It also makes `mysqld`'s argv[0]-derived basedir ambiguous, which is exactly the class of thing that cost this project a full misdiagnosis in the MySQL slice.

### D6 — The datadir is untouched, and it is shared across install sources

The datadir stays at `<home>/data/mysql/<major>/`, keyed by **major**, not by where the binaries came from. A package install must never write there.

That is deliberate: a user who has an initialized 8.4 datadir from the brew era and then gets the tarball-installed 8.4 **keeps their databases**. Same major, same MySQL, same on-disk format.

**Confirmed safe: the upstream tarball is 8.4.11, the same minor Homebrew installs today.** A datadir initialized under the brew build opens under this one. The rule still stands for every future entry — MySQL will not open a datadir initialized by a *newer* server, so **the catalogue must never move a user's server backwards**.

### D3b — One row per major, and what that costs (decided 2026-08-01)

Discovery **merges per major**: with both a packaged and a Homebrew 8.4 present, the list holds **one** 8.4 runtime — ours — and the Homebrew one is not in it. That is D3's "ours wins" made concrete, and it is the shape the UI's one-row-per-major model needs.

The live proof surfaced a consequence I had not thought through when writing D3, and it is accepted deliberately rather than by accident: **once a packaged 8.4 lands, the user can no longer uninstall their Homebrew 8.4 from inside the app.** The row is badged packaged, and a packaged runtime offers no Uninstall — `openvhost-pkg` has no uninstall counterpart at all yet. `MysqlRow` renders an explicit note so it reads as a known limit rather than a missing button, and `brew uninstall mysql@8.4` still works.

The alternative — two rows for one major — buys that one affordance at the cost of the row model, on every page, permanently. Not worth it. The real fix is the slice that gives `openvhost-pkg` an uninstall.

### D7 — Never touch a Homebrew keg

This slice installs into our tree only. It does not `brew uninstall` anything, does not relink, does not migrate. A user who wants their brew MySQL gone does that themselves, or through the uninstall slice's brew path, which stays.

Two install sources coexisting is the *intended* state during a migration, not a bug to resolve early.

### D8 — Not in scope

- **Uninstalling a `packages/`-installed version** — slice 2, together with `current` repointing and the installed-version list. `openvhost-pkg` has **no uninstall counterpart to `install_package` at all** today; that is a slice, not a corner of this one.
- **The remote `openvhost/manifests` repo and index signing** — slice 6.
- **MariaDB, nginx, PHP, Apache** — slices 3–5.
- **Retiring the Homebrew paths** — slice 7, after every service has a new route.
- **`.tar.zst` / `.tar.xz`** — only needed once we build our own artifacts.

## What Slice 0 and Slice 1 settled

Every one of these was an open question when this spec was written. All are now measured; none blocks.

| Question | Answer |
|---|---|
| Does the extractor accept a real MySQL tarball? | **It did not** — three blockers. All fixed in PR #43, and the archive now installs in 5.19 s. |
| The reserved-name rule (`aux`/`con`/`nul`/…) | **Never fired.** The premise was wrong: this tarball ships no `mysql-test/`. The rule is untouched. |
| `MAX_ENTRIES` · `MAX_REL_BYTES` · `MAX_DEPTH` | **375 / 80 / 6** against 100 000 / 240 / 32. Untouched. |
| `strip_single_root` — the silent one | Was real: no explicit root header, so the install returned `Ok` one level too deep. Fixed, and its test asserts on the **tree**, never on `Result`. |
| Symlinks with `..` targets | 22 of 34, and **load-bearing** — dropping them gives a `mysqld` that SIGABRTs in dyld. The rule now bounds `..` by the link's own depth instead of banning it. |
| The 900 s download timeout | Was a **~1.5 Mbit/s floor** wearing a network error's clothes. Replaced with a 30 s idle window. |
| `otool -L bin/mysqld` | Only system paths and `@loader_path`. **Genuinely relocatable.** |
| `codesign -dv` | **Developer ID (Oracle America), hardened runtime** — and still valid after our extraction. |
| First-exec timing | 809 ms cold → 16 ms warm; pre-payable in staging. See D4. |

## Testing

- **Pure:** the catalogue entry; target selection (`macos-arm64` vs `macos-x86_64`); the recorded-version round trip; discovery preferring `packages/` over Homebrew and reporting the source of each runtime.
- **Real filesystem, tempdir:** an install lands at `package_dir(...)` and `current` points at it; a second identical install is a no-op rather than a re-download; a failed install leaves no partial tree and never touches `<home>/data/`.
- **Vacuity proof per group.** Be most suspicious of the "datadir untouched" and "no partial tree" assertions — assert content and **inode**, not a `Result`. The uninstall slice proved that necessary: rewriting kept files with byte-identical content at fresh inodes passed a content-only check.
- **Live proof:** install MySQL 8.4 from the real tarball into a hermetic `OPENVHOST_HOME` under `/tmp` (not `$TMPDIR` — the 103-byte `sun_path` ceiling), initialize a datadir, create a table, insert a row, restart, read it back. Then **confirm the owner's brew-installed `mysql@8.4` still works and was never touched.** Paste real output.

## Security posture

Network download, archive extraction, and a new install path that writes under `<home>`. `openvhost-pkg` carries a security-auditor APPROVE from PR #5, but **this is its first production use**, and golden rule 6 (runtime download with SHA-256 verification only) is exactly what it is being asked to honour. **security-auditor review is required.**

Claims to verify: the URL and SHA-256 reaching the downloader come only from the compiled-in catalogue and cannot be influenced by anything a user supplies; verification happens **before** extraction and on the same file descriptor, never re-opened by path; extraction stays inside the staging root; `<home>/data/` and the credential store are never written on any path including errors; and the atomic rename cannot promote a partially extracted tree.

## Verification owed to a human

1. Databases page installs MySQL 8.4 with no Homebrew involvement; watch the download and verification.
2. Initialize, connect with the generated password, create a table, read it back.
3. Restart the app; the service comes back and the data survives.
4. `brew list --versions` is unchanged — your own MySQL is untouched.
5. The UI says which install a running MySQL came from.
