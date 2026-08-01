# Slice 1 — MySQL from the upstream tarball, off Homebrew

- **Date:** 2026-08-01
- **Status:** Written ahead of Slice 0's payload proof. **Every fact Slice 0 is measuring is marked `[S0]` and is a blank, not an assumption.** The design shape does not depend on those answers; the catalogue entry and two limits do.
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

`[S0]` The exact version, URL and SHA-256 come from Slice 0. Also from Slice 0: whether upstream publishes a `.asc` for the **macOS** artifact specifically — we compute our own SHA-256 either way, but the upstream signature is the trust anchor for that computation and its absence is worth recording.

### D3 — Discovery reads our tree *and* Homebrew, in that order

**The owner is running a brew-installed `mysql@8.4 8.4.11` right now.** Stranding them would be a self-inflicted wound in the first slice of the migration.

So discovery gains a `packages/`-tree walk and keeps the existing Homebrew walk as a fallback. Ours wins where both exist — we know its version exactly, brew's we would have to probe. One `if`; a cheap hedge that buys the whole migration room to be incremental. Brew discovery is retired in slice 7, not here.

**The UI must say where a runtime came from.** A user debugging "which mysqld am I actually running" should not have to guess, and during a migration that question gets asked.

### D4 — The version is recorded at install and never probed again

This is the ServBay property and the structural cure for the defect the uninstall slice's live proof found: after a real `brew install mysql@8.4`, the app reported nothing installed, because the **first** execution of the freshly extracted 55 MB `mysqld` takes **11.53 s** (Gatekeeper first-run scan; second exec 0.039 s) against a 5 s `PROBE_TIMEOUT`.

The research established that this cost **follows us into the new model** — the binary that took 11.53 s was not quarantined, so the trigger is simply "first exec of a new binary", which our pipeline reproduces exactly. A larger timeout only moves the cliff.

We installed it, so we know what it is: `state.db` records the exact version at install time. Probing survives **only** for discovering Homebrew runtimes we did not install.

`[S0]` Whether a package installed through our own pipeline pays the same first-exec cost, and whether it lands before or after the atomic rename. If it recurs *after* the rename, every install pays it twice and we should do one deliberate warm-up exec at the end of install, where the user already expects to wait.

### D5 — Spawn a resolved concrete path, never `current`

`current` is for humans and for future upgrade flows. The supervisor must resolve it to a concrete `packages/mysql/8.4/<version>/` path **at spawn time** and record that path with the process.

Spawning *through* the symlink means a `current` swap silently changes which engine a restart brings up — the running process and the one the UI describes would diverge with nothing in between to notice. It also makes `mysqld`'s argv[0]-derived basedir ambiguous, which is exactly the class of thing that cost this project a full misdiagnosis in the MySQL slice.

### D6 — The datadir is untouched, and it is shared across install sources

The datadir stays at `<home>/data/mysql/<major>/`, keyed by **major**, not by where the binaries came from. A package install must never write there.

That is deliberate: a user who has an initialized 8.4 datadir from the brew era and then gets the tarball-installed 8.4 **keeps their databases**. Same major, same MySQL, same on-disk format.

`[S0]` One thing to confirm rather than assume: the upstream tarball's exact minor versus the brew-installed `8.4.11`. MySQL will not open a datadir initialized by a *newer* server. If the pinned tarball is older than 8.4.11, pin forward instead — and the catalogue must never move a user's server backwards.

### D7 — Never touch a Homebrew keg

This slice installs into our tree only. It does not `brew uninstall` anything, does not relink, does not migrate. A user who wants their brew MySQL gone does that themselves, or through the uninstall slice's brew path, which stays.

Two install sources coexisting is the *intended* state during a migration, not a bug to resolve early.

### D8 — Not in scope

- **Uninstalling a `packages/`-installed version** — slice 2, together with `current` repointing and the installed-version list. `openvhost-pkg` has **no uninstall counterpart to `install_package` at all** today; that is a slice, not a corner of this one.
- **The remote `openvhost/manifests` repo and index signing** — slice 6.
- **MariaDB, nginx, PHP, Apache** — slices 3–5.
- **Retiring the Homebrew paths** — slice 7, after every service has a new route.
- **`.tar.zst` / `.tar.xz`** — only needed once we build our own artifacts.

## Facts Slice 0 is measuring, and what each one changes

These are blanks in this spec, not assumptions:

| `[S0]` | If it goes the wrong way |
|---|---|
| Does the extractor accept a real MySQL tarball at all? | The reserved-name rule (`aux`/`con`/`nul`/`com0-9`/`lpt0-9`) rejects the **entire archive** if any path component's stem matches — plausible inside `mysql-test/`. A relaxation was part of the P0-6 APPROVE, so it is a security-auditor conversation, not a code change. **This can block the whole programme.** |
| `MAX_ENTRIES` 100 000 · `MAX_REL_BYTES` 240 · `MAX_DEPTH` 32 | A DB tarball with a large test suite may exceed these. Raising a limit is a design change with an audit trail, not a tweak. |
| `strip_single_root` | **The only silent failure in the set.** It strips only when the root is an explicit directory entry; otherwise the install "succeeds" one level too deep and discovery finds no `bin/mysqld` — indistinguishable from D4's symptom, and it would send a diagnosis straight down the wrong path. |
| Symlinks with `..` targets | Rejected today; DB tarballs contain symlinks. |
| 900 s total download timeout vs ~200 MB | Caps a 200 MB download at roughly 2 Mbit/s. |
| `otool -L bin/mysqld` | Tells us whether upstream's tarball is genuinely relocatable or expects a fixed prefix. |
| `codesign -dv` | An unsigned arm64 binary is **killed at exec**. Adhoc is fine; unsigned is not. |
| First-exec timing | D4. |

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
