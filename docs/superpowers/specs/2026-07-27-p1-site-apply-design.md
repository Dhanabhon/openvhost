<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Site Apply Pipeline — Design

**Date:** 2026-07-27
**Status:** approved by owner, ready for implementation planning
**Slice:** Phase 1 — the first slice in which a site created in the UI is actually served

## 1. Goal

Make the Sites list real. Today a site created in the UI is a row in `state.db` and
nothing more: nginx serves a single hard-coded demo docroot on port 8080 and has never
heard of `state.db`. After this slice, pressing **Apply** turns the enabled sites into
the live nginx + php-fpm configuration, with a diff shown before anything is written and
an automatic rollback if the generated config does not validate.

Success criterion — a human can do this on a clean machine:

1. Create a site `myapp` → domain `myapp.localhost` → docroot pointing at a folder
   containing `index.php` and `style.css`.
2. Press Apply, read the diff, confirm.
3. Open `http://myapp.localhost:8080` and see the PHP page, with the stylesheet applied.

## 2. Starting position: two config worlds that do not meet

This is the fact that shapes the whole slice.

| | What runs today | What is generated today |
|---|---|---|
| Source | `demo_stack.rs` (P0-4), hard-coded `const` strings | `openvhost-conf` (P0-7), Tera templates |
| Output | `~/.openvhost/conf/nginx.conf`, `conf/php-fpm.conf` | `config/generated/nginx/{nginx.conf,sites/*.conf}`, `config/generated/php/<major>/php-fpm.conf` |
| Knows about sites | No — one docroot, `home/www`, port 8080 | Yes, `RenderCtx` is per-site |
| Reached at runtime | Yes, the supervisor spawns from it | **No — nothing calls it** |

The demo stack was the correct P0-4 answer (prove nginx + php-fpm can serve phpinfo);
it is the wrong Phase 1 answer. **This slice retires it**: the generated tree becomes
the single source of truth. The owner chose this over keeping both worlds in sync
(decision, §9).

Precisely what changes in `provision_macos_demo_stack`:

| Behaviour | Fate |
|---|---|
| Creating `conf/`, `www/`, `run/`, `run/nginx/`, `logs/` | **kept** — the generated tree still needs them, minus `conf/` |
| Writing `conf/nginx.conf` and `conf/php-fpm.conf` | **removed** — replaced by the generated set |
| Seeding `www/index.php` with phpinfo | **kept** — it becomes the catch-all's welcome page (§6.3) |
| `MAX_SOCKET_PATH_BYTES` guard | **kept**, now applied to the per-major socket (§4.7) |

`stack.rs` correspondingly points the nginx `ServiceSpec` at
`config/generated/nginx/nginx.conf` and the php-fpm spec at
`config/generated/php/<major>/php-fpm.conf`. Because a service must be registrable
before anything has ever been applied, **registration does not require those files to
exist**: an unapplied home yields a service whose Start fails honestly, which is the
existing P0-3 spawn-failure contract, and the pending-changes banner is what tells the
user to apply.

## 3. Architecture

New module `openvhost-core::site::apply`. `openvhost-core` gains a dependency on
`openvhost-conf` — no cycle, since `openvhost-conf` depends on neither, and the
ownership map (plan §6.2) already assigns "openvhost-conf glue" to the core engineer.
Core stays `tauri`-free, so the `openvhost` CLI reuses `apply()` verbatim when it lands.

Four pieces, each independently testable:

| Piece | Responsibility | IO |
|---|---|---|
| `render_set(&ApplyInput) -> Result<Vec<GeneratedFile>, ApplyError>` | The whole desired config set from sites + installed runtimes | none (pure) |
| `plan(desired, gen_root) -> Result<ApplyPlan, ApplyError>` | Read the on-disk tree, classify Added / Modified / Removed, keep previous contents | read only |
| `commit(&ApplyPlan)` / `rollback(&ApplyPlan)` | Atomic writes (tmp + rename) and deletions / restore from the plan's snapshot | write |
| `apply(...) -> Result<ApplyOutcome, ApplyError>` | commit → validate → rollback on failure | orchestrates |

**Restarting is not core's job.** `apply()` returns an `ApplyOutcome`; the desktop app
owns the `Supervisor` and decides what to restart. This is what keeps the CLI able to
call the same function with no supervisor in the process.

**Probing is not core's job either.** `ApplyInput` carries the discovered runtimes as
data:

```rust
pub struct InstalledRuntimes {
    pub nginx_bin: PathBuf,
    pub php: Vec<PhpRuntime>,   // { major: String, fpm_bin: PathBuf }, ordered
}

pub struct ApplyInput {
    pub home: PathBuf,
    pub sites: Vec<Site>,       // all of them; render_set filters on `enabled`
    pub runtimes: InstalledRuntimes,
}
```

The caller probes (today `find_brew_binaries()` plus §3.3) and passes the result in, so
every unit test constructs runtimes by hand and no test depends on what happens to be
installed on the machine running it. `php` being ordered gives the catch-all a defined
"first installed major" (§6.3).

`plan()` needs no binary at all — only `apply()` does, for `nginx -t`. This is what lets
the banner refresh cheaply on every site mutation without spawning a process.

### 3.1 The generated set

All paths below are relative to `<home>/config/generated/`:

- `nginx/nginx.conf` — one main config.
- `nginx/sites/00-default.conf` — the catch-all `default_server`.
- `nginx/sites/<domain>.conf` — one per **enabled** site.
- `php/<major>/php-fpm.conf` — one per **installed** PHP major, not per site.

### 3.2 Service set follows installed runtimes, not sites

The Services panel rows are registered from the runtimes present on the machine. Apply
rewrites their config files and restarts them; it never adds or removes a service. This
matters because `Supervisor` has `register()` but no `unregister()`, and this invariant
means we do not need one. With Homebrew's single PHP that is one `php-fpm` row today,
while the shape already supports several.

### 3.3 New probe

`openvhost-conf::inspect` gains `probe_php_fpm_version(bin, …) -> Option<String>`,
modelled on the existing `probe_nginx_version`, parsing the major.minor out of
`php-fpm -v`. The installed major is needed in three places: the pool path, the
`RenderCtx.php_major`, and the runtime check in §4.2. Deriving it from the Homebrew
directory name instead would break the moment packages move to `packages/` (P0-6).

## 4. Data flow and semantics

```
sites (state.db, enabled only) + installed runtimes (nginx bin; php-fpm bin + major)
      │
      ▼  render_set — pure
desired: Vec<GeneratedFile>
      │
      ▼  plan — reads config/generated/
ApplyPlan { Added / Modified / Removed, each with its previous contents }
      │
      ▼  diff shown, user confirms
commit  (tmp + rename per file; delete stale files)
      │
      ▼  validate_live(nginx_bin, main_conf, err_log)
 ok   ──────────────────► restart running services ──► ApplyOutcome
 fail ──► rollback ─────────────────────────────────► ApplyError::ValidationFailed
```

Writing before validating is safe because a running nginx holds its configuration in
memory: the files on disk have no effect until the restart, which only happens after a
green `nginx -t`. In exchange we validate **the exact files that will run**, rather than
a temp-directory reconstruction whose every embedded path differs from the real one.

### 4.1 Ordering rule

Every check that can fail without touching the filesystem runs first. Once `commit`
begins, the only exits are success, rollback, or `RollbackFailed`.

### 4.2 PHP version handling

If an enabled site requests a PHP major that is not installed, apply fails with
`MissingRuntime { site, requested, available }` **before any file is written**. Nothing
is silently substituted: a config that claims 8.3 while serving 8.4 is a lie the user
would eventually debug the hard way.

`PhpVersion` already parses as `major.minor`, which is exactly `RenderCtx.php_major`, so
no conversion is needed.

### 4.3 Disabled sites

Excluded from the desired set, so their existing config file appears in the plan as
**Removed**. Disabling a site removes it from nginx rather than leaving it served with a
flag flipped in the database.

### 4.4 Deletion scope

Stale-file removal is confined to files matching `nginx/sites/*.conf` and
`php/*/php-fpm.conf` under `config/generated/`. `config/custom/` is never read for
planning and never written or deleted — it is the user's half of the split that
principle 3 of the plan establishes.

### 4.5 Filenames

Site config filenames derive from `domain`, which carries a `UNIQUE` constraint in the
`sites` table, so two enabled sites cannot collide by construction.

### 4.6 Restart policy

Only services already running are restarted. A service that is `Stopped` stays stopped,
and the outcome says the new config applies when it is next started. Order is php-fpm
first, then nginx, so the socket exists before nginx connects. `quit::stop_all_with()`
is reused to wait for a genuine `Stopped` state rather than assuming the stop took
effect.

### 4.7 Fixed values

- Listen address: `127.0.0.1:8080` for every site (name-based virtual hosting). Port 80
  needs the privileged helper and is Phase 3.
- Socket per major: `run/php-fpm-<major>.sock`. At `~/.openvhost` that is roughly 45
  bytes, well under the 103-byte `sun_path` ceiling; the existing
  `MAX_SOCKET_PATH_BYTES` guard stays and still governs.
- `*.localhost` resolves to 127.0.0.1 on macOS without any hosts-file edit (verified),
  which is what makes the MVP hosts story "do nothing" as the plan anticipated.

## 5. Error surface

```rust
pub enum ApplyError {
    MissingRuntime { site: String, requested: String, available: Vec<String> },
    NoWebServerBinary,
    Render(ConfError),
    Io { op: &'static str, path: PathBuf, source: std::io::Error },
    ValidationFailed { stderr: String },
    RollbackFailed { original: Box<ApplyError>, rollback: Box<ApplyError>, stranded: Vec<PathBuf> },
}
```

| Variant | Disk state afterwards |
|---|---|
| `MissingRuntime`, `NoWebServerBinary`, `Render` | untouched |
| `Io`, `ValidationFailed` | rolled back, byte-identical to before |
| `RollbackFailed` | **mixed — stated explicitly, with the stranded paths listed** |

`RollbackFailed` carries both errors and the paths it could not restore. Collapsing it
into a generic failure would leave the user with a tree matching neither the old nor the
new configuration and no way to know it.

## 6. Templates

### 6.1 `nginx/main.conf.tera` — MIME types

Add an inline `types { … }` block plus `default_type application/octet-stream;`.
Without it every response is octet-stream and browsers refuse to apply stylesheets.

Inlined rather than `include /opt/homebrew/etc/nginx/mime.types` because generated
output must be deterministic and independent of the Homebrew layout that P0-6 is already
replacing. The block covers html, css, js, json, svg, png, jpg, gif, webp, woff2, ico,
txt, xml and mp4; anything further goes in `config/custom/`.

### 6.2 `nginx/site.conf.tera` — a real PHP vhost

The current template routes **every** request into FastCGI with `SCRIPT_FILENAME` fixed
at `$document_root/index.php`. That is adequate for a phpinfo demo and broken for any
real site: stylesheets and images are handed to the PHP interpreter, and every URL
returns `index.php`. Replaced with:

```nginx
index index.php index.html;

location / {
    try_files $uri $uri/ /index.php$is_args$args;
}

location ~ \.php$ {
    try_files $uri =404;
    fastcgi_split_path_info ^(.+\.php)(/.+)$;
    {{ php_pass }}
    fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
    fastcgi_param PATH_INFO       $fastcgi_path_info;
    fastcgi_param REDIRECT_STATUS 200;
    # … SCRIPT_NAME, REQUEST_URI, DOCUMENT_ROOT, DOCUMENT_URI, QUERY_STRING,
    #   REQUEST_METHOD, CONTENT_TYPE, CONTENT_LENGTH, SERVER_PROTOCOL,
    #   GATEWAY_INTERFACE, SERVER_SOFTWARE, REMOTE_ADDR, REMOTE_PORT,
    #   SERVER_ADDR, SERVER_NAME, SERVER_PORT, HTTPS
    include "{{ custom_site_glob }}";
}

location ~ /\. { deny all; }
```

Two of these lines are load-bearing beyond correctness:

- **`try_files $uri =404;`** inside the PHP location. Without it, a file uploaded as
  `avatar.jpg` whose bytes are PHP executes via `/uploads/avatar.jpg/x.php`. This is the
  standard path-info arbitrary-execution hole, and the guard is the standard fix.
- **`REDIRECT_STATUS 200`.** php-fpm rejects requests that lack it, and the resulting
  bare "Access denied" is disproportionately hard to diagnose.

`location ~ /\.` keeps `.env` and `.git` inside a docroot from being served.

### 6.3 `nginx/default-site.conf.tera` — new

The catch-all: `listen … default_server;` with `server_name _;`, rooted at `home/www`,
serving the existing phpinfo page as a welcome screen and using the first installed PHP
major. It needs its own template because `_` is not a hostname and `RenderCtx`'s
validator correctly rejects it.

With zero sites, or a request for an unmatched host, this is what answers — so nginx
always has at least one `server` block and always starts. When no PHP runtime is
installed at all, the catch-all is generated static-only (no PHP location); any site
needing PHP is already blocked by §4.2.

## 7. UI

- **Pending-changes banner** in `SitesPanel`, shown when `plan.changes` is non-empty:
  the count plus a button opening the dialog. Refreshed on panel mount and after every
  site mutation.
- **`ApplyDialog.svelte`** — the changed files with Added / Modified / Removed badges and
  a unified diff in monospace. The `+`/`-` colours must meet AA in both themes (the
  contrast regression behind PR #22 came from exactly this kind of new token).
- **Four explicit states:** idle, applying (buttons disabled), succeeded (naming which
  services were restarted), failed.
- **Error rendering:** `nginx -t` stderr is displayed `pre-wrap` (the ServiceRow lesson —
  unwrapped stderr ran off-screen). `MissingRuntime` renders as a readable list of
  site / requested / available, never a debug-formatted string.

Diffing is done in Rust with `similar` (MIT) so that the CLI later produces identical
output rather than a second, divergent implementation.

## 8. Testing

**Rust unit — `openvhost-core::site::apply`**

- `render_set` is deterministic for identical input.
- The catch-all is present in every generated set, including the zero-site set.
- One pool per installed major, deduplicated, regardless of how many sites use it.
- A disabled site produces a `Removed` change for its existing file.
- `MissingRuntime` is returned with the filesystem untouched — asserted by comparing a
  full tree snapshot, not by trusting the call order.
- `plan` classifies Added, Modified and Removed correctly against a temp tree.
- **commit + rollback restores the tree byte-for-byte**, driven by an injected validator
  that always fails.
- `config/custom/` is neither modified nor deleted by any apply.

**Rust integration (macOS, Homebrew nginx):** a real apply against a temp home passes
`nginx -t`.

**E2E, extending the P0-9 harness:** create a temp site whose docroot holds `index.php`
and `style.css`, apply, start the stack, then assert three things:

1. The PHP page renders (PHP is genuinely wired).
2. `style.css` returns `Content-Type: text/css` — the §6.1 regression test.
3. `/style.css/x.php` returns 404 — the §6.2 regression test.

Assertions 2 and 3 exist because those are the two defects this slice fixes; they are
regression tests, not decoration.

**Frontend (vitest SSR):** the banner appears only with a non-empty plan; the diff view
renders all three change kinds; a failed apply surfaces the stderr; controls are
disabled while applying. Every assertion must be able to fail for the reason it claims —
the nine unfalsifiable tests found in the status-bar slice are the standard to avoid.

## 9. Decisions taken (owner, 2026-07-27)

1. **Retire the demo stack** rather than layering generated vhosts on top of it. Two
   config worlds needing manual synchronisation is a bug generator.
2. **Block on a missing PHP version**, naming the site and the available versions,
   rather than skipping the site or silently serving it with another version.
3. **Restart through the supervisor** instead of adding a signal/reload API. SIGHUP
   hot-reload is a later slice; new php-fpm pools require a restart regardless.
4. **One Apply for the whole set**, with a pending-changes banner, rather than per-site
   apply — nginx has a single configuration set, and per-site apply would regenerate and
   restart everything anyway while implying otherwise.
5. **Catch-all landing page** so nginx always starts and an unmatched host gets an
   explanation instead of a connection error.
6. **Orchestration in `openvhost-core`**, write-then-validate-with-rollback. Alternatives
   considered: a separate `openvhost-apply` crate (cleaner layering, a fifth crate for
   ~400 lines) and orchestration in the desktop app (rejected — the CLI could not reuse
   it, against plan principle 4).

## 10. Gates

- **security-auditor is a merge blocker.** The slice adds two Tauri commands, and the
  IPC command surface is on the golden-rule-2 list. The reviewer should also confirm the
  §6.2 execution guard and the `config/custom/` deletion boundary.
- **License gate** for the new `similar` dependency (MIT, GPL-3.0-compatible).
- No `unwrap`/`expect` outside tests; every file write atomic; `cargo fmt` +
  `clippy -D warnings` + `svelte-check` clean; both Rust and UI suites green.

## 11. Out of scope

Hot reload without restart · hosts-file management (unnecessary while `*.localhost`
works) · Apache adapter · HTTPS · per-site custom directives in the UI · multiple PHP
versions actually installed (that is the package-manager slice; this slice only makes
the seam correct) · the `openvhost` CLI surface itself.
