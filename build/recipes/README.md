<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# The recipe interface

`build/build.sh <name> <version>` owns the sequence. A recipe supplies only what
is genuinely specific to one package:

```
fetch → verify → extract → configure → build → install → normalize
      → sign → audit → pack → verify-artifact → manifest
```

Stages in **bold** below are yours. The rest — signing, auditing, packing, the
manifest — are the driver's, are identical for every package, and are not
negotiable per recipe. That is the point of D7: nginx and PHP must slot in
without the driver changing. **If a package cannot be built without changing
`build.sh`, report it as a finding rather than editing the driver.**

Two departures from the sequence as written in the design, both deliberate and
explained at the top of `build.sh`: `sign` runs before `audit` (contract check 3
verifies signatures, which cannot exist yet on an unsigned tree), and
`verify-artifact` re-runs the whole contract on the packed tarball.

## A recipe is sourced, not run

`build.sh` and `audit.sh` both `source` the recipe. At source time a recipe may
**only** set variables and define functions. It must not download, compile, or
create anything: sourcing happens before the environment has been made hermetic,
and `audit.sh` sources recipes too, where no build environment exists at all.

`RECIPE_NAME` and `RECIPE_VERSION` are already set when the recipe is sourced,
so pins can be written in terms of the version being built.

Every recipe carries `# shellcheck shell=bash` and `# shellcheck disable=SC2034`
in its header: the file has no shebang, and the `RECIPE_*` variables look unused
to a tool that cannot see the driver reading them.

## Required variables

| Variable | Meaning |
|---|---|
| `RECIPE_SOURCE_URL` | where upstream's source archive comes from |
| `RECIPE_SOURCE_SHA256` | the pinned digest of that archive |

## Required functions

| Function | Must do |
|---|---|
| `recipe_fetch` | put the source archive, and any signature it needs, under `$BUILD_DOWNLOADS`. Use `bp_download`. |
| `recipe_verify_source` | verify upstream's signature **and** `RECIPE_SOURCE_SHA256`. Failing here must abort the build; provenance is verified, never assumed (§7). |
| `recipe_extract` | unpack into `$BUILD_SRC`. |
| `recipe_configure` | configure into `$BUILD_OBJ`, installing to `$BUILD_PREFIX`. Record every flag with `bp_record_flags` so it reaches the manifest. |
| `recipe_build` | compile. `$BUILD_JOBS` is the parallelism to use. |
| `recipe_install` | install into `$BUILD_PREFIX` — **not** into a staging directory that is later moved. |

## Optional variables

| Variable | Default | Meaning |
|---|---|---|
| `RECIPE_BUILD_TOOLS` | `()` | tools resolved to absolute paths **before** `PATH` is scrubbed. Anything not in `/usr/bin` or `/bin` belongs here: `cmake`, `make`, `bison`, `gpg`, `perl`. |
| `RECIPE_DEPENDS` | `()` | `name:version` entries built first, with `--stage-only`. Find them with `bp_dep_prefix <name> <version>`. |
| `RECIPE_IGNORE_PREFIXES` | `/opt/homebrew /usr/local /Applications/ServBay` | prefixes a configure step must ignore. Join them with `bp_ignore_prefix_path`. |
| `RECIPE_SERVER_BIN` | `""` | the server binary, relative to the tree root, that contract checks 5 and 6 exercise — `bin/mariadbd`, `sbin/nginx`. Empty means the package has no server, and both checks report `SKIPPED (no server binary)`. |
| `RECIPE_SERVER_VERSION_ARGS` | `(--version)` | how to ask that binary to identify itself and exit 0. |
| `RECIPE_SIGNING_KEY_FPR`, `RECIPE_SIGNING_KEY_EXPIRY`, `RECIPE_SIGNING_KEY_VERIFIED_ON` | `""` | upstream's signing key, its expiry, and when we last cross-checked the fingerprint against a second host. Recorded in the manifest. |
| `RECIPE_UPSTREAM_RELEASE_DATE`, `RECIPE_LAST_CHECKED` | `""` | §14's tripwire. A stale check must be visible in the source, not remembered. |

## Optional functions

| Function | Default | Must do |
|---|---|---|
| `recipe_normalize` | no-op | rewrite install names so every reference is `@loader_path/...`. Runs **before** signing, because `install_name_tool` invalidates a signature. Static linking makes this unnecessary, which is why D3 chose it. |
| `recipe_serve_probe <tree> <scratch>` | none | contract check 6. Start the server, create a table, insert a row, restart, read it back. Exit 0 only if the row came back. |
| `recipe_manifest_extra` | none | print one JSON value (object or array) recorded under `"recipe"` in the manifest. |

### What `recipe_serve_probe` may touch

It runs against `<tree>` **in place** and must write only inside `<scratch>` —
datadir, socket, pidfile, logs. `audit.sh` gives it a scratch directory under
the neutral build root, well inside the 103-byte `sun_path` ceiling that a unix
socket has. It must leave no process running when it returns, on either path.

## Helpers the driver provides

| Helper | Does |
|---|---|
| `bp_tool <name>` | absolute path of a tool declared in `RECIPE_BUILD_TOOLS`. **Never call a build tool by bare name** — ServBay's `bison` was on `PATH`, could not run at all, and broke the reference build (spec §2). |
| `bp_download <url> <dest>` | fetch to `<dest>` via a `.part` file, skipping work already done. |
| `bp_verify_sha256 <file> <sha256>` | abort unless the digest matches. |
| `bp_dep_prefix <name> <version>` | where a `RECIPE_DEPENDS` entry was staged. |
| `bp_ignore_prefix_path` | `RECIPE_IGNORE_PREFIXES` joined for `-DCMAKE_IGNORE_PREFIX_PATH`. |
| `bp_record_flags <flag>...` | add flags to the manifest's record. |
| `bp_rm_tree <path>` | `rm -rf` with the path validated first: absolute, not shallow, inside the build root. Use this instead of `rm -rf`. |
| `bp_log <msg>` / `bp_die <msg>` | report / abort. |
| `bp_machos <tree>` | list every Mach-O under a tree. |

## Paths a recipe may write to

| Variable | Is |
|---|---|
| `$BUILD_PREFIX` | `/tmp/openvhost-build/<name>-<version>` — the install prefix **and** the staged tree, in one place |
| `$BUILD_DOWNLOADS`, `$BUILD_SRC`, `$BUILD_OBJ` | scratch, under `/tmp/openvhost-build/_work/<name>-<version>/` |

Those, plus the output directory the driver owns, are the only places anything
may be written. **Nothing may touch `~/.openvhost`, a datadir, or Homebrew.**

`$BUILD_PREFIX` is deliberately meaningless (D8). Roughly fifty files in a
finished MariaDB tree embed the install prefix, and post-processing them all is
fragile; so the build installs to a stable, anonymous path and contract check 4
enforces that the builder's real directories never appear. **Install directly to
`$BUILD_PREFIX`.** A `DESTDIR` staging directory that is moved afterwards puts
the staging path into those fifty files, which is precisely the defect the
reference tree has.

## Passing the contract is the recipe's job

`build/audit.sh` runs twice per build — once on the staged tree, once on the
packed tarball — and a failure is a failed build. The checks are in
`docs/superpowers/specs/2026-08-02-p2-build-pipeline-design.md` §8. Two are
worth stating here because they shape how a recipe must be written:

- **Linkage.** Every `otool -L` entry must be `/usr/lib/*`, `/System/*`,
  `@loader_path/...` or `@rpath/...`, **and** every `LC_RPATH` must be
  `@loader_path`-relative. `@rpath` is only as relocatable as the rpaths that
  resolve it, and `otool -L` never shows one — so the second half is not a
  detail. A recipe whose binaries carry an absolute `LC_RPATH` must fix that in
  `recipe_normalize`; one that ships `@rpath/libfoo.dylib` alongside
  `LC_RPATH = @loader_path/../lib` is already correct and needs no rewriting.
- **Builder identity.** Nothing in the tree may contain the builder's home
  directory, username, session directories, or the tree's own ancestors.

If you are ever tempted to relax a contract check to make a build pass, that is
the moment the pipeline stops being worth having. Report it instead.

## Start from `_template.sh`

Copy `_template.sh` to `<name>.sh`. Names beginning with `_` are not package
names — `build.sh` rejects them — so the template can never be built by mistake.
