<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Site Apply Pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the enabled sites in `state.db` into the live nginx + php-fpm configuration, with a diff preview before writing, real-file validation, and rollback on failure.

**Architecture:** A new `openvhost-core::site::apply` module renders the whole config set from sites plus the installed runtimes (pure), diffs it against `<home>/config/generated/`, commits atomically, validates the real files with `nginx -t`, and rolls back byte-for-byte if validation fails. The desktop app probes runtimes once at startup, exposes two IPC commands, and restarts the running services after a green apply. The P0-4 demo stack stops writing configs.

**Tech Stack:** Rust 2021 (tokio, thiserror, async-trait, Tera, `similar` for diffs), Tauri 2 + tauri-specta, SvelteKit + Svelte 5 runes, vitest.

**Source spec:** `docs/superpowers/specs/2026-07-27-p1-site-apply-design.md`

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later` (`<!-- ... -->` for `.svelte`, `; ...`/`# ...` for config templates).
- Every commit is DCO-signed: `git commit -s`. Conventional Commits (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`).
- No `unwrap()`/`expect()` outside `#[cfg(test)]`. The workspace denies `clippy::unwrap_used`/`expect_used` under `-D warnings`.
- All live-tree file writes are atomic (temp file in the same directory, then rename).
- `openvhost-core` must never depend on `tauri`.
- Gate before every commit: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`, plus `pnpm -C apps/desktop test` and `pnpm -C apps/desktop exec svelte-check` for frontend tasks.
- In a fresh worktree run `pnpm install --offline --frozen-lockfile` in `apps/desktop` before any frontend gate, or it fails with a bogus "Cannot find package".
- Listen address is `127.0.0.1:8080` for every site. Socket paths are `<home>/run/php-fpm-<major>.sock` and must stay within 103 bytes.
- Service ids are `nginx` and `php-fpm-<major>` (e.g. `php-fpm-8.4`).
- Tauri DTOs must not expose `usize`/`isize` — specta rejects them (see the comment in `apps/desktop/src-tauri/src/lib.rs`). Use `u32`.
- Task 7 adds IPC commands, so the branch is **merge-blocked pending a security-auditor APPROVE** (CLAUDE.md golden rule 2).

## File Structure

**`crates/openvhost-conf`**
- `templates/nginx/main.conf.tera` — modify: add `types {}` + `default_type`.
- `templates/nginx/site.conf.tera` — modify: real vhost (static files, front controller, PHP location).
- `templates/nginx/php-location.conf.tera` — create: the single definition of the PHP `location` block, shared by site and catch-all.
- `templates/nginx/default-site.conf.tera` — create: the catch-all `default_server`.
- `src/engine.rs` — modify: register the two new templates.
- `src/webserver.rs` — modify: `generate_main_config(home)`, new `generate_php_location`/`generate_default_site_config`.
- `src/phpruntime.rs` — modify: `generate_pool_config(home, major, upstream)`.
- `src/inspect.rs` — modify: add `probe_php_fpm_version`.
- `src/lib.rs` — modify: exports.

**`crates/openvhost-core`**
- `Cargo.toml` — modify: depend on `openvhost-conf` and `similar`.
- `src/site/apply/mod.rs` — create: public types, `render_set`, re-exports.
- `src/site/apply/error.rs` — create: `ApplyError`, `RollbackReport`.
- `src/site/apply/plan.rs` — create: `plan`, owned-file discovery, diff rendering.
- `src/site/apply/commit.rs` — create: `commit`, `rollback`, `atomic_write`, `apply`, `ConfigValidator`, `NginxValidator`.
- `src/site/mod.rs`, `src/lib.rs` — modify: wire the module up.
- `src/platform/macos/demo_stack.rs` — modify: stop writing configs.
- `tests/macos_stack.rs` — modify: assert the new provisioning contract.
- `tests/site_apply_e2e.rs` — create: the serve-it-for-real test.

**`apps/desktop`**
- `src-tauri/src/stack.rs` — modify: probe runtimes, point specs at the generated tree.
- `src-tauri/src/commands.rs` — modify: two commands + DTOs.
- `src-tauri/src/lib.rs` — modify: manage `InstalledRuntimes`, register commands.
- `src/lib/ipc/bindings.ts` — regenerated, committed.
- `src/lib/ipc/index.ts` — modify: typed wrappers.
- `src/lib/apply.svelte.ts` (+ `.test.ts`) — create: the store.
- `src/lib/components/PendingChangesBanner.svelte` — create.
- `src/lib/components/ApplyDialog.svelte` (+ `.test.ts`) — create.
- `src/routes/+page.svelte` — modify: wire banner + dialog.
- `src/lib/components/WebServerRow.svelte`, `webserver.panel.test.ts` — modify: copy now names the generated config.

---

## Task 1: nginx templates that can serve a real site

The current site template hands every request — stylesheets, images, everything — to PHP with `SCRIPT_FILENAME` pinned to `index.php`, and the main config declares no MIME types. Both must be fixed before any site is worth serving.

Two adapter signatures also change here. `generate_main_config` and `generate_pool_config` take a whole `RenderCtx` but read only `home` (and, for the pool, `php_major` + `php_upstream`). Task 3 would otherwise have to invent a fake site to render a non-site file. Removing the unused parameters is what stops that.

**Files:**
- Modify: `crates/openvhost-conf/templates/nginx/main.conf.tera`
- Create: `crates/openvhost-conf/templates/nginx/php-location.conf.tera`
- Create: `crates/openvhost-conf/templates/nginx/default-site.conf.tera`
- Modify: `crates/openvhost-conf/templates/nginx/site.conf.tera`
- Modify: `crates/openvhost-conf/src/engine.rs`
- Modify: `crates/openvhost-conf/src/webserver.rs`
- Modify: `crates/openvhost-conf/src/phpruntime.rs`
- Test: `crates/openvhost-conf/src/webserver.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  ```rust
  // trait WebServerAdapter
  fn generate_main_config(&self, home: &Path) -> Result<GeneratedFile, ConfError>;
  fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;  // unchanged signature
  fn generate_default_site_config(
      &self,
      home: &Path,
      listen: SocketAddr,
      php_upstream: Option<&PhpUpstream>,
  ) -> Result<GeneratedFile, ConfError>;

  // trait PhpRuntimeAdapter
  fn generate_pool_config(
      &self,
      home: &Path,
      major: &str,
      upstream: &PhpUpstream,
  ) -> Result<Option<GeneratedFile>, ConfError>;
  ```
  The catch-all is written to `<home>/config/generated/nginx/sites/00-default_server.conf`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/openvhost-conf/src/webserver.rs`, inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn main_config_declares_mime_types() {
    let f = NginxAdapter
        .generate_main_config(std::path::Path::new("/tmp/ovh"))
        .unwrap();
    let c = &f.contents;
    // Without a types block nginx labels every response octet-stream and
    // browsers refuse to apply the stylesheet.
    assert!(c.contains("text/css                              css;"));
    assert!(c.contains("application/javascript                js mjs;"));
    assert!(c.contains("default_type application/octet-stream;"));
}

#[test]
fn site_config_serves_static_files_without_php() {
    let c = NginxAdapter.generate_site_config(&unix_ctx()).unwrap().contents;
    // The front controller handles unknown paths; real files are served as files.
    assert!(c.contains("try_files $uri $uri/ /index.php$is_args$args;"));
    assert!(c.contains("index index.php index.html;"));
    // SCRIPT_FILENAME must follow the request, not be pinned to index.php.
    assert!(c.contains("fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
    assert!(!c.contains("$document_root/index.php"));
}

#[test]
fn php_location_refuses_to_execute_a_path_that_is_not_a_file() {
    let c = NginxAdapter.generate_site_config(&unix_ctx()).unwrap().contents;
    // Without this, an uploaded avatar.jpg containing PHP executes via
    // /uploads/avatar.jpg/x.php. The guard is the whole defence.
    assert!(c.contains("try_files $uri =404;"));
    assert!(c.contains("fastcgi_param REDIRECT_STATUS 200;"));
    assert!(c.contains("location ~ /\\. {"));
}

#[test]
fn default_site_is_the_catch_all_and_can_run_php() {
    let sock = PathBuf::from("/tmp/ovh/run/php-fpm-8.4.sock");
    let up = PhpUpstream::UnixSocket(sock);
    let f = NginxAdapter
        .generate_default_site_config(
            std::path::Path::new("/tmp/ovh"),
            "127.0.0.1:8080".parse().unwrap(),
            Some(&up),
        )
        .unwrap();
    assert_eq!(
        f.path,
        PathBuf::from("/tmp/ovh/config/generated/nginx/sites/00-default_server.conf")
    );
    let c = &f.contents;
    assert!(c.contains("listen 127.0.0.1:8080 default_server;"));
    assert!(c.contains("server_name _;"));
    assert!(c.contains(r#"root "/tmp/ovh/www";"#));
    assert!(c.contains(r#"fastcgi_pass "unix:/tmp/ovh/run/php-fpm-8.4.sock";"#));
}

#[test]
fn default_site_without_php_has_no_fastcgi_at_all() {
    let f = NginxAdapter
        .generate_default_site_config(
            std::path::Path::new("/tmp/ovh"),
            "127.0.0.1:8080".parse().unwrap(),
            None,
        )
        .unwrap();
    let c = &f.contents;
    assert!(c.contains("default_server;"));
    // A fastcgi_pass with no pool behind it is a 502 generator.
    assert!(!c.contains("fastcgi_pass"));
    assert!(!c.contains("location ~ [^/]\\.php"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openvhost-conf`
Expected: FAIL — `generate_main_config` takes a `&RenderCtx` (mismatched types) and `generate_default_site_config` does not exist.

- [ ] **Step 3: Add the MIME types to the main template**

In `crates/openvhost-conf/templates/nginx/main.conf.tera`, inside `http {`, immediately after the `access_log` line:

```
    types {
        text/html                             html htm;
        text/css                              css;
        text/xml                              xml;
        text/plain                            txt;
        application/javascript                js mjs;
        application/json                      json map;
        application/pdf                       pdf;
        application/zip                       zip;
        image/svg+xml                         svg svgz;
        image/png                             png;
        image/jpeg                            jpeg jpg;
        image/gif                             gif;
        image/webp                            webp;
        image/avif                            avif;
        image/x-icon                          ico;
        font/woff                             woff;
        font/woff2                            woff2;
        font/ttf                              ttf;
        video/mp4                             mp4;
    }
    default_type application/octet-stream;
```

Inlined rather than `include`-ing Homebrew's `mime.types`: generated output must be deterministic and must not depend on a layout P0-6 is already replacing.

- [ ] **Step 4: Create the shared PHP location template**

Create `crates/openvhost-conf/templates/nginx/php-location.conf.tera`:

```
    location ~ [^/]\.php(/|$) {
        try_files $uri =404;
        fastcgi_split_path_info ^(.+?\.php)(/.*)$;
        {{ php_pass }}
        fastcgi_index index.php;
        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;
        fastcgi_param SCRIPT_NAME $fastcgi_script_name;
        fastcgi_param PATH_INFO $fastcgi_path_info;
        fastcgi_param REDIRECT_STATUS 200;
        fastcgi_param QUERY_STRING $query_string;
        fastcgi_param REQUEST_METHOD $request_method;
        fastcgi_param CONTENT_TYPE $content_type;
        fastcgi_param CONTENT_LENGTH $content_length;
        fastcgi_param REQUEST_URI $request_uri;
        fastcgi_param DOCUMENT_URI $document_uri;
        fastcgi_param DOCUMENT_ROOT $document_root;
        fastcgi_param SERVER_PROTOCOL $server_protocol;
        fastcgi_param GATEWAY_INTERFACE CGI/1.1;
        fastcgi_param SERVER_SOFTWARE nginx;
        fastcgi_param REMOTE_ADDR $remote_addr;
        fastcgi_param REMOTE_PORT $remote_port;
        fastcgi_param SERVER_ADDR $server_addr;
        fastcgi_param SERVER_PORT $server_port;
        fastcgi_param SERVER_NAME $server_name;
        include "{{ custom_site_glob }}";
    }
```

One definition, rendered in Rust and injected into both the site and catch-all templates, so the parameter list cannot drift between them.

- [ ] **Step 5: Rewrite the site template**

Replace the body of `crates/openvhost-conf/templates/nginx/site.conf.tera` (keep the existing banner comment lines verbatim):

```
{{ php_upstream_block }}server {
    listen {{ listen_addr }};
    server_name {{ server_name }};
    root "{{ docroot }}";
    index index.php index.html;

    location / {
        try_files $uri $uri/ /index.php$is_args$args;
    }

{{ php_location }}
    location ~ /\. {
        deny all;
    }
}
```

- [ ] **Step 6: Create the catch-all template**

Create `crates/openvhost-conf/templates/nginx/default-site.conf.tera`:

```
# ---------------------------------------------------------------------------
# GENERATED by OpenVHost — DO NOT EDIT. This is the catch-all served when a
# request matches no site. To customize, add files under:
#   {{ custom_sites_dir }}
# ---------------------------------------------------------------------------
server {
    listen {{ listen_addr }} default_server;
    server_name _;
    root "{{ docroot }}";
    index index.php index.html;

    location / {
        try_files $uri $uri/ =404;
    }

{{ php_location }}
    location ~ /\. {
        deny all;
    }
}
```

- [ ] **Step 7: Register the new templates**

In `crates/openvhost-conf/src/engine.rs`, add two constants next to the existing ones and two entries to `add_raw_templates`:

```rust
const PHP_LOCATION: &str = include_str!("../templates/nginx/php-location.conf.tera");
const DEFAULT_SITE_NGINX: &str = include_str!("../templates/nginx/default-site.conf.tera");
```

```rust
            ("nginx/php-location.conf", PHP_LOCATION),
            ("nginx/default-site.conf", DEFAULT_SITE_NGINX),
```

- [ ] **Step 8: Update the adapter**

In `crates/openvhost-conf/src/webserver.rs`:

Change the trait to:

```rust
#[async_trait]
pub trait WebServerAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn generate_main_config(&self, home: &Path) -> Result<GeneratedFile, ConfError>;
    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    fn generate_default_site_config(
        &self,
        home: &Path,
        listen: std::net::SocketAddr,
        php_upstream: Option<&PhpUpstream>,
    ) -> Result<GeneratedFile, ConfError>;
    async fn validate(&self, bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError>;
    fn supports_hot_reload(&self) -> bool;
}
```

Change `generate_main_config`'s body to take `home: &Path` (replace every `ctx.home` with `home`, and `Self::gen_dir(&ctx.home)` with `Self::gen_dir(home)`).

Factor the upstream branch out of `generate_site_config` so the catch-all can reuse it, and add the two new renderers:

```rust
impl NginxAdapter {
    fn gen_dir(home: &Path) -> PathBuf {
        home.join("config/generated/nginx")
    }

    /// The OS branch, in Rust rather than in a Tera conditional: the platform
    /// seam stays type-checked (spec 2026-07-23 §4).
    fn upstream_parts(upstream: &PhpUpstream, upstream_name: &str) -> Result<(String, String), ConfError> {
        Ok(match upstream {
            PhpUpstream::UnixSocket(p) => {
                let sock = to_config_path(p)?;
                (String::new(), format!("fastcgi_pass \"unix:{sock}\";"))
            }
            PhpUpstream::TcpPorts(ports) => {
                let mut block = format!("upstream {upstream_name} {{\n");
                for addr in ports {
                    block.push_str(&format!("    server {addr} max_fails=1 fail_timeout=1s;\n"));
                }
                block.push_str("}\n\n");
                let pass = format!(
                    "fastcgi_pass {upstream_name};\n        fastcgi_next_upstream error timeout invalid_header http_500;"
                );
                (block, pass)
            }
        })
    }

    /// The PHP `location` block, or an empty string when no PHP runtime is
    /// installed — a `fastcgi_pass` with no pool behind it only produces 502s.
    fn php_location(
        home_str: &str,
        server_name: &str,
        php_pass: &str,
    ) -> Result<String, ConfError> {
        let mut tc = tera::Context::new();
        tc.insert("php_pass", php_pass);
        tc.insert(
            "custom_site_glob",
            &format!("{home_str}/config/custom/sites/{server_name}.d/*.conf"),
        );
        render("nginx/php-location.conf", &tc)
    }
}
```

`generate_site_config` now calls `upstream_parts` and `php_location` (with `ctx.server_name`) and inserts `php_location` instead of the old `php_pass`/param list. `generate_default_site_config`:

```rust
    fn generate_default_site_config(
        &self,
        home: &Path,
        listen: std::net::SocketAddr,
        php_upstream: Option<&PhpUpstream>,
    ) -> Result<GeneratedFile, ConfError> {
        let home_str = to_config_path(home)?;
        let docroot = to_config_path(&home.join("www"))?;
        let php_location = match php_upstream {
            // `default` is a fixed, safe token: it names the custom-config
            // directory for the catch-all and the Windows upstream block.
            Some(up) => {
                let (_, pass) = Self::upstream_parts(up, "php_default")?;
                Self::php_location(&home_str, "default", &pass)?
            }
            None => String::new(),
        };
        let mut tc = tera::Context::new();
        tc.insert("custom_sites_dir", &format!("{home_str}/config/custom/sites"));
        tc.insert("listen_addr", &listen.to_string());
        tc.insert("docroot", &docroot);
        tc.insert("php_location", &php_location);
        let contents = render("nginx/default-site.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(home).join("sites").join("00-default_server.conf"),
            contents,
        })
    }
```

Note: the catch-all's `upstream{}` block is deliberately dropped (`_`) — on the unix path it is always empty, and the Windows pool manager is a later phase that will revisit the catch-all.

Update `validate()` to call `self.generate_main_config(&ctx.home)`.

- [ ] **Step 9: Update the php-fpm pool adapter**

In `crates/openvhost-conf/src/phpruntime.rs`, change the trait method and impl to:

```rust
    fn generate_pool_config(
        &self,
        home: &Path,
        major: &str,
        upstream: &PhpUpstream,
    ) -> Result<Option<GeneratedFile>, ConfError>;
```

The body is the current one with `ctx.home` → `home`, `ctx.php_major` → `major`, `ctx.php_upstream` → `upstream`. Update the adapter's own `validate()` and its unit tests to pass the three values out of the ctx they already build.

- [ ] **Step 10: Run the tests**

Run: `cargo test -p openvhost-conf`
Expected: PASS, including the pre-existing `site_config_tcp_upstream_seam` and `generation_is_deterministic`.

- [ ] **Step 11: Prove it against real nginx**

Run: `cargo test -p openvhost-conf --test validate_live`
Expected: PASS — the generated set still passes `nginx -t` with the new directives. If nginx rejects `location ~ [^/]\.php(/|$)`, the regex needs escaping fixes in the template, not a weakened guard.

- [ ] **Step 12: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
git add crates/openvhost-conf
git commit -s -m "feat(conf): generate vhosts that can serve a real site

Serve static files as files, route unknown paths through the front
controller, and follow the request in SCRIPT_FILENAME. Declare MIME
types so stylesheets are not labelled octet-stream. Add the catch-all
default_server. The PHP location lives in one shared template, guarded
by try_files =404 so an uploaded file cannot be executed via path info.

generate_main_config and generate_pool_config now take only what they
read, so a non-site file no longer needs a fabricated site context."
```

---

## Task 2: probe the installed php-fpm version

The pool path, the render context and the "is this version installed" check all need the major of the php-fpm on disk. `probe_nginx_version` documents in its own doc comment that it must not be widened to cover php-fpm — php-fpm writes to stdout in a different shape.

**Files:**
- Modify: `crates/openvhost-conf/src/inspect.rs`
- Modify: `crates/openvhost-conf/src/lib.rs`
- Test: `crates/openvhost-conf/src/inspect.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub async fn probe_php_fpm_version(bin: &Path) -> Option<String>` returning `major.minor` (e.g. `"8.4"`), `None` on any failure.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/openvhost-conf/src/inspect.rs`:

```rust
#[test]
fn parses_the_php_fpm_banner_down_to_major_minor() {
    let out = "PHP 8.4.23 (fpm-fcgi) (built: Jul 10 2026 10:11:12)\n\
               Copyright (c) The PHP Group\n";
    assert_eq!(parse_php_version(out), Some("8.4".to_string()));
}

#[test]
fn rejects_banners_that_are_not_php() {
    // nginx's banner must never satisfy this parser — the two probes are
    // separate on purpose.
    assert_eq!(parse_php_version("nginx version: nginx/1.27.3"), None);
    assert_eq!(parse_php_version(""), None);
    assert_eq!(parse_php_version("PHP notaversion"), None);
    assert_eq!(parse_php_version("PHP 8"), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-conf parse_php`
Expected: FAIL — `parse_php_version` not found.

- [ ] **Step 3: Implement**

Add to `crates/openvhost-conf/src/inspect.rs`:

```rust
/// The installed php-fpm's `major.minor` (`8.4` from
/// `PHP 8.4.23 (fpm-fcgi) ...`), or `None` for any failure.
///
/// Separate from [`probe_nginx_version`] by contract: php-fpm writes its
/// banner to STDOUT in a `PHP <version>` shape, where nginx writes
/// `nginx version: nginx/<version>` to STDERR. Neither parser can read the
/// other's output.
///
/// No `-e` equivalent is needed: php-fpm's `-v` writes nowhere but stdout.
pub async fn probe_php_fpm_version(bin: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-v");
    let out = run_bounded(&mut cmd).await.ok()?;
    parse_php_version(&String::from_utf8_lossy(&out.stdout))
}

/// Line-by-line so a preceding warning cannot consume the banner — the same
/// bug [`parse_version`] documents for nginx.
fn parse_php_version(stdout: &str) -> Option<String> {
    stdout.lines().find_map(parse_php_version_line)
}

fn parse_php_version_line(line: &str) -> Option<String> {
    let token = line.strip_prefix("PHP ")?.split_whitespace().next()?;
    let mut parts = token.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major) || !digits(minor) {
        return None;
    }
    Some(format!("{major}.{minor}"))
}
```

Export it from `crates/openvhost-conf/src/lib.rs`:

```rust
pub use inspect::{PROBE_TIMEOUT, probe_nginx_version, probe_php_fpm_version, validate_live};
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p openvhost-conf parse_php`
Expected: PASS (4 assertions in 2 tests).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test -p openvhost-conf
git add crates/openvhost-conf
git commit -s -m "feat(conf): probe the installed php-fpm version"
```

---

## Task 3: render the whole config set from sites

Pure function, no IO. This is where the "block on a missing PHP version" rule lives, and it runs before anything else can touch the disk.

**Files:**
- Modify: `crates/openvhost-core/Cargo.toml`
- Create: `crates/openvhost-core/src/site/apply/mod.rs`
- Create: `crates/openvhost-core/src/site/apply/error.rs`
- Modify: `crates/openvhost-core/src/site/mod.rs`
- Modify: `crates/openvhost-core/src/lib.rs`
- Test: `crates/openvhost-core/src/site/apply/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 1's adapter signatures.
- Produces:
  ```rust
  pub const MAX_SOCKET_PATH_BYTES: usize = 103;
  pub const LISTEN_PORT: u16 = 8080;

  pub struct PhpRuntime { pub major: String, pub fpm_bin: PathBuf }
  pub struct InstalledRuntimes { pub nginx_bin: PathBuf, pub php: Vec<PhpRuntime> }
  pub struct ApplyInput { pub home: PathBuf, pub sites: Vec<Site>, pub runtimes: InstalledRuntimes }

  pub fn render_set(input: &ApplyInput) -> Result<Vec<GeneratedFile>, ApplyError>;
  pub fn listen_addr() -> SocketAddr;                  // 127.0.0.1:LISTEN_PORT
  pub fn socket_path(home: &Path, major: &str) -> Result<PathBuf, ApplyError>;

  pub enum ApplyError {
      MissingRuntime { site: String, requested: String, available: Vec<String> },
      Conf(ConfError),
      Io { op: &'static str, path: PathBuf, source: std::io::Error },
      ValidationFailed { stderr: String },
      RollbackFailed { original: Box<ApplyError>, rollback: Box<ApplyError>, stranded: Vec<PathBuf> },
      Core(CoreError),
  }
  ```
  Naming note vs the spec: the variant wrapping `ConfError` is `Conf`, not `Render` — it also carries validator-spawn failures, so `Render` would mislead.

- [ ] **Step 1: Add the dependencies**

In `crates/openvhost-core/Cargo.toml` under `[dependencies]`:

```toml
openvhost-conf = { path = "../openvhost-conf" }
similar = "2"
```

`similar` is MIT, GPL-3.0-compatible. `openvhost-conf` depends on neither core nor tauri, so no cycle and no tauri edge.

- [ ] **Step 2: Write the failing tests**

Create `crates/openvhost-core/src/site/apply/mod.rs` with only the test module for now:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn site(name: &str, domain: &str, php: &str, enabled: bool) -> Site {
        Site {
            id: SiteId::new(),
            name: SiteName::parse(name).unwrap(),
            domain: Domain::parse(domain).unwrap(),
            docroot: Docroot::parse("/tmp/projects/app").unwrap(),
            web_server: WebServer::parse("nginx").unwrap(),
            php_version: PhpVersion::parse(php).unwrap(),
            enabled,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn runtimes(majors: &[&str]) -> InstalledRuntimes {
        InstalledRuntimes {
            nginx_bin: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
            php: majors
                .iter()
                .map(|m| PhpRuntime {
                    major: (*m).to_string(),
                    fpm_bin: PathBuf::from(format!("/opt/homebrew/opt/php@{m}/sbin/php-fpm")),
                })
                .collect(),
        }
    }

    fn input(sites: Vec<Site>, majors: &[&str]) -> ApplyInput {
        ApplyInput {
            home: PathBuf::from("/tmp/ovh"),
            sites,
            runtimes: runtimes(majors),
        }
    }

    #[test]
    fn renders_main_catch_all_site_and_pool() {
        let set = render_set(&input(vec![site("app", "app.localhost", "8.4", true)], &["8.4"])).unwrap();
        let paths: Vec<String> = set.iter().map(|f| f.path.display().to_string()).collect();
        assert_eq!(
            paths,
            vec![
                "/tmp/ovh/config/generated/nginx/nginx.conf",
                "/tmp/ovh/config/generated/nginx/sites/00-default_server.conf",
                "/tmp/ovh/config/generated/nginx/sites/app.localhost.conf",
                "/tmp/ovh/config/generated/php/8.4/php-fpm.conf",
            ]
        );
    }

    #[test]
    fn is_deterministic() {
        let i = input(vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        assert_eq!(render_set(&i).unwrap(), render_set(&i).unwrap());
    }

    #[test]
    fn a_disabled_site_is_not_rendered() {
        let set = render_set(&input(
            vec![
                site("app", "app.localhost", "8.4", true),
                site("old", "old.localhost", "8.4", false),
            ],
            &["8.4"],
        ))
        .unwrap();
        assert!(set.iter().any(|f| f.path.ends_with("app.localhost.conf")));
        assert!(!set.iter().any(|f| f.path.ends_with("old.localhost.conf")));
    }

    #[test]
    fn one_pool_per_installed_major_regardless_of_site_count() {
        let set = render_set(&input(
            vec![
                site("a", "a.localhost", "8.4", true),
                site("b", "b.localhost", "8.4", true),
                site("c", "c.localhost", "8.4", true),
            ],
            &["8.4"],
        ))
        .unwrap();
        let pools = set.iter().filter(|f| f.path.ends_with("php-fpm.conf")).count();
        assert_eq!(pools, 1);
    }

    #[test]
    fn pools_are_rendered_for_installed_majors_nobody_uses() {
        // The service set follows what is installed, not what sites ask for.
        let set = render_set(&input(vec![site("a", "a.localhost", "8.4", true)], &["8.3", "8.4"])).unwrap();
        assert!(set.iter().any(|f| f.path.ends_with("php/8.3/php-fpm.conf")));
        assert!(set.iter().any(|f| f.path.ends_with("php/8.4/php-fpm.conf")));
    }

    #[test]
    fn a_site_wanting_an_uninstalled_version_blocks_the_whole_apply() {
        let err = render_set(&input(
            vec![
                site("app", "app.localhost", "8.4", true),
                site("legacy", "legacy.localhost", "7.4", true),
            ],
            &["8.4"],
        ))
        .unwrap_err();
        match err {
            ApplyError::MissingRuntime { site, requested, available } => {
                assert_eq!(site, "legacy");
                assert_eq!(requested, "7.4");
                assert_eq!(available, vec!["8.4".to_string()]);
            }
            other => panic!("expected MissingRuntime, got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_site_never_blocks_on_a_missing_version() {
        let set = render_set(&input(
            vec![site("legacy", "legacy.localhost", "7.4", false)],
            &["8.4"],
        ));
        assert!(set.is_ok());
    }

    #[test]
    fn each_site_points_at_the_pool_socket_for_its_own_version() {
        let set = render_set(&input(
            vec![
                site("a", "a.localhost", "8.3", true),
                site("b", "b.localhost", "8.4", true),
            ],
            &["8.3", "8.4"],
        ))
        .unwrap();
        let a = set.iter().find(|f| f.path.ends_with("a.localhost.conf")).unwrap();
        let b = set.iter().find(|f| f.path.ends_with("b.localhost.conf")).unwrap();
        assert!(a.contents.contains("unix:/tmp/ovh/run/php-fpm-8.3.sock"));
        assert!(b.contents.contains("unix:/tmp/ovh/run/php-fpm-8.4.sock"));
    }

    #[test]
    fn a_home_too_deep_for_the_socket_is_refused_before_anything_renders() {
        let deep = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));
        let err = render_set(&ApplyInput {
            home: deep,
            sites: vec![site("app", "app.localhost", "8.4", true)],
            runtimes: runtimes(&["8.4"]),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::Core(CoreError::SocketPathTooLong { .. })
        ));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-core apply`
Expected: FAIL — the module does not compile; nothing in `super::*` exists.

- [ ] **Step 4: Write the error type**

Create `crates/openvhost-core/src/site/apply/error.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors for the site-apply pipeline.

use std::path::PathBuf;

use openvhost_conf::ConfError;

use crate::CoreError;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// An enabled site asks for a PHP major that is not installed. Raised
    /// before any file is touched: a config claiming 8.3 while served by 8.4
    /// is a lie the user debugs the hard way.
    #[error("site {site} needs PHP {requested}, which is not installed (installed: {})", available.join(", "))]
    MissingRuntime {
        site: String,
        requested: String,
        available: Vec<String>,
    },
    /// Generation or validator launch failed.
    #[error("config: {0}")]
    Conf(#[from] ConfError),
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// `nginx -t` rejected the generated set. The tree has been rolled back.
    #[error("the generated config was rejected by the web server:\n{stderr}")]
    ValidationFailed { stderr: String },
    /// Both the apply and its rollback failed. The generated tree now matches
    /// NEITHER the old nor the new configuration; `stranded` names the files
    /// that could not be restored. Never collapse this into a generic
    /// failure — it is the only signal the user gets that the tree is mixed.
    #[error("apply failed ({original}) AND rollback failed ({rollback}); these files were not restored: {}",
        stranded.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    RollbackFailed {
        original: Box<ApplyError>,
        rollback: Box<ApplyError>,
        stranded: Vec<PathBuf>,
    },
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// What a rollback managed to do. Rollback continues past a failure — restoring
/// four of five files beats abandoning at the first error — so it reports the
/// first error together with every path it could not restore.
#[derive(Debug)]
pub struct RollbackReport {
    pub first_error: ApplyError,
    pub stranded: Vec<PathBuf>,
}
```

- [ ] **Step 5: Implement `render_set`**

Put this above the test module in `crates/openvhost-core/src/site/apply/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Turn the enabled sites into the complete generated config set, then plan,
//! commit and validate it. See
//! docs/superpowers/specs/2026-07-27-p1-site-apply-design.md.

mod commit;
mod error;
mod plan;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

use openvhost_conf::{
    GeneratedFile, NginxAdapter, PhpFpmRuntime, PhpRuntimeAdapter, PhpUpstream, RenderCtx,
    WebServerAdapter,
};

pub use commit::{ConfigValidator, NginxValidator, apply, commit, rollback};
pub use error::{ApplyError, RollbackReport};
pub use plan::{ApplyPlan, ChangeKind, FileChange, plan};

use crate::site::model::{
    Docroot, Domain, PhpVersion, Site, SiteId, SiteName, WebServer,
};
use crate::CoreError;

/// Darwin's `sun_path` is 104 bytes including the NUL. php-fpm does not reject
/// a longer path — it warns, truncates, binds the wrong path, and nginx 502s
/// forever. Refuse early instead.
pub const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Every site is name-based virtual hosting on one port. Port 80 needs the
/// privileged helper (Phase 3).
pub const LISTEN_PORT: u16 = 8080;

pub fn listen_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, LISTEN_PORT))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpRuntime {
    pub major: String,
    pub fpm_bin: PathBuf,
}

/// What is installed on this machine. Passed in as data rather than probed
/// here, so every test constructs it by hand and no test depends on what the
/// machine running it happens to have. `php` is ordered: the first entry is
/// the catch-all's runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntimes {
    pub nginx_bin: PathBuf,
    pub php: Vec<PhpRuntime>,
}

#[derive(Debug, Clone)]
pub struct ApplyInput {
    pub home: PathBuf,
    /// ALL sites; `render_set` filters on `enabled` itself.
    pub sites: Vec<Site>,
    pub runtimes: InstalledRuntimes,
}

/// The php-fpm socket for one major, guarded against the `sun_path` ceiling.
pub fn socket_path(home: &Path, major: &str) -> Result<PathBuf, ApplyError> {
    let p = home.join("run").join(format!("php-fpm-{major}.sock"));
    let len = p.as_os_str().as_encoded_bytes().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(ApplyError::Core(CoreError::SocketPathTooLong { path: p, len }));
    }
    Ok(p)
}

/// nginx `upstream{}` block name: `[a-z0-9_]`, and genuinely unique per site.
///
/// Derived from the site's UUID rather than its domain because a charset
/// substitution on the domain is not injective — `a-b.example` and
/// `a.b-example` would both reduce to `php_a_b_example`, and on the Windows
/// path that means one nginx context defining the same upstream block twice
/// with different backends. The id is the table's primary key, so uniqueness
/// is structural.
fn upstream_name(id: &SiteId) -> String {
    format!("php_{}", id.as_str().replace('-', ""))
}

/// The complete desired config set, sorted by path so the output is stable.
/// Pure: no filesystem access at all.
pub fn render_set(input: &ApplyInput) -> Result<Vec<GeneratedFile>, ApplyError> {
    let nginx = NginxAdapter;
    let fpm = PhpFpmRuntime;
    let listen = listen_addr();

    // Every check that can fail without touching the disk runs first (spec §4.1).
    let available: Vec<String> = input.runtimes.php.iter().map(|r| r.major.clone()).collect();
    for site in input.sites.iter().filter(|s| s.enabled) {
        if !available.iter().any(|m| m == site.php_version.as_str()) {
            return Err(ApplyError::MissingRuntime {
                site: site.name.as_str().to_string(),
                requested: site.php_version.as_str().to_string(),
                available,
            });
        }
    }

    let mut out = vec![nginx.generate_main_config(&input.home)?];

    let default_upstream = match input.runtimes.php.first() {
        Some(rt) => Some(PhpUpstream::UnixSocket(socket_path(&input.home, &rt.major)?)),
        None => None,
    };
    out.push(nginx.generate_default_site_config(&input.home, listen, default_upstream.as_ref())?);

    for site in input.sites.iter().filter(|s| s.enabled) {
        let major = site.php_version.as_str();
        let ctx = RenderCtx::new(
            input.home.clone(),
            site.domain.as_str(),
            PathBuf::from(site.docroot.as_str()),
            listen,
            major,
            PhpUpstream::UnixSocket(socket_path(&input.home, major)?),
            upstream_name(&site.id),
        )?;
        out.push(nginx.generate_site_config(&ctx)?);
    }

    for rt in &input.runtimes.php {
        let upstream = PhpUpstream::UnixSocket(socket_path(&input.home, &rt.major)?);
        if let Some(f) = fpm.generate_pool_config(&input.home, &rt.major, &upstream)? {
            out.push(f);
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
```

Add to `crates/openvhost-core/src/site/mod.rs`:

```rust
pub mod apply;
```

Add to `crates/openvhost-core/src/lib.rs`, next to the existing site re-exports:

```rust
pub use site::apply::{
    ApplyError, ApplyInput, ApplyPlan, ChangeKind, FileChange, InstalledRuntimes, PhpRuntime,
    apply, plan, render_set,
};
```

(The `plan`/`commit` modules land in Tasks 4 and 5; until then, comment out the `mod plan;`/`mod commit;` lines and the re-exports that name them, and restore them in those tasks.)

- [ ] **Step 6: Run the tests**

Run: `cargo test -p openvhost-core apply`
Expected: PASS — 9 tests.

- [ ] **Step 7: Run the license gate for the new dependency**

Run: `cargo deny check licenses`
Expected: PASS. `similar` is MIT; the repo's `deny.toml` is the authority and a
GPL-incompatible or unknown-license addition is rejected here rather than in
review. If `cargo-deny` is not installed: `cargo install cargo-deny`.

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
git add crates/openvhost-core
git commit -s -m "feat(core): render the nginx + php-fpm config set from sites

Pure rendering: enabled sites become vhosts, installed runtimes become
pools, and a site asking for an uninstalled PHP major blocks the whole
apply before any file is touched."
```

---

## Task 4: plan — diff the desired set against the disk

Read-only. This is what the pending-changes banner calls, so it must not spawn a process.

**Files:**
- Create: `crates/openvhost-core/src/site/apply/plan.rs`
- Modify: `crates/openvhost-core/src/site/apply/mod.rs` (uncomment `mod plan;` and its re-exports)
- Test: `crates/openvhost-core/src/site/apply/plan.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `render_set`, `ApplyInput`, `ApplyError` from Task 3.
- Produces:
  ```rust
  pub enum ChangeKind { Added, Modified, Removed }
  pub struct FileChange { pub path: PathBuf, pub kind: ChangeKind, pub before: Option<String>, pub after: Option<String>, pub diff: String }
  pub struct ApplyPlan { pub gen_root: PathBuf, pub main_conf: PathBuf, pub changes: Vec<FileChange> }
  pub fn plan(input: &ApplyInput) -> Result<ApplyPlan, ApplyError>;
  ```
  A file whose contents already match is **not** a change. `plan.changes.is_empty()` is exactly "nothing to apply".

- [ ] **Step 1: Write the failing tests**

Create `crates/openvhost-core/src/site/apply/plan.rs` with the test module:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::apply::tests_support::{input_with_home, site};
    use crate::site::model::Docroot;

    #[test]
    fn everything_is_added_against_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        let p = plan(&i).unwrap();
        assert_eq!(p.changes.len(), 4);
        assert!(p.changes.iter().all(|c| c.kind == ChangeKind::Added));
        assert_eq!(p.main_conf, home.path().join("config/generated/nginx/nginx.conf"));
    }

    #[test]
    fn an_unchanged_tree_plans_nothing() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        crate::site::apply::commit(&plan(&i).unwrap()).unwrap();
        let second = plan(&i).unwrap();
        assert!(second.changes.is_empty(), "re-planning an applied tree must be a no-op");
    }

    #[test]
    fn editing_a_site_shows_exactly_one_modified_file() {
        let home = tempfile::tempdir().unwrap();
        let before = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        crate::site::apply::commit(&plan(&before).unwrap()).unwrap();

        let mut moved = site("app", "app.localhost", "8.4", true);
        moved.docroot = Docroot::parse("/tmp/projects/moved").unwrap();
        let after = input_with_home(home.path(), vec![moved], &["8.4"]);

        let p = plan(&after).unwrap();
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].kind, ChangeKind::Modified);
        assert!(p.changes[0].path.ends_with("app.localhost.conf"));
        assert!(p.changes[0].diff.contains("-    root \"/tmp/projects/app\";"));
        assert!(p.changes[0].diff.contains("+    root \"/tmp/projects/moved\";"));
    }

    #[test]
    fn disabling_a_site_removes_its_file() {
        let home = tempfile::tempdir().unwrap();
        let on = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        crate::site::apply::commit(&plan(&on).unwrap()).unwrap();

        let off = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", false)], &["8.4"]);
        let p = plan(&off).unwrap();
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].kind, ChangeKind::Removed);
        assert!(p.changes[0].path.ends_with("app.localhost.conf"));
        assert!(p.changes[0].after.is_none());
        assert!(p.changes[0].before.is_some());
    }

    #[test]
    fn custom_config_is_invisible_to_planning() {
        let home = tempfile::tempdir().unwrap();
        let custom = home.path().join("config/custom/sites");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("mine.conf"), "# hand written\n").unwrap();

        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        let p = plan(&i).unwrap();
        assert!(
            p.changes.iter().all(|c| !c.path.starts_with(home.path().join("config/custom"))),
            "planning must never name a file under config/custom"
        );
    }

    #[test]
    fn a_stray_file_in_the_generated_tree_is_removed() {
        let home = tempfile::tempdir().unwrap();
        let sites_dir = home.path().join("config/generated/nginx/sites");
        std::fs::create_dir_all(&sites_dir).unwrap();
        std::fs::write(sites_dir.join("ghost.localhost.conf"), "# left over\n").unwrap();

        let i = input_with_home(home.path(), vec![], &["8.4"]);
        let p = plan(&i).unwrap();
        assert!(p.changes.iter().any(|c| c.kind == ChangeKind::Removed
            && c.path.ends_with("ghost.localhost.conf")));
    }
}
```

Shared test helpers: move `site`, `runtimes` and a new `input_with_home` from Task 3's test module into a `#[cfg(test)] pub(crate) mod tests_support;` inside `crates/openvhost-core/src/site/apply/mod.rs`, and have Task 3's tests import from it too. `input_with_home(home: &Path, sites: Vec<Site>, majors: &[&str]) -> ApplyInput` is `input` with the home replaced. Everything the helper module names (`Site`, `SiteId`, `SiteName`, `Domain`, `Docroot`, `WebServer`, `PhpVersion`) is already imported by `mod.rs`; add `tempfile` to `[dev-dependencies]` only if it is missing — it is already there.

Note on naming: `mod commit` and the re-exported `fn commit` coexist because Rust keeps modules and functions in different namespaces, so `crate::site::apply::commit(&p)` in expression position is unambiguously the function. This is the same shape as the existing `mod error; pub use error::...`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core apply::plan`
Expected: FAIL — `plan` not found.

- [ ] **Step 3: Implement**

Write into `crates/openvhost-core/src/site/apply/plan.rs`, above the tests:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Diff the desired config set against what is on disk. Read-only: this is
//! what the pending-changes banner calls, so it must not spawn anything.

use std::path::{Path, PathBuf};

use openvhost_conf::GeneratedFile;

use super::{ApplyError, ApplyInput, render_set};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
    /// The contents currently on disk. `None` only for `Added`.
    pub before: Option<String>,
    /// The contents to be written. `None` only for `Removed`.
    pub after: Option<String>,
    /// Unified diff, rendered once here so the CLI and the UI cannot disagree.
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPlan {
    pub gen_root: PathBuf,
    pub main_conf: PathBuf,
    /// Sorted by path. EMPTY means the disk already matches the sites — that
    /// is exactly the condition the banner hides on.
    pub changes: Vec<FileChange>,
}

fn read_if_exists(path: &Path) -> Result<Option<String>, ApplyError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ApplyError::Io {
            op: "read",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn unified(path: &Path, before: &str, after: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .header(&format!("a/{}", path.display()), &format!("b/{}", path.display()))
        .to_string()
}

/// Generated files this pipeline owns, and therefore may delete: the site
/// configs and the per-major pools. `config/custom/` is never listed, so it can
/// never be planned for removal.
fn owned_files(gen_root: &Path) -> Result<Vec<PathBuf>, ApplyError> {
    let mut out = Vec::new();
    let sites_dir = gen_root.join("nginx/sites");
    for entry in read_dir_or_empty(&sites_dir)? {
        if entry.extension().is_some_and(|e| e == "conf") {
            out.push(entry);
        }
    }
    let php_dir = gen_root.join("php");
    for major_dir in read_dir_or_empty(&php_dir)? {
        let pool = major_dir.join("php-fpm.conf");
        if pool.is_file() {
            out.push(pool);
        }
    }
    out.sort();
    Ok(out)
}

/// Directory entries, or an empty list when the directory does not exist —
/// a home that has never been applied is not an error condition.
fn read_dir_or_empty(dir: &Path) -> Result<Vec<PathBuf>, ApplyError> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ApplyError::Io {
                op: "read_dir",
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|source| ApplyError::Io {
            op: "read_dir",
            path: dir.to_path_buf(),
            source,
        })?;
        out.push(e.path());
    }
    Ok(out)
}

pub fn plan(input: &ApplyInput) -> Result<ApplyPlan, ApplyError> {
    let desired: Vec<GeneratedFile> = render_set(input)?;
    let gen_root = input.home.join("config/generated");
    let mut changes = Vec::new();

    for f in &desired {
        let before = read_if_exists(&f.path)?;
        match &before {
            Some(b) if *b == f.contents => continue,
            Some(b) => changes.push(FileChange {
                diff: unified(&f.path, b, &f.contents),
                path: f.path.clone(),
                kind: ChangeKind::Modified,
                before: before.clone(),
                after: Some(f.contents.clone()),
            }),
            None => changes.push(FileChange {
                diff: unified(&f.path, "", &f.contents),
                path: f.path.clone(),
                kind: ChangeKind::Added,
                before: None,
                after: Some(f.contents.clone()),
            }),
        }
    }

    let desired_paths: std::collections::BTreeSet<&Path> =
        desired.iter().map(|f| f.path.as_path()).collect();
    for stale in owned_files(&gen_root)? {
        if desired_paths.contains(stale.as_path()) {
            continue;
        }
        let before = read_if_exists(&stale)?;
        let before_text = before.clone().unwrap_or_default();
        changes.push(FileChange {
            diff: unified(&stale, &before_text, ""),
            path: stale,
            kind: ChangeKind::Removed,
            before,
            after: None,
        });
    }

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ApplyPlan {
        main_conf: gen_root.join("nginx/nginx.conf"),
        gen_root,
        changes,
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p openvhost-core apply`
Expected: PASS. (The plan tests call `commit`, delivered in Task 5 — if executing tasks strictly in order, implement Task 5's `commit`/`rollback` first and run the two suites together. Task 5's steps assume this.)

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
git add crates/openvhost-core
git commit -s -m "feat(core): plan a site apply as a unified diff

Read-only: an already-applied tree plans zero changes, a disabled site
plans a removal, and config/custom is never named."
```

---

## Task 5: commit, validate, roll back

**Files:**
- Create: `crates/openvhost-core/src/site/apply/commit.rs`
- Modify: `crates/openvhost-core/src/site/apply/mod.rs` (uncomment `mod commit;` and its re-exports)
- Test: `crates/openvhost-core/src/site/apply/commit.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ApplyPlan`, `FileChange`, `ChangeKind`, `ApplyError`, `RollbackReport`.
- Produces:
  ```rust
  pub struct ApplyOutcome { pub applied: usize, pub validator_stderr: String }

  #[async_trait::async_trait]
  pub trait ConfigValidator: Send + Sync {
      async fn validate(&self, main_conf: &Path) -> Result<openvhost_conf::ValidationReport, ApplyError>;
  }
  pub struct NginxValidator { pub bin: PathBuf, pub err_log: PathBuf }

  pub fn commit(plan: &ApplyPlan) -> Result<(), ApplyError>;
  pub fn rollback(plan: &ApplyPlan) -> Result<(), RollbackReport>;
  pub async fn apply(plan: &ApplyPlan, validator: &dyn ConfigValidator) -> Result<ApplyOutcome, ApplyError>;
  ```
  `async-trait` must be added to `crates/openvhost-core/Cargo.toml` (`async-trait = "0.1"`).

- [ ] **Step 1: Write the failing tests**

In `crates/openvhost-core/src/site/apply/commit.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::apply::plan as make_plan;
    use crate::site::apply::tests_support::{input_with_home, site};
    use std::collections::BTreeMap;

    /// Every regular file under `root`, as path → contents. The whole point of
    /// the rollback test is a byte-for-byte comparison, so snapshot everything.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(s) = std::fs::read_to_string(&p) {
                    out.insert(p, s);
                }
            }
        }
        out
    }

    struct AlwaysRejects;
    #[async_trait::async_trait]
    impl ConfigValidator for AlwaysRejects {
        async fn validate(&self, _main: &Path) -> Result<openvhost_conf::ValidationReport, ApplyError> {
            Ok(openvhost_conf::ValidationReport {
                ok: false,
                stderr: "nginx: [emerg] simulated rejection".into(),
            })
        }
    }

    struct AlwaysAccepts;
    #[async_trait::async_trait]
    impl ConfigValidator for AlwaysAccepts {
        async fn validate(&self, _main: &Path) -> Result<openvhost_conf::ValidationReport, ApplyError> {
            Ok(openvhost_conf::ValidationReport { ok: true, stderr: String::new() })
        }
    }

    #[tokio::test]
    async fn a_green_validation_leaves_the_new_config_in_place() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        let p = make_plan(&i).unwrap();
        let out = apply(&p, &AlwaysAccepts).await.unwrap();
        assert_eq!(out.applied, 4);
        let conf = home.path().join("config/generated/nginx/sites/app.localhost.conf");
        assert!(std::fs::read_to_string(conf).unwrap().contains("server_name app.localhost;"));
    }

    #[tokio::test]
    async fn a_rejected_config_restores_the_tree_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        let first = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        apply(&make_plan(&first).unwrap(), &AlwaysAccepts).await.unwrap();
        let before = snapshot(home.path());

        // A second site, plus removing the first — exercises Added, Modified
        // and Removed in one rollback.
        let second = input_with_home(
            home.path(),
            vec![site("other", "other.localhost", "8.4", true)],
            &["8.4"],
        );
        let p = make_plan(&second).unwrap();
        let err = apply(&p, &AlwaysRejects).await.unwrap_err();
        assert!(matches!(err, ApplyError::ValidationFailed { .. }));

        assert_eq!(snapshot(home.path()), before, "rollback must restore every byte");
    }

    #[tokio::test]
    async fn the_validator_stderr_reaches_the_caller() {
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        let err = apply(&make_plan(&i).unwrap(), &AlwaysRejects).await.unwrap_err();
        match err {
            ApplyError::ValidationFailed { stderr } => {
                assert!(stderr.contains("simulated rejection"));
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn commit_writes_through_a_temp_file_in_the_same_directory() {
        // A rename across filesystems fails; a temp file in the target's own
        // directory is what makes the write atomic.
        let home = tempfile::tempdir().unwrap();
        let i = input_with_home(home.path(), vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        commit(&make_plan(&i).unwrap()).unwrap();
        let sites = home.path().join("config/generated/nginx/sites");
        let leftovers: Vec<_> = std::fs::read_dir(&sites)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".conf"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must not survive: {leftovers:?}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core apply::commit`
Expected: FAIL — `commit` not found.

- [ ] **Step 3: Implement**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Install a planned config set, validate the real files, and restore the
//! previous tree if the validator rejects them.

use std::path::{Path, PathBuf};

use super::{ApplyError, ApplyPlan, ChangeKind, RollbackReport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: usize,
    /// `nginx -t` writes to stderr even on success; kept so the UI can show it.
    pub validator_stderr: String,
}

#[async_trait::async_trait]
pub trait ConfigValidator: Send + Sync {
    async fn validate(&self, main_conf: &Path)
        -> Result<openvhost_conf::ValidationReport, ApplyError>;
}

/// The real validator. `-e <err_log>` is mandatory on every nginx invocation,
/// which `validate_live` handles.
pub struct NginxValidator {
    pub bin: PathBuf,
    pub err_log: PathBuf,
}

#[async_trait::async_trait]
impl ConfigValidator for NginxValidator {
    async fn validate(&self, main_conf: &Path)
        -> Result<openvhost_conf::ValidationReport, ApplyError> {
        Ok(openvhost_conf::validate_live(&self.bin, main_conf, &self.err_log).await?)
    }
}

/// Write via a temp file in the SAME directory, then rename: a rename is
/// atomic only within one filesystem.
fn atomic_write(path: &Path, contents: &str) -> Result<(), ApplyError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| ApplyError::Io {
        op: "create_dir_all",
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, contents).map_err(|source| ApplyError::Io {
        op: "write",
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| ApplyError::Io {
        op: "rename",
        path: path.to_path_buf(),
        source,
    })
}

fn remove_if_exists(path: &Path) -> Result<(), ApplyError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ApplyError::Io {
            op: "remove",
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub fn commit(plan: &ApplyPlan) -> Result<(), ApplyError> {
    for c in &plan.changes {
        match c.kind {
            ChangeKind::Added | ChangeKind::Modified => {
                atomic_write(&c.path, c.after.as_deref().unwrap_or_default())?;
            }
            ChangeKind::Removed => remove_if_exists(&c.path)?,
        }
    }
    Ok(())
}

/// Undo a commit. Continues past a failure — restoring four files out of five
/// beats abandoning at the first error — and reports everything it could not
/// put back.
pub fn rollback(plan: &ApplyPlan) -> Result<(), RollbackReport> {
    let mut first_error: Option<ApplyError> = None;
    let mut stranded = Vec::new();
    for c in &plan.changes {
        let r = match c.kind {
            ChangeKind::Added => remove_if_exists(&c.path),
            ChangeKind::Modified | ChangeKind::Removed => {
                atomic_write(&c.path, c.before.as_deref().unwrap_or_default())
            }
        };
        if let Err(e) = r {
            stranded.push(c.path.clone());
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    match first_error {
        None => Ok(()),
        Some(first_error) => Err(RollbackReport { first_error, stranded }),
    }
}

fn with_rollback(plan: &ApplyPlan, original: ApplyError) -> ApplyError {
    match rollback(plan) {
        Ok(()) => original,
        Err(report) => ApplyError::RollbackFailed {
            original: Box::new(original),
            rollback: Box::new(report.first_error),
            stranded: report.stranded,
        },
    }
}

/// Install, validate, and restore on rejection.
///
/// Writing before validating is safe because a running nginx holds its config
/// in memory: nothing on disk takes effect until the caller restarts it, which
/// it only does after this returns `Ok`. The payoff is that the validator sees
/// the exact files that will run.
pub async fn apply(
    plan: &ApplyPlan,
    validator: &dyn ConfigValidator,
) -> Result<ApplyOutcome, ApplyError> {
    if let Err(e) = commit(plan) {
        return Err(with_rollback(plan, e));
    }
    match validator.validate(&plan.main_conf).await {
        Ok(r) if r.ok => Ok(ApplyOutcome {
            applied: plan.changes.len(),
            validator_stderr: r.stderr,
        }),
        Ok(r) => Err(with_rollback(plan, ApplyError::ValidationFailed { stderr: r.stderr })),
        Err(e) => Err(with_rollback(plan, e)),
    }
}
```

Add `async-trait = "0.1"` to `crates/openvhost-core/Cargo.toml` `[dependencies]`, and add `ApplyOutcome` to the `pub use commit::{...}` list in `mod.rs` and the `lib.rs` re-exports.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p openvhost-core apply`
Expected: PASS — Task 4's and Task 5's suites together.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings && cargo test --workspace
git add crates/openvhost-core
git commit -s -m "feat(core): commit, validate and roll back a site apply

Atomic per-file installs, then nginx -t against the real files. A
rejection restores the tree byte-for-byte; a rollback that itself fails
reports the stranded paths rather than hiding a mixed tree."
```

---

## Task 6: retire the demo stack as the config source

**Files:**
- Modify: `crates/openvhost-core/src/platform/macos/demo_stack.rs`
- Modify: `crates/openvhost-core/tests/macos_stack.rs`
- Modify: `apps/desktop/src-tauri/src/stack.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src/lib/components/WebServerRow.svelte`
- Modify: `apps/desktop/src/lib/components/webserver.panel.test.ts`

**Interfaces:**
- Consumes: `InstalledRuntimes`, `PhpRuntime`, `socket_path`, `LISTEN_PORT` (Task 3); `probe_php_fpm_version` (Task 2).
- Produces:
  ```rust
  // openvhost_core::platform::macos::demo_stack
  pub fn provision_home(home: &Path) -> Result<(), CoreError>;   // replaces provision_macos_demo_stack

  // apps/desktop/src-tauri/src/stack.rs
  pub struct StackPaths { pub home: PathBuf, pub nginx_bin: PathBuf, pub nginx_conf: PathBuf }
  pub struct MacosStack { pub specs: Vec<ServiceSpec>, pub paths: Option<StackPaths>, pub runtimes: Option<InstalledRuntimes> }
  pub fn macos_stack() -> MacosStack;
  ```
  Service ids become `nginx` and `php-fpm-<major>`.

- [ ] **Step 1: Rewrite the provisioning test**

Replace the config-content assertions in `crates/openvhost-core/tests/macos_stack.rs` with the new contract:

```rust
#[test]
fn provisioning_creates_the_directories_and_seeds_the_welcome_page() {
    let home = short_home();
    provision_home(home.path()).unwrap();
    for dir in ["www", "run", "run/nginx", "logs"] {
        assert!(home.path().join(dir).is_dir(), "{dir} must exist");
    }
    let index = home.path().join("www/index.php");
    assert!(index.is_file());
    assert!(std::fs::read_to_string(index).unwrap().contains("phpinfo"));
}

#[test]
fn provisioning_no_longer_writes_any_config() {
    // The generated tree is the only config source now; a stale hand-written
    // conf/ would be a second source of truth nobody updates.
    let home = short_home();
    provision_home(home.path()).unwrap();
    assert!(!home.path().join("conf/nginx.conf").exists());
    assert!(!home.path().join("conf/php-fpm.conf").exists());
}

#[test]
fn provisioning_is_idempotent() {
    let home = short_home();
    provision_home(home.path()).unwrap();
    provision_home(home.path()).unwrap();
    assert!(home.path().join("www/index.php").is_file());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core --test macos_stack`
Expected: FAIL — `provision_home` not found.

- [ ] **Step 3: Strip the config writing**

In `crates/openvhost-core/src/platform/macos/demo_stack.rs`:

- Delete the `NGINX_CONF` and `FPM_CONF` constants, the `StackPaths` struct, and the socket-length guard together with `MAX_SOCKET_PATH_BYTES` (the constant now lives in `site::apply`, which is portable — `apply` is not macOS-only code and cannot import from a `#[cfg(target_os = "macos")]` module).
- Keep `INDEX_PHP`, `atomic_write`, `BrewStack` and `find_brew_binaries`.
- Replace `provision_macos_demo_stack` with:

```rust
/// Create the directories the generated config set expects and seed the
/// welcome page. Writes NO configuration: `site::apply` owns every generated
/// file now.
pub fn provision_home(home: &Path) -> Result<(), CoreError> {
    for dir in ["www", "run", "run/nginx", "logs"] {
        let d = home.join(dir);
        std::fs::create_dir_all(&d).map_err(|source| CoreError::ProvisionIo {
            op: "create_dir_all",
            path: d.clone(),
            source,
        })?;
    }
    atomic_write(&home.join("www/index.php"), INDEX_PHP)
}
```

Keep `CoreError::SocketPathTooLong` — `site::apply::socket_path` raises it.

- [ ] **Step 4: Repoint the supervised specs**

Rewrite `macos_stack()` in `apps/desktop/src-tauri/src/stack.rs`:

```rust
pub fn macos_stack() -> MacosStack {
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("stack: cannot resolve OPENVHOST_HOME, skipping service rows: {e}");
            return MacosStack { specs: vec![], paths: None, runtimes: None };
        }
    };
    if let Err(e) = provision_home(&home) {
        eprintln!("stack: provisioning failed (rows registered anyway): {e}");
    }
    let brew = find_brew_binaries().unwrap_or_else(fallback_brew);

    // Probing spawns a process, so it happens ONCE here and the result is
    // managed state. That is what lets `plan_site_apply` stay process-free.
    let major = tauri::async_runtime::block_on(openvhost_conf::probe_php_fpm_version(
        &brew.php_fpm,
    ));

    let nginx_conf = home.join("config/generated/nginx/nginx.conf");
    let php: Vec<PhpRuntime> = major
        .iter()
        .map(|m| PhpRuntime { major: m.clone(), fpm_bin: brew.php_fpm.clone() })
        .collect();

    let mut specs = Vec::new();
    for rt in &php {
        specs.push(ServiceSpec {
            id: format!("php-fpm-{}", rt.major),
            display_name: format!("PHP-FPM {}", rt.major),
            endpoint: Some(format!("run/php-fpm-{}.sock", rt.major)),
            spawn: SpawnSpec {
                program: rt.fpm_bin.clone(),
                args: vec![
                    OsString::from("-F"),
                    OsString::from("-O"),
                    OsString::from("-n"),
                    OsString::from("-y"),
                    home.join(format!("config/generated/php/{}/php-fpm.conf", rt.major))
                        .into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        });
    }
    specs.push(ServiceSpec {
        id: "nginx".into(),
        display_name: "nginx".into(),
        endpoint: Some(format!("http://127.0.0.1:{}", LISTEN_PORT)),
        spawn: SpawnSpec {
            program: brew.nginx.clone(),
            args: vec![
                OsString::from("-e"),
                home.join("logs/nginx.error.log").into_os_string(),
                OsString::from("-c"),
                nginx_conf.clone().into_os_string(),
            ],
            cwd: None,
            env: vec![],
        },
    });

    MacosStack {
        specs,
        paths: Some(StackPaths { home, nginx_bin: brew.nginx.clone(), nginx_conf }),
        runtimes: Some(InstalledRuntimes { nginx_bin: brew.nginx, php }),
    }
}
```

The config files these specs name may not exist yet on a home that has never been applied. That is deliberate: registration must not depend on an apply having happened, and Start then fails honestly with the missing path (the P0-3 spawn-failure contract) while the pending-changes banner tells the user what to do. When the php-fpm version cannot be probed, `php` is empty, no PHP row registers, and any site needing PHP is blocked by `MissingRuntime` with an empty `available` list.

Update the existing `reported_paths_match_the_registered_nginx_spec` test — it still applies verbatim to the nginx spec.

In `apps/desktop/src-tauri/src/lib.rs`, manage the runtimes alongside the paths:

```rust
app.manage(stack.runtimes);   // Option<InstalledRuntimes>
```

- [ ] **Step 5: Update the UI copy that names the old path**

`apps/desktop/src/lib/components/WebServerRow.svelte` explains that `provision_macos_demo_stack` rewrites `<home>/conf/nginx.conf`. Replace that with the generated path and the new owner, e.g.:

> Regenerated by Apply from your sites — edits to this file are lost. Add custom directives under `config/custom/` instead.

Update the matching expectations in `apps/desktop/src/lib/components/webserver.panel.test.ts`.

- [ ] **Step 6: Run everything**

```bash
cargo test --workspace
pnpm -C apps/desktop test
```
Expected: PASS.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings
git add crates/openvhost-core apps/desktop
git commit -s -m "refactor: make the generated tree the only config source

provision_home creates directories and seeds the welcome page; it no
longer writes nginx.conf or php-fpm.conf. The supervised specs point at
config/generated, and php-fpm rows are per installed major."
```

---

## Task 7: the IPC surface

**Merge-blocked: this task adds Tauri commands, so the branch needs a security-auditor APPROVE before merge (golden rule 2).**

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src/lib/ipc/bindings.ts` (regenerated)
- Modify: `apps/desktop/src/lib/ipc/index.ts`
- Test: `apps/desktop/src-tauri/src/commands.rs` (`#[cfg(test)] mod tests`), `apps/desktop/src/lib/ipc/ipc.test.ts`

**Interfaces:**
- Consumes: `plan`, `apply`, `NginxValidator`, `ApplyInput`, `InstalledRuntimes` (Tasks 3-5); `StackPaths`, managed `Option<InstalledRuntimes>` (Task 6).
- Produces:
  ```rust
  pub struct FileChangeDto { pub path: String, pub kind: String, pub diff: String }   // kind: "added" | "modified" | "removed"
  pub struct ApplyPlanDto { pub changes: Vec<FileChangeDto> }
  pub struct ApplyOutcomeDto { pub applied: u32, pub restarted: Vec<String>, pub not_started: Vec<String> }

  #[tauri::command] pub async fn plan_site_apply(...) -> Result<ApplyPlanDto, IpcError>;
  #[tauri::command] pub async fn apply_sites(...) -> Result<ApplyOutcomeDto, IpcError>;
  ```
  TS names (specta camel-cases): `planSiteApply()`, `applySites()`, `ApplyPlanDto.changes[].kind`, `ApplyOutcomeDto.notStarted`.

- [ ] **Step 1: Write the failing tests**

In `apps/desktop/src-tauri/src/commands.rs` tests:

```rust
#[test]
fn change_kind_maps_to_a_stable_wire_string() {
    // The dialog switches on these; a rename here silently breaks its badges.
    assert_eq!(change_kind_str(ChangeKind::Added), "added");
    assert_eq!(change_kind_str(ChangeKind::Modified), "modified");
    assert_eq!(change_kind_str(ChangeKind::Removed), "removed");
}

#[test]
fn a_missing_runtime_reaches_the_ui_naming_the_site_and_versions() {
    let e: IpcError = ApplyError::MissingRuntime {
        site: "legacy".into(),
        requested: "7.4".into(),
        available: vec!["8.4".into()],
    }
    .into();
    match e {
        IpcError::Core { message } => {
            assert!(message.contains("legacy"));
            assert!(message.contains("7.4"));
            assert!(message.contains("8.4"));
        }
        other => panic!("expected Core, got {other:?}"),
    }
}
```

In `apps/desktop/src/lib/ipc/ipc.test.ts`, following the existing pattern for a command wrapper, assert that `planSiteApply` returns `data` on success and throws the normalized `IpcError` on failure.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-desktop change_kind`
Expected: FAIL — `change_kind_str` not found.

- [ ] **Step 3: Implement the commands**

In `apps/desktop/src-tauri/src/commands.rs`:

```rust
use openvhost_core::{ApplyError, ApplyInput, ChangeKind, InstalledRuntimes};

impl From<ApplyError> for IpcError {
    fn from(e: ApplyError) -> Self {
        // Every variant's Display already names the site, the versions or the
        // stranded paths, so one arm is enough and none of that detail is lost.
        IpcError::Core { message: e.to_string() }
    }
}

fn change_kind_str(k: ChangeKind) -> &'static str {
    match k {
        ChangeKind::Added => "added",
        ChangeKind::Modified => "modified",
        ChangeKind::Removed => "removed",
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeDto {
    pub path: String,
    /// "added" | "modified" | "removed"
    pub kind: String,
    pub diff: String,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPlanDto {
    pub changes: Vec<FileChangeDto>,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOutcomeDto {
    /// `u32`, not `usize`: specta rejects pointer-sized ints (see lib.rs).
    pub applied: u32,
    pub restarted: Vec<String>,
    /// Services whose config changed but which were not running, so the new
    /// config takes effect the next time they start.
    pub not_started: Vec<String>,
}

/// Build the apply input from state.db plus the runtimes probed at startup.
async fn apply_input(
    db: &Db,
    runtimes: &Option<InstalledRuntimes>,
    paths: &Option<StackPaths>,
) -> Result<ApplyInput, IpcError> {
    let (Some(runtimes), Some(paths)) = (runtimes.as_ref(), paths.as_ref()) else {
        return Err(IpcError::Core {
            message: "no web server stack is configured for this platform".into(),
        });
    };
    let repo = SqliteSiteRepository::new(db);
    Ok(ApplyInput {
        home: paths.home.clone(),
        sites: repo.list().await?,
        runtimes: runtimes.clone(),
    })
}

/// What Apply would change. Read-only and process-free — the pending-changes
/// banner calls this after every site mutation.
#[tauri::command]
#[specta::specta]
pub async fn plan_site_apply(
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, Option<InstalledRuntimes>>,
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<ApplyPlanDto, IpcError> {
    let input = apply_input(db.inner(), runtimes.inner(), paths.inner()).await?;
    let p = openvhost_core::plan(&input)?;
    Ok(ApplyPlanDto {
        changes: p
            .changes
            .into_iter()
            .map(|c| FileChangeDto {
                path: c.path.display().to_string(),
                kind: change_kind_str(c.kind).to_string(),
                diff: c.diff,
            })
            .collect(),
    })
}
```

And the command that writes:

```rust
/// Apply the sites, then restart whichever affected services are running.
///
/// The restart is the app's job, not core's: `openvhost-core` has no supervisor
/// and must stay usable from the CLI.
#[tauri::command]
#[specta::specta]
pub async fn apply_sites(
    db: tauri::State<'_, Db>,
    runtimes: tauri::State<'_, Option<InstalledRuntimes>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
) -> Result<ApplyOutcomeDto, IpcError> {
    let input = apply_input(db.inner(), runtimes.inner(), paths.inner()).await?;
    let Some(stack) = paths.inner().as_ref() else {
        return Err(IpcError::Core {
            message: "no web server stack is configured for this platform".into(),
        });
    };
    let p = openvhost_core::plan(&input)?;
    let validator = openvhost_core::NginxValidator {
        bin: stack.nginx_bin.clone(),
        err_log: stack.home.join("logs/nginx.error.log"),
    };
    let outcome = openvhost_core::apply(&p, &validator).await?;

    // php-fpm before nginx: nginx connects to the pool socket, so the pool has
    // to be listening first.
    let mut ids: Vec<String> = input
        .runtimes
        .php
        .iter()
        .map(|r| format!("php-fpm-{}", r.major))
        .collect();
    ids.push("nginx".to_string());

    // Only restart what is actually running. A stopped service keeps its state;
    // the new config takes effect when the user starts it.
    let snapshot = sup.snapshot();
    let running: Vec<String> = ids
        .iter()
        .filter(|id| {
            snapshot
                .iter()
                .any(|s| s.id == **id && matches!(s.state, ServiceState::Running))
        })
        .cloned()
        .collect();
    let not_started: Vec<String> = ids
        .iter()
        .filter(|id| !running.contains(id))
        .cloned()
        .collect();

    // Wait for a real Stopped rather than assuming `stop` took effect — the same
    // reason quit.rs polls instead of firing and hoping.
    let for_pending = Arc::clone(sup.inner());
    let watched = running.clone();
    let for_stop = Arc::clone(sup.inner());
    crate::quit::stop_all_with(
        move || {
            for_pending
                .snapshot()
                .into_iter()
                .filter(|s| watched.contains(&s.id))
                .filter(|s| !matches!(s.state, ServiceState::Stopped | ServiceState::Failed { .. }))
                .map(|s| s.id)
                .collect()
        },
        move |id| {
            let _ = for_stop.stop(id);
        },
        std::time::Duration::from_secs(10),
        std::time::Duration::from_millis(50),
    )
    .await;

    for id in &running {
        sup.start(id)?;
    }

    Ok(ApplyOutcomeDto {
        // `u32`, not `usize`: specta rejects pointer-sized ints.
        applied: u32::try_from(outcome.applied).unwrap_or(u32::MAX),
        restarted: running,
        not_started,
    })
}
```

`stop_all_with` takes closures (`pending: Fn() -> Vec<String>`, `stop: Fn(&str)`, `timeout`, `poll`), which is exactly what lets this watch a subset of services rather than everything pending.

Register both commands in `collect_commands![...]` in `apps/desktop/src-tauri/src/lib.rs`.

- [ ] **Step 4: Regenerate the bindings**

Run: `cargo test -p openvhost-desktop export_bindings`
Then add wrappers to `apps/desktop/src/lib/ipc/index.ts` following the existing shape (`if (result.status === 'error') throw result.error;`) and export the new types.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p openvhost-desktop
pnpm -C apps/desktop test
```
Expected: PASS. `git diff --stat apps/desktop/src/lib/ipc/bindings.ts` must show the new commands — a missing diff means the export test did not run.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace -- -D warnings
git add apps/desktop
git commit -s -m "feat(desktop): expose plan_site_apply and apply_sites over IPC"
```

- [ ] **Step 7: Request the security audit**

Dispatch the `security-auditor` subagent over the branch diff, asking specifically about: the two new commands' inputs (neither takes a caller-supplied path), the `config/custom/` deletion boundary in `plan.rs::owned_files`, and the `try_files $uri =404` execution guard in `php-location.conf.tera`. A written APPROVE is required before merge.

---

## Task 8: the banner and the diff dialog

**Files:**
- Create: `apps/desktop/src/lib/apply.svelte.ts` + `apply.svelte.test.ts`
- Create: `apps/desktop/src/lib/components/PendingChangesBanner.svelte`
- Create: `apps/desktop/src/lib/components/ApplyDialog.svelte` + `ApplyDialog.svelte.test.ts`
- Modify: `apps/desktop/src/routes/+page.svelte`

**Interfaces:**
- Consumes: `planSiteApply()`, `applySites()`, `ApplyPlanDto`, `ApplyOutcomeDto` from Task 7.
- Produces:
  ```ts
  export interface ApplyApi {
      planSiteApply(): Promise<ApplyPlanDto>;
      applySites(): Promise<ApplyOutcomeDto>;
  }
  export class ApplyStore {
      changes: FileChangeDto[];
      error: string;            // '' when clear
      applying: boolean;
      outcome: ApplyOutcomeDto | null;
      get pendingCount(): number;
      refresh(): Promise<void>;
      run(): Promise<boolean>;
  }
  ```

- [ ] **Step 1: Write the failing store tests**

`apps/desktop/src/lib/apply.svelte.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { ApplyStore } from './apply.svelte';
import type { ApplyOutcomeDto, ApplyPlanDto } from './ipc';

const change = (path: string, kind: string) => ({ path, kind, diff: `--- a\n+++ b\n+${path}\n` });

function api(overrides: Partial<{ plan: ApplyPlanDto; outcome: ApplyOutcomeDto; fail: unknown }> = {}) {
	return {
		planSiteApply: async () => {
			if (overrides.fail) throw overrides.fail;
			return overrides.plan ?? { changes: [] };
		},
		applySites: async () => {
			if (overrides.fail) throw overrides.fail;
			return overrides.outcome ?? { applied: 0, restarted: [], notStarted: [] };
		}
	};
}

describe('ApplyStore', () => {
	it('reports nothing pending for an empty plan', async () => {
		const s = new ApplyStore(api());
		await s.refresh();
		expect(s.pendingCount).toBe(0);
	});

	it('counts the changes a plan returns', async () => {
		const s = new ApplyStore(api({ plan: { changes: [change('/a.conf', 'added'), change('/b.conf', 'removed')] } }));
		await s.refresh();
		expect(s.pendingCount).toBe(2);
	});

	it('surfaces a failed plan as an error and keeps the count at zero', async () => {
		const s = new ApplyStore(api({ fail: { kind: 'core', message: 'nginx is missing' } }));
		await s.refresh();
		expect(s.error).toBe('nginx is missing');
		expect(s.pendingCount).toBe(0);
	});

	it('clears the pending changes after a successful apply', async () => {
		const s = new ApplyStore({
			planSiteApply: async () => ({ changes: s.outcome ? [] : [change('/a.conf', 'added')] }),
			applySites: async () => ({ applied: 1, restarted: ['nginx'], notStarted: [] })
		});
		await s.refresh();
		expect(s.pendingCount).toBe(1);
		expect(await s.run()).toBe(true);
		expect(s.outcome?.restarted).toEqual(['nginx']);
		expect(s.pendingCount).toBe(0);
	});

	it('keeps the changes and shows the validator output when apply fails', async () => {
		const s = new ApplyStore({
			planSiteApply: async () => ({ changes: [change('/a.conf', 'added')] }),
			applySites: async () => {
				throw { kind: 'core', message: 'nginx: [emerg] unknown directive' };
			}
		});
		await s.refresh();
		expect(await s.run()).toBe(false);
		expect(s.error).toContain('unknown directive');
		expect(s.pendingCount).toBe(1);
		expect(s.applying).toBe(false);
	});

	it('refuses a second concurrent apply', async () => {
		let calls = 0;
		const s = new ApplyStore({
			planSiteApply: async () => ({ changes: [change('/a.conf', 'added')] }),
			applySites: async () => {
				calls += 1;
				await new Promise((r) => setTimeout(r, 5));
				return { applied: 1, restarted: [], notStarted: [] };
			}
		});
		await s.refresh();
		await Promise.all([s.run(), s.run()]);
		expect(calls).toBe(1);
	});
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test apply.svelte`
Expected: FAIL — cannot resolve `./apply.svelte`.

- [ ] **Step 3: Implement the store**

`apps/desktop/src/lib/apply.svelte.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Pending-changes state for the site apply pipeline. `refresh` is cheap by
// design (the Rust side spawns nothing), so it runs after every site mutation.
import type { ApplyOutcomeDto, ApplyPlanDto, FileChangeDto } from './ipc';

export interface ApplyApi {
	planSiteApply(): Promise<ApplyPlanDto>;
	applySites(): Promise<ApplyOutcomeDto>;
}

function errorMessage(e: unknown): string {
	if (typeof e === 'object' && e !== null && 'message' in e) {
		const m = (e as { message?: unknown }).message;
		if (typeof m === 'string' && m !== '') return m;
	}
	return 'The command failed.';
}

export class ApplyStore {
	changes = $state<FileChangeDto[]>([]);
	error = $state('');
	applying = $state(false);
	outcome = $state<ApplyOutcomeDto | null>(null);

	constructor(private api: ApplyApi) {}

	get pendingCount(): number {
		return this.changes.length;
	}

	async refresh(): Promise<void> {
		this.error = '';
		try {
			this.changes = (await this.api.planSiteApply()).changes;
		} catch (e) {
			this.error = errorMessage(e);
			this.changes = [];
		}
	}

	/**
	 * Apply, then re-plan. The re-plan is the honest source of the new pending
	 * count: assuming zero would hide anything the apply could not write.
	 *
	 * The re-entrancy guard lives here rather than only on the button's
	 * `disabled` attribute — deleting an attribute leaves no test failing.
	 */
	async run(): Promise<boolean> {
		if (this.applying) return false;
		this.applying = true;
		this.error = '';
		try {
			this.outcome = await this.api.applySites();
		} catch (e) {
			this.error = errorMessage(e);
			return false;
		} finally {
			this.applying = false;
		}
		await this.refresh();
		return true;
	}
}
```

- [ ] **Step 4: Run the store tests**

Run: `pnpm -C apps/desktop test apply.svelte`
Expected: PASS — 6 tests.

- [ ] **Step 5: Write the failing component tests**

`apps/desktop/src/lib/components/ApplyDialog.svelte.test.ts`, using the SSR `render`-to-string pattern the existing `SiteDrawer.svelte.test.ts` uses:

```ts
it('renders a badge for every change kind', () => {
	const body = renderDialog({
		changes: [c('/nginx.conf', 'modified'), c('/sites/a.conf', 'added'), c('/sites/b.conf', 'removed')]
	});
	expect(body).toContain('data-kind="modified"');
	expect(body).toContain('data-kind="added"');
	expect(body).toContain('data-kind="removed"');
});

it('shows the diff text for each file', () => {
	const body = renderDialog({ changes: [c('/sites/a.conf', 'added')] });
	expect(body).toContain('+/sites/a.conf');
});

it('shows the validator error with its line breaks preserved', () => {
	const body = renderDialog({ changes: [], error: 'nginx: [emerg] line 1\nline 2' });
	expect(body).toContain('line 2');
	expect(body).toMatch(/white-space:\s*pre-wrap/);
});

it('disables the apply button while an apply is in flight', () => {
	expect(renderDialog({ changes: [c('/a.conf', 'added')], applying: true })).toContain('disabled');
	expect(renderDialog({ changes: [c('/a.conf', 'added')], applying: false })).not.toContain('disabled');
});

it('names the services it restarted', () => {
	const body = renderDialog({ changes: [], outcome: { applied: 2, restarted: ['php-fpm-8.4', 'nginx'], notStarted: [] } });
	expect(body).toContain('php-fpm-8.4');
	expect(body).toContain('nginx');
});

it('says when a changed service was not running', () => {
	const body = renderDialog({ changes: [], outcome: { applied: 1, restarted: [], notStarted: ['nginx'] } });
	expect(body).toMatch(/next time|not running/i);
});
```

Each assertion must be able to fail for the reason it names — the `disabled` test checks both directions for exactly that reason.

`PendingChangesBanner` gets its own cases inside the same file or a sibling: hidden at `count === 0`, singular/plural copy at 1 and 2.

- [ ] **Step 6: Run to verify failure**

Run: `pnpm -C apps/desktop test ApplyDialog`
Expected: FAIL — component does not exist.

- [ ] **Step 7: Build the components**

`PendingChangesBanner.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import Button from './Button.svelte';

	let { count, onReview }: { count: number; onReview: () => void } = $props();
</script>

{#if count > 0}
	<div class="banner" role="status" data-testid="pending-changes">
		<span>
			{count}
			{count === 1 ? 'change' : 'changes'} not applied yet
		</span>
		<div class="grow"></div>
		<Button variant="primary" onclick={onReview}>Review and apply</Button>
	</div>
{/if}
```

`ApplyDialog.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import type { ApplyOutcomeDto, FileChangeDto } from '$lib/ipc';
	import Button from './Button.svelte';

	let {
		changes,
		applying = false,
		error = '',
		outcome = null,
		onApply,
		onClose
	}: {
		changes: readonly FileChangeDto[];
		applying?: boolean;
		error?: string;
		outcome?: ApplyOutcomeDto | null;
		onApply: () => void;
		onClose: () => void;
	} = $props();
</script>

<div class="scrim">
	<section class="dialog" role="dialog" aria-modal="true" aria-label="Apply changes">
		<header>
			<h2>Apply changes</h2>
			<p class="sub">{changes.length} {changes.length === 1 ? 'file' : 'files'}</p>
		</header>

		<div class="files">
			{#each changes as c (c.path)}
				<article class="file" data-kind={c.kind}>
					<div class="path">
						<span class="badge" data-kind={c.kind}>{c.kind}</span>
						<span class="mono">{c.path}</span>
					</div>
					<!-- Split per line so an added/removed line can carry its own colour;
					     a single <pre> could only be coloured as a whole. -->
					<pre class="diff">{#each c.diff.split('\n') as line}<span
								class="line"
								data-line={line.startsWith('+') ? 'add' : line.startsWith('-') ? 'del' : 'ctx'}
								>{line}
							</span>{/each}</pre>
				</article>
			{/each}
		</div>

		{#if error !== ''}
			<!-- pre-wrap: nginx's stderr is multi-line and ran off-screen when it
			     was rendered as a single line (the ServiceRow lesson). -->
			<p class="error" role="alert">{error}</p>
		{/if}

		{#if outcome}
			<p class="ok" role="status">
				Applied.
				{#if outcome.restarted.length > 0}Restarted {outcome.restarted.join(', ')}.{/if}
				{#if outcome.notStarted.length > 0}
					{outcome.notStarted.join(', ')} was not running — the new config applies next time it starts.
				{/if}
			</p>
		{/if}

		<footer>
			<Button onclick={onClose}>Close</Button>
			<Button variant="primary" disabled={applying || changes.length === 0} onclick={onApply}>
				{applying ? 'Applying…' : 'Apply'}
			</Button>
		</footer>
	</section>
</div>

<style>
	.error {
		white-space: pre-wrap;
		color: var(--vh-danger-text);
	}
	.diff {
		white-space: pre;
		overflow-x: auto;
		font-family: var(--vh-font-mono);
		font-size: var(--vh-text-table);
	}
	/* … layout using --vh-surface / --vh-border / --vh-space-* … */
</style>
```

A unified diff's header lines (`+++ b/...`, `--- a/...`) also start with `+`/`-`, so they pick up the add/del colour. That is acceptable — they genuinely name the new and old file — and the alternative, a line-index special case, is more code than the problem is worth.

Diff colours: add `--vh-diff-add-*` / `--vh-diff-del-*` pairs to `apps/desktop/src/lib/styles/tokens.css` for both themes, and verify each foreground/background pair reaches 4.5:1 before committing — the contrast regression fixed in PR #22 came in exactly this way.

- [ ] **Step 8: Wire it into the page**

In `apps/desktop/src/routes/+page.svelte`: construct `ApplyStore` with the Task 7 wrappers, call `applyStore.refresh()` after `store.load()` and after every mutation (`save`, `remove`, `removeRow`, `setEnabled`), render `PendingChangesBanner` above `SitesPanel`, and open `ApplyDialog` from its button.

- [ ] **Step 9: Run the whole frontend gate**

```bash
pnpm -C apps/desktop test
pnpm -C apps/desktop exec svelte-check
```
Expected: PASS, 0 errors / 0 warnings.

- [ ] **Step 10: Commit**

```bash
git add apps/desktop/src
git commit -s -m "feat(ui): review and apply pending site changes

A banner when the generated config drifts from the sites, a dialog
showing the unified diff per file, and a failure path that surfaces the
validator's own output."
```

---

## Task 9: prove a real site is actually served

The regression test for the two template defects this slice fixes. It runs only where Homebrew nginx and php-fpm exist, and skips cleanly elsewhere.

**Files:**
- Create: `crates/openvhost-core/tests/site_apply_e2e.rs`

**Interfaces:**
- Consumes: `plan`, `apply`, `NginxValidator`, `ApplyInput`, `InstalledRuntimes`, `provision_home`, `find_brew_binaries`, `probe_php_fpm_version`, `LISTEN_PORT`.

- [ ] **Step 1: Write the test**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end: apply a site, start the real stack, and prove the three things
//! the generated config must get right — PHP runs, static files keep their MIME
//! type, and a non-file path is never executed.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Kills the child on drop, so a failed assertion cannot leave a stray nginx
/// or php-fpm holding the port.
struct Killed(Child);
impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Minimal HTTP/1.0 GET. Raw TCP keeps this test dependency-free.
fn get(port: u16, host: &str, path: &str, deadline: Instant) -> Option<String> {
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                if s.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}
```

The test body:

1. `tempfile::tempdir()` for the home (keep it short — the socket guard is real), `provision_home(&home)`.
2. `find_brew_binaries()`; `return` early with an explanatory `eprintln!` if `None`.
3. `probe_php_fpm_version(&brew.php_fpm).await` for the major; return early if `None`.
4. Build a docroot in a second temp dir with `index.php` = `<?php echo "PHP-OK " . PHP_VERSION;` and `style.css` = `body { color: red; }`.
5. Build `ApplyInput` with one enabled site (`domain` = `e2e.localhost`, that docroot, that major).
6. `apply(&plan(&input)?, &NginxValidator { .. }).await` — assert `Ok`.
7. Spawn php-fpm (`-F -O -n -y <home>/config/generated/php/<major>/php-fpm.conf`) and nginx (`-e <home>/logs/nginx.error.log -c <home>/config/generated/nginx/nginx.conf`), each wrapped in `Killed`.
8. Assert, with a 10-second deadline:

```rust
    let php = get(LISTEN_PORT, "e2e.localhost", "/index.php", deadline).expect("no response");
    assert!(php.contains("PHP-OK"), "PHP did not execute:\n{php}");

    let css = get(LISTEN_PORT, "e2e.localhost", "/style.css", deadline).unwrap();
    // Regression: without the types{} block this is application/octet-stream
    // and the browser refuses to apply the stylesheet.
    assert!(css.contains("Content-Type: text/css"), "wrong MIME type:\n{css}");
    assert!(css.contains("color: red"));

    // Regression: without `try_files $uri =404` in the PHP location, a file
    // that is not PHP is executed as PHP through path info.
    let exploit = get(LISTEN_PORT, "e2e.localhost", "/style.css/x.php", deadline).unwrap();
    assert!(exploit.starts_with("HTTP/1.1 404"), "path-info guard failed:\n{exploit}");

    // The catch-all answers a host that matches no site.
    let fallback = get(LISTEN_PORT, "nothing.localhost", "/index.php", deadline).unwrap();
    assert!(fallback.contains("phpinfo") || fallback.contains("PHP Version"));
```

Use `#[tokio::test]` since `apply` and the probe are async.

**Port note:** the test binds the real `LISTEN_PORT` (8080), so it cannot run while the app is running. Mark it `#[ignore]` only if that proves disruptive in practice; the default is to run it, since a silently-skipped E2E is worth nothing.

- [ ] **Step 2: Run it**

Run: `cargo test -p openvhost-core --test site_apply_e2e -- --nocapture`
Expected: PASS. A failure on the CSS or the exploit assertion means Task 1's template regressed — fix the template, never the assertion.

- [ ] **Step 3: Full gate and commit**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
pnpm -C apps/desktop test && pnpm -C apps/desktop exec svelte-check
git add crates/openvhost-core/tests
git commit -s -m "test(core): serve a real site end to end

Applies a site, starts the real nginx + php-fpm, and asserts PHP runs,
CSS keeps its MIME type, and /style.css/x.php is a 404."
```

---

## Definition of Done

- [ ] Creating a site in the UI, pressing Apply and confirming the diff serves it at `http://<domain>:8080`.
- [ ] A stylesheet in the docroot arrives as `text/css` and is not executed.
- [ ] Disabling a site removes its vhost on the next apply.
- [ ] A site requesting an uninstalled PHP major blocks the apply with a message naming the site and the installed versions, leaving the tree untouched.
- [ ] A config that fails `nginx -t` rolls back byte-for-byte and shows nginx's own stderr.
- [ ] `~/.openvhost/conf/nginx.conf` is no longer written by anything.
- [ ] security-auditor APPROVE recorded on the branch.
- [ ] Full gate green: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop exec svelte-check`.
