# Web Server Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A rail entry and `/web-server` page that report the real facts about each web server OpenVHost knows — supervised status, the binary and config that were actually registered, version, hot-reload support, the live config's text — plus a Validate action that runs `nginx -t` against the live config.

**Architecture:** Read-only throughout. New `openvhost-conf/src/inspect.rs` owns the two process probes (`probe_nginx_version`, `validate_live`), keeping them in the tauri-free crate that already owns web-server knowledge. `apps/desktop/src-tauri/src/stack.rs` starts returning the paths it already resolves so the app can `manage` them as state; three new read-only IPC commands read that state and call `inspect`. The page reuses the shared services store for status, so no second status source exists.

**Tech Stack:** Rust (tokio, thiserror, specta/tauri-specta), SvelteKit 5 runes, Tailwind 4, vitest with `svelte/server` SSR rendering.

**Spec:** `docs/superpowers/specs/2026-07-25-p1-web-server-page-design.md`

## Global Constraints

- SPDX header as line 1 of every new file: `// SPDX-License-Identifier: GPL-3.0-or-later` (Rust/TS), `<!-- SPDX-License-Identifier: GPL-3.0-or-later -->` (Svelte).
- `openvhost-core`, `-proc`, `-conf`, `-pkg` must never depend on `tauri`.
- No `unwrap()` / `expect()` outside `#[cfg(test)]`.
- **Do not modify `crates/openvhost-proc/`.** The spec's §3.3 correction explains why: adding a spec accessor for a read-only page is out of scope.
- **No `materialize()` call anywhere in this slice.** Its contract forbids a live home.
- **`-e <err_log>` is mandatory on every nginx invocation.** Without it nginx writes into the Homebrew prefix (`/opt/homebrew/var`). The existing P0-7 validator carries this comment; it is not optional.
- Both process spawns must be bounded by a timeout and kill the child on expiry.
- The client sends only an opaque brand id parsed against a closed list — never a path, filename, or argument. Binary and config paths are derived server-side.
- Both `read_web_server_config` and `validate_web_server_config` reject an unsupported brand id with a validation error, never empty output.
- Design tokens only in Svelte (`--vh-*` from `src/lib/styles/tokens.css`); zero colour/size literals.
- No `console.log`, no `any`, no `@ts-ignore`.
- Never `window.alert` / `window.confirm` — the dialog plugin replaces them webview-wide and our ACL denies them, so they fail silently.
- Do not edit `prettier.config.js` (a hook blocks it). Do not hand-edit `src/lib/ipc/bindings.ts` (generated — regenerate it).
- Conventional Commits, DCO sign-off (`git commit -s`), **no `Co-Authored-By` trailer**.
- Gate suite, read from a raw pipe (a summarising wrapper has hidden a lint crash here before): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop lint`, `pnpm -C apps/desktop check` (0 errors 0 warnings), `pnpm -C apps/desktop test`, `pnpm -C apps/desktop build`, `bash scripts/check-spdx.sh`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/openvhost-conf/src/inspect.rs` *(create)* | The two process probes. Pure functions over explicit paths — no home resolution, no state, no globals, so they are testable with a fake binary. |
| `crates/openvhost-conf/src/lib.rs` *(modify)* | Declare + re-export the new module. |
| `crates/openvhost-conf/src/error.rs` *(modify)* | One new `ConfError` variant for a timed-out validator. |
| `apps/desktop/src-tauri/src/stack.rs` *(modify)* | Return the resolved paths alongside the specs instead of discarding them. |
| `apps/desktop/src-tauri/src/lib.rs` *(modify)* | `manage` the resolved paths; register the three commands. |
| `apps/desktop/src-tauri/src/commands.rs` *(modify)* | The three read-only commands + their DTOs + brand-id parsing. |
| `apps/desktop/src/lib/webservers.derive.ts` *(create)* | Pure view helpers (row labels, status correlation), unit-testable without a DOM. |
| `apps/desktop/src/lib/webservers.svelte.ts` *(create)* | `WebServersStore` — load, read config, validate; owns its own error channel. |
| `apps/desktop/src/lib/components/WebServerPanel.svelte` *(create)* | The page head + rows. |
| `apps/desktop/src/lib/components/WebServerRow.svelte` *(create)* | One brand's row: facts, config disclosure, Validate. |
| `apps/desktop/src/routes/web-server/+page.svelte` *(create)* | Route; wires store to panel inside `AppShell`. |
| `apps/desktop/src/lib/components/Rail.svelte` *(modify)* | "Web Server" becomes a real nav item. |
| `apps/desktop/src/lib/components/AppShell.svelte` *(modify)* | Widen the `active` union to include `'web-server'`. |

---

## Task 1: `inspect.rs` — the two process probes

**Files:**
- Create: `crates/openvhost-conf/src/inspect.rs`
- Modify: `crates/openvhost-conf/src/lib.rs`
- Modify: `crates/openvhost-conf/src/error.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/openvhost-conf/src/inspect.rs`

**Interfaces:**
- Consumes: `ConfError` and `ValidationReport { ok: bool, stderr: String }` (already exported from this crate; `ValidationReport` lives in `ctx.rs`).
- Produces, for Task 3:
  - `pub async fn probe_version(bin: &Path) -> Option<String>`
  - `pub async fn validate_live(bin: &Path, conf: &Path, err_log: &Path) -> Result<ValidationReport, ConfError>`
  - `pub const PROBE_TIMEOUT: Duration` (5 seconds)
  - New variant `ConfError::ValidatorTimeout { bin: String, secs: u64 }`

- [ ] **Step 1: Write the failing tests**

Create `crates/openvhost-conf/src/inspect.rs` with only the test module plus the two signatures unimplemented (`todo!()` is acceptable *for this one step only*, and must be gone by Step 3):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only inspection of an installed web server: what version is it, and is
//! a given config file valid? Both shell out, so both are bounded by
//! `PROBE_TIMEOUT` and kill the child on expiry — these run behind a UI action,
//! where an unbounded wait is a hung spinner.
//!
//! Deliberately NOT `WebServerAdapter::validate`: that materializes generated
//! files into `ctx.home` first, and `validate::materialize`'s contract forbids
//! pointing it at a live home. `validate_live` validates a config file that
//! already exists, in place, writing nothing.

use std::path::Path;
use std::time::Duration;

use crate::ValidationReport;
use crate::error::ConfError;

/// Both probes are short-lived local process launches; 5s is far beyond a
/// healthy `nginx -v`/`-t` and short enough that a wedged binary surfaces as an
/// error instead of a spinner that never resolves.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn probe_version(bin: &Path) -> Option<String> {
    todo!()
}

pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError> {
    todo!()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fake "binary": a shell script that writes fixed text and exits with a
    /// fixed code. Lets these tests assert real spawn behaviour without needing
    /// nginx installed, which CI and most dev machines cannot assume.
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn version_is_read_from_stderr_not_stdout() {
        // nginx prints its banner to STDERR. A stdout-only reader returns None
        // against a real nginx, which is the bug this pins.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "echo 'nginx version: nginx/1.27.3' 1>&2");
        assert_eq!(probe_version(&bin).await.as_deref(), Some("1.27.3"));
    }

    #[tokio::test]
    async fn version_tolerates_a_banner_with_extra_build_detail() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "echo 'nginx version: nginx/1.25.1 (Ubuntu)' 1>&2");
        assert_eq!(probe_version(&bin).await.as_deref(), Some("1.25.1"));
    }

    #[tokio::test]
    async fn version_is_none_when_the_output_has_no_version() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "echo 'totally unrelated' 1>&2");
        assert_eq!(probe_version(&bin).await, None);
    }

    #[tokio::test]
    async fn version_is_none_when_the_binary_does_not_exist() {
        // A missing binary must not be an error that fails a whole page load.
        assert_eq!(probe_version(Path::new("/nonexistent/nginx")).await, None);
    }

    #[tokio::test]
    async fn validate_reports_ok_from_the_exit_code_alone() {
        // Success still writes to stderr (nginx says "syntax is ok" there), so
        // deriving `ok` from stderr emptiness would report every success as a
        // failure.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "echo 'syntax is ok' 1>&2; exit 0");
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(r.ok);
        assert!(r.stderr.contains("syntax is ok"));
    }

    #[tokio::test]
    async fn validate_reports_failure_and_keeps_stderr_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "echo 'unknown directive \"bogus\"' 1>&2; exit 1");
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(!r.ok);
        assert!(r.stderr.contains("unknown directive"));
    }

    #[tokio::test]
    async fn validate_passes_the_mandatory_error_log_flag_and_the_config_path() {
        // Pins the argv shape. Without `-e`, nginx writes into the Homebrew
        // prefix; without `-c`, it validates its own compiled-in config instead
        // of ours. The fake echoes its args so the test can read them back.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", r#"echo "$@" 1>&2"#);
        let r = validate_live(
            &bin,
            Path::new("/tmp/live.conf"),
            Path::new("/tmp/err.log"),
        )
        .await
        .unwrap();
        assert!(r.stderr.contains("-e /tmp/err.log"), "argv was: {}", r.stderr);
        assert!(r.stderr.contains("-t"), "argv was: {}", r.stderr);
        assert!(r.stderr.contains("-c /tmp/live.conf"), "argv was: {}", r.stderr);
    }

    #[tokio::test]
    async fn validate_errors_when_the_binary_cannot_be_launched() {
        let e = validate_live(
            Path::new("/nonexistent/nginx"),
            Path::new("/tmp/x.conf"),
            Path::new("/tmp/e.log"),
        )
        .await
        .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorSpawn { .. }));
    }

    #[tokio::test]
    async fn validate_times_out_instead_of_hanging_forever() {
        // The P0-7 validator uses a bare `.output().await` with no timeout. This
        // pins that the UI-facing path does not inherit that.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(&d.path(), "nginx", "sleep 30");
        let e = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorTimeout { .. }), "got {e:?}");
    }
}
```

Add to `crates/openvhost-conf/src/lib.rs`, after `mod engine;`:

```rust
mod inspect;
```

and extend its `pub use` block with:

```rust
pub use inspect::{PROBE_TIMEOUT, probe_version, validate_live};
```

Add to `crates/openvhost-conf/src/error.rs`, inside `pub enum ConfError`, after the `ValidatorSpawn` variant:

```rust
    #[error("validator {bin} did not finish within {secs}s and was killed")]
    ValidatorTimeout { bin: String, secs: u64 },
```

Add `tempfile` to the crate's dev-dependencies if absent — it is already there (`crates/openvhost-conf/Cargo.toml`, `[dev-dependencies] tempfile = "3"`), so no change is expected; verify rather than assume.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openvhost-conf inspect 2>&1 | tail -20`

Expected: the suite fails. `todo!()` panics, so failures read `not yet implemented`. **The timeout test must be among the failures** — if it passes at this point something is wrong with the test, not the code.

- [ ] **Step 3: Implement both probes**

Replace the two `todo!()` bodies in `crates/openvhost-conf/src/inspect.rs`:

```rust
/// Version as the binary reports it (`1.27.3` from `nginx version: nginx/1.27.3`),
/// or `None` for any failure — missing binary, non-zero exit, unparseable banner,
/// timeout. Deliberately not a `Result`: a page that lists servers should still
/// list them when one version is unknowable, and the caller has nothing
/// actionable to do with the distinction.
pub async fn probe_version(bin: &Path) -> Option<String> {
    let run = tokio::process::Command::new(bin).arg("-v").output();
    // nginx writes its banner to STDERR, not stdout.
    let out = tokio::time::timeout(PROBE_TIMEOUT, run).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    parse_version(&text)
}

/// Pull `1.27.3` out of `nginx version: nginx/1.27.3 (extra build detail)`.
/// Split out so the parsing is testable without spawning anything.
fn parse_version(stderr: &str) -> Option<String> {
    let after_slash = stderr.split_once('/')?.1;
    let token = after_slash
        .split_whitespace()
        .next()?
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    if token.is_empty() || !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(token.to_string())
}

/// Validate a config file that ALREADY EXISTS, in place. Writes nothing to
/// `conf` and never calls `materialize`.
///
/// `-e <err_log>` is MANDATORY: without it nginx writes into its compiled-in
/// prefix (`/opt/homebrew/var`) rather than our home.
pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError> {
    let mut child = tokio::process::Command::new(bin)
        .arg("-e")
        .arg(err_log)
        .arg("-t")
        .arg("-c")
        .arg(conf)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ConfError::ValidatorSpawn {
            bin: bin.display().to_string(),
            source: e,
        })?;

    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => {
            let out = res.map_err(|e| ConfError::ValidatorSpawn {
                bin: bin.display().to_string(),
                source: e,
            })?;
            Ok(ValidationReport {
                // Exit code ONLY — nginx writes to stderr even on success.
                ok: out.status.success(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
        Err(_) => Err(ConfError::ValidatorTimeout {
            bin: bin.display().to_string(),
            secs: PROBE_TIMEOUT.as_secs(),
        }),
    }
}
```

Note on the timeout branch: `wait_with_output()` consumes the child, so the kill-on-expiry comes from tokio dropping the `Child` — `tokio::process::Child` defaults to `kill_on_drop(false)`, so **add `.kill_on_drop(true)` to the `Command` builder above** (immediately after `.stderr(...)`) or the sleeping process is leaked. Verify with the timeout test plus `pgrep -f 'sleep 30'` returning nothing after the test run.

Also add a direct unit test for the extracted parser, since it is now a separate function:

```rust
    #[test]
    fn parse_version_pulls_the_token_after_the_slash() {
        assert_eq!(parse_version("nginx version: nginx/1.27.3").as_deref(), Some("1.27.3"));
        assert_eq!(parse_version("nginx/1.2.3 (x)").as_deref(), Some("1.2.3"));
        assert_eq!(parse_version("no slash here"), None);
        assert_eq!(parse_version("trailing/"), None);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p openvhost-conf inspect 2>&1 | grep -E "test result|FAILED"`

Expected: `test result: ok.` with 0 failed. Then confirm no leaked child: `pgrep -f 'sleep 30' || echo "no leak"` → `no leak`.

- [ ] **Step 5: Run the Rust gate and commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
bash scripts/check-spdx.sh
git add crates/openvhost-conf/src/inspect.rs crates/openvhost-conf/src/lib.rs crates/openvhost-conf/src/error.rs
git commit -s -m "feat(conf): inspect module — version probe and in-place config validation

Both bounded by a 5s timeout with kill_on_drop, because these run behind a UI
action where an unbounded wait is a spinner that never resolves.

validate_live deliberately does NOT reuse WebServerAdapter::validate: that
materializes generated files into ctx.home first, and materialize()'s contract
forbids pointing it at a live home. This validates a config that already
exists, in place, writing nothing.

-e <err_log> is passed for the same reason the P0-7 validator passes it —
without it nginx writes into its compiled-in prefix instead of our home — and a
test pins the argv shape so that cannot silently regress. ok comes from the exit
code alone; nginx writes to stderr on success too."
```

---

### Task 1 — As built (divergences from the code blocks above)

Task 1 is complete. Its code blocks above are the plan as written; the implementation
diverged in ways later tasks must use. They are recorded here rather than by editing the
blocks, so the plan stays an honest record of what was predicted versus what was needed.

1. **`probe_version` → `probe_nginx_version(bin: &Path, err_log: &Path)`.** Renamed because
   it parses nginx's banner specifically — php-fpm writes `-v` to *stdout* and would yield
   `None`, making the generic name a trap. The `err_log` argument was added so the version
   probe also satisfies the global "`-e` on every nginx invocation" constraint literally,
   rather than resting on an unverifiable claim about when nginx initialises its log.
2. **The timeout kills the process *group*, not the child.** `kill_on_drop` alone leaked a
   real grandchild — `Child::kill()` signals one pid. The implementation uses
   `process_group(0)` plus `libc::kill(-pgid, SIGKILL)`, mirroring `openvhost-proc`'s
   `UnixDriver`, which added `libc` to this crate under
   `[target.'cfg(unix)'.dependencies]`. This duplicates a containment invariant that
   already exists in `openvhost-proc`; the golden-rule-4 reading is documented in the
   module and is **pending security-auditor confirmation in Task 6**.
3. Test module is `#[cfg(all(test, unix))]` (it uses `PermissionsExt` and `#!/bin/sh`
   fakes, which would fail a Windows workspace *compile*), and the timeout test uses
   `#[tokio::test(start_paused = true)]`.

## Task 2: `stack.rs` returns the paths it already resolved

**Files:**
- Modify: `apps/desktop/src-tauri/src/stack.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `apps/desktop/src-tauri/src/stack.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces, for Task 3:
  ```rust
  pub struct StackPaths {
      pub home: std::path::PathBuf,
      pub nginx_bin: std::path::PathBuf,
      pub nginx_conf: std::path::PathBuf,
  }
  pub struct MacosStack {
      pub specs: Vec<ServiceSpec>,
      pub paths: Option<StackPaths>,
  }
  pub fn macos_stack() -> MacosStack
  ```
  `paths` is `None` exactly when home resolution failed (the same condition that already yields zero specs).

**Why:** the page must report the binary and config that were **actually registered**. `Supervisor` exposes no way to read a `ServiceSpec` back (`register`/`snapshot`/`log_tail`/`start`/`stop`/`subscribe` only, and `ServiceStatus` has no program field), and a second `find_brew_binaries()` call could disagree with the first — it returns `None` unless **both** nginx and php-fpm exist, so on a machine with nginx but no php-fpm a re-probe would report "not found" for a binary that is present. Returning the already-computed values removes the possibility of disagreement.

- [ ] **Step 1: Write the failing test**

Append to `apps/desktop/src-tauri/src/stack.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The paths handed to the UI must be the SAME ones baked into the specs.
    /// A second `find_brew_binaries()` call could disagree with the first (it
    /// returns None unless BOTH nginx and php-fpm exist), so this pins that the
    /// page and the supervisor cannot drift.
    #[test]
    fn reported_paths_match_the_registered_nginx_spec() {
        let stack = macos_stack();
        let Some(paths) = stack.paths else {
            // No home resolvable in this environment: the specs must be empty
            // too, which is the existing contract.
            assert!(stack.specs.is_empty());
            return;
        };
        let nginx = stack
            .specs
            .iter()
            .find(|s| s.id == "nginx")
            .expect("nginx spec should be registered when a home resolves");
        assert_eq!(nginx.spawn.program, paths.nginx_bin);
        // The spec spawns with `-c <conf>`; the reported conf must be that path.
        let args: Vec<String> = nginx
            .spawn
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let c = args.iter().position(|a| a == "-c").expect("nginx spawns with -c");
        assert_eq!(args[c + 1], paths.nginx_conf.to_string_lossy());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p openvhost-desktop stack 2>&1 | tail -15`

Expected: a compile error — `macos_stack` and `StackPaths` do not exist yet (`cannot find function`/`cannot find type`). A compile failure is a valid RED here.

- [ ] **Step 3: Implement**

In `apps/desktop/src-tauri/src/stack.rs`, add above `macos_stack_specs`:

```rust
/// The paths the stack actually registered, so the Web Server page can report
/// them instead of re-probing and possibly disagreeing.
pub struct StackPaths {
    pub home: PathBuf,
    pub nginx_bin: PathBuf,
    pub nginx_conf: PathBuf,
}

/// Specs to register plus the paths they were built from. `paths` is `None`
/// exactly when the home could not be resolved — the same condition that
/// already produces zero specs.
pub struct MacosStack {
    pub specs: Vec<ServiceSpec>,
    pub paths: Option<StackPaths>,
}
```

Then convert the existing `macos_stack_specs` into `macos_stack`. Keep every existing behaviour — the provisioning call, the non-fatal error logging, the fallback binaries, both `ServiceSpec`s, `DEMO_PORT` — and change only the return:

```rust
pub fn macos_stack() -> MacosStack {
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("stack: cannot resolve OPENVHOST_HOME, skipping nginx/php-fpm rows: {e}");
            return MacosStack { specs: vec![], paths: None };
        }
    };
    if let Err(e) = provision_macos_demo_stack(&home, DEMO_PORT) {
        eprintln!("stack: provisioning failed (rows registered anyway): {e}");
    }
    let brew = find_brew_binaries().unwrap_or_else(fallback_brew);
    let conf = home.join("conf");
    let nginx_conf = conf.join("nginx.conf");
    let paths = StackPaths {
        home: home.clone(),
        nginx_bin: brew.nginx.clone(),
        nginx_conf: nginx_conf.clone(),
    };
    let specs = vec![
        // ... the php-fpm ServiceSpec exactly as it is today ...
        ServiceSpec {
            id: "nginx".into(),
            display_name: "nginx".into(),
            endpoint: Some(format!("http://127.0.0.1:{DEMO_PORT}")),
            spawn: SpawnSpec {
                program: brew.nginx,
                args: vec![
                    OsString::from("-e"),
                    home.join("logs/nginx.error.log").into_os_string(),
                    OsString::from("-c"),
                    nginx_conf.into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
    ];
    MacosStack { specs, paths: Some(paths) }
}
```

Keep the php-fpm spec byte-identical to what is there now; only the nginx spec's `-c` argument changes source (from `conf.join("nginx.conf")` to the shared `nginx_conf` binding), which must produce the same path.

In `apps/desktop/src-tauri/src/lib.rs`, at the registration site (currently a loop over `macos_stack_specs()` near line 151), switch to the new function and manage the paths. The existing call looks like `for spec in stack::macos_stack_specs() { supervisor.register(spec); }` inside the single-instance-lock arm; replace with:

```rust
let stack = stack::macos_stack();
for spec in stack.specs {
    supervisor.register(spec);
}
// Manage the Option ITSELF, unconditionally. Tauri implements `CommandArg` only
// for `State<'r, T>` — there is no impl for `Option<State<'r, T>>` — so a command
// cannot take an optionally-managed state. Making `Option<StackPaths>` the managed
// type is what lets Task 3's command distinguish "no home resolved" from "not
// wired up", while always having something to extract.
app.manage(stack.paths);
```

Place `app.manage(stack.paths)` in the same arm and before any command can run, alongside the existing `Db` management. The managed type is `Option<StackPaths>`, never a bare `StackPaths`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p openvhost-desktop stack 2>&1 | grep -E "test result|FAILED"`

Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
git add apps/desktop/src-tauri/src/stack.rs apps/desktop/src-tauri/src/lib.rs
git commit -s -m "refactor(desktop): stack returns the paths it resolved, and the app manages them

The Web Server page must report the binary and config that were ACTUALLY
registered. Supervisor exposes no way to read a ServiceSpec back — register/
snapshot/log_tail/start/stop/subscribe only, and ServiceStatus carries no
program path — and a second find_brew_binaries() call could disagree with the
first, since it returns None unless BOTH nginx and php-fpm exist. Returning the
already-computed values removes the possibility of drift, with no change to
openvhost-proc.

A test asserts the reported paths equal the registered spec's program and its
-c argument, so the page and the supervisor cannot diverge silently."
```

---

## Task 3: the three read-only IPC commands

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register in `collect_commands!`)
- Regenerate: `apps/desktop/src/lib/ipc/bindings.ts`
- Test: inline `#[cfg(test)] mod tests` in `apps/desktop/src-tauri/src/commands.rs`

**Interfaces:**
- Consumes: `openvhost_conf::{probe_nginx_version, validate_live}` (Task 1); `crate::stack::StackPaths` (Task 2); the existing `IpcError` with its `Validation { field, message }` and `Core { message }` variants.
- Produces, for Task 4 — the exact TS shapes specta will emit:
  ```ts
  type WebServerDto = {
    id: string; displayName: string; supported: boolean;
    serviceId: string | null; binaryPath: string | null;
    version: string | null; supportsHotReload: boolean; configPath: string | null;
  }
  type ValidationReportDto = { ok: boolean; stderr: string }
  listWebServers(): Promise<WebServerDto[]>
  readWebServerConfig(id: string): Promise<string>
  validateWebServerConfig(id: string): Promise<ValidationReportDto>
  ```

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `apps/desktop/src-tauri/src/commands.rs` (there is already a `site_ipc_tests`-style module; add a sibling module rather than editing existing tests):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod web_server_ipc_tests {
    use super::*;

    #[test]
    fn brand_parses_only_the_closed_list() {
        assert_eq!(WebServerBrand::parse("nginx").unwrap(), WebServerBrand::Nginx);
        assert_eq!(WebServerBrand::parse("apache").unwrap(), WebServerBrand::Apache);
    }

    /// The client never sends a path — only this id. An unknown id must be a
    /// validation error, NOT a silent fallback to nginx, or a typo in the UI
    /// would quietly operate on the wrong server.
    #[test]
    fn unknown_brand_is_a_validation_error_naming_the_field() {
        let e = WebServerBrand::parse("../../etc/passwd").unwrap_err();
        match e {
            IpcError::Validation { field, .. } => assert_eq!(field, "id"),
            other => panic!("expected Validation, got {other:?}"),
        }
        assert!(WebServerBrand::parse("NGINX").is_err(), "parsing must be exact-match");
        assert!(WebServerBrand::parse("").is_err());
    }

    /// Apache has no adapter and no template, so it is listed but not operable.
    /// Returning empty output instead of an error would let a UI bug render
    /// "Apache's config is empty" for "Apache has no config".
    #[test]
    fn apache_is_listed_as_unsupported_and_carries_no_paths() {
        let row = WebServerDto::apache();
        assert_eq!(row.id, "apache");
        assert!(!row.supported);
        assert!(row.binary_path.is_none());
        assert!(row.config_path.is_none());
        assert!(row.service_id.is_none());
    }

    #[test]
    fn unsupported_brand_is_rejected_before_any_path_is_touched() {
        let e = WebServerBrand::Apache.require_supported().unwrap_err();
        match e {
            IpcError::Validation { field, message } => {
                assert_eq!(field, "id");
                assert!(message.to_lowercase().contains("apache"));
            }
            other => panic!("expected Validation, got {other:?}"),
        }
        assert!(WebServerBrand::Nginx.require_supported().is_ok());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openvhost-desktop web_server_ipc 2>&1 | tail -15`

Expected: compile errors — `WebServerBrand`, `WebServerDto`, `require_supported` do not exist.

- [ ] **Step 3: Implement the brand type, DTOs and commands**

Add to `apps/desktop/src-tauri/src/commands.rs`:

```rust
use crate::stack::StackPaths;

/// The web servers OpenVHost knows about. A CLOSED list: the client sends only
/// this id — never a path, filename or argument — and every path used by the
/// commands below is derived server-side from managed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServerBrand {
    Nginx,
    Apache,
}

impl WebServerBrand {
    /// Exact match only. An unknown id is a validation error rather than a
    /// fallback: silently treating a typo as nginx would operate on a server
    /// the caller did not name.
    fn parse(s: &str) -> Result<Self, IpcError> {
        match s {
            "nginx" => Ok(Self::Nginx),
            "apache" => Ok(Self::Apache),
            _ => Err(IpcError::Validation {
                field: "id".into(),
                message: format!("unknown web server {s:?}"),
            }),
        }
    }

    fn supported(self) -> bool {
        matches!(self, Self::Nginx)
    }

    /// Reject an unsupported brand BEFORE deriving any path, so the failure is
    /// "OpenVHost cannot do this" rather than an empty read that reads as "this
    /// server has no configuration".
    fn require_supported(self) -> Result<(), IpcError> {
        if self.supported() {
            return Ok(());
        }
        Err(IpcError::Validation {
            field: "id".into(),
            message: "OpenVHost cannot serve Apache sites yet — it only generates nginx config"
                .into(),
        })
    }
}

/// One row on the Web Server page.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WebServerDto {
    pub id: String,
    pub display_name: String,
    pub supported: bool,
    /// Correlates with the shared services store for live status; `None` when
    /// the brand is not a supervised service.
    pub service_id: Option<String>,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub supports_hot_reload: bool,
    pub config_path: Option<String>,
}

impl WebServerDto {
    /// Listed so the UI can say plainly that it is not available, rather than
    /// hiding it and leaving the site editor's Apache option unexplained.
    fn apache() -> Self {
        Self {
            id: "apache".into(),
            display_name: "Apache".into(),
            supported: false,
            service_id: None,
            binary_path: None,
            version: None,
            supports_hot_reload: false,
            config_path: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReportDto {
    pub ok: bool,
    pub stderr: String,
}

/// The managed paths, or a rendered error when the home never resolved.
///
/// The managed type is `Option<StackPaths>`, not `StackPaths`: tauri implements
/// `CommandArg` only for `State<'r, T>`, so a command cannot take an
/// optionally-managed state. Task 2 therefore manages the `Option` itself.
fn stack_paths<'a>(
    paths: &'a tauri::State<'_, Option<StackPaths>>,
) -> Result<&'a StackPaths, IpcError> {
    paths.inner().as_ref().ok_or_else(|| IpcError::Core {
        message: "the OpenVHost home could not be resolved, so no web server is configured".into(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn list_web_servers(
    paths: tauri::State<'_, Option<StackPaths>>,
) -> Result<Vec<WebServerDto>, IpcError> {
    let p = stack_paths(&paths)?;
    // Probing the version SPAWNS `nginx -v`, so merely opening this page starts
    // a process. Bounded: one short-lived probe, fixed argv, PROBE_TIMEOUT.
    // Renamed from `probe_version` and given an `err_log` argument during Task 1 —
    // see that task's As-built note. `-e` is mandatory on every nginx invocation,
    // including `-v`.
    let err_log = p.home.join("logs/nginx.error.log");
    let version = openvhost_conf::probe_nginx_version(&p.nginx_bin, &err_log).await;
    Ok(vec![
        WebServerDto {
            id: "nginx".into(),
            display_name: "nginx".into(),
            supported: true,
            service_id: Some("nginx".into()),
            binary_path: Some(p.nginx_bin.display().to_string()),
            version,
            supports_hot_reload: openvhost_conf::NginxAdapter.supports_hot_reload(),
            config_path: Some(p.nginx_conf.display().to_string()),
        },
        WebServerDto::apache(),
    ])
}

#[tauri::command]
#[specta::specta]
pub async fn read_web_server_config(
    paths: tauri::State<'_, Option<StackPaths>>,
    id: String,
) -> Result<String, IpcError> {
    let brand = WebServerBrand::parse(&id)?;
    brand.require_supported()?;
    let p = stack_paths(&paths)?;
    // NOT a general file reader: the path comes from managed state keyed by the
    // parsed brand, so it cannot be aimed at an arbitrary file.
    std::fs::read_to_string(&p.nginx_conf).map_err(|e| IpcError::Core {
        message: format!("cannot read {}: {e}", p.nginx_conf.display()),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn validate_web_server_config(
    paths: tauri::State<'_, Option<StackPaths>>,
    id: String,
) -> Result<ValidationReportDto, IpcError> {
    let brand = WebServerBrand::parse(&id)?;
    brand.require_supported()?;
    let p = stack_paths(&paths)?;
    let err_log = p.home.join("logs/nginx.error.log");
    let report = openvhost_conf::validate_live(&p.nginx_bin, &p.nginx_conf, &err_log)
        .await
        .map_err(|e| IpcError::Core { message: e.to_string() })?;
    Ok(ValidationReportDto { ok: report.ok, stderr: report.stderr })
}
```

`WebServerAdapter` must be in scope for `supports_hot_reload()` — add it to the `openvhost_conf` import if the file does not already import it.

Add the three commands to `collect_commands!` in `apps/desktop/src-tauri/src/lib.rs`, after `commands::delete_site`:

```rust
            commands::list_web_servers,
            commands::read_web_server_config,
            commands::validate_web_server_config,
```

Add `openvhost-conf` to `apps/desktop/src-tauri/Cargo.toml`'s `[dependencies]` if absent, matching how sibling workspace crates are declared there.

- [ ] **Step 4: Run the tests, then regenerate bindings**

```bash
cargo test -p openvhost-desktop web_server_ipc 2>&1 | grep -E "test result|FAILED"
cargo test -p openvhost-desktop export_bindings 2>&1 | grep -E "test result|FAILED"
```

Expected: both `test result: ok.`. The `export_bindings` test regenerates `apps/desktop/src/lib/ipc/bindings.ts`; confirm it now contains the three new commands and both new types:

```bash
grep -cE "listWebServers|readWebServerConfig|validateWebServerConfig" apps/desktop/src/lib/ipc/bindings.ts
```

Expected: `3` or more. Never hand-edit that file.

- [ ] **Step 5: Commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src-tauri/Cargo.toml apps/desktop/src/lib/ipc/bindings.ts
git commit -s -m "feat(ipc): three read-only web-server inspection commands

The client sends only an opaque brand id parsed against a closed list — never a
path, filename or argument — and every path is derived server-side from the
StackPaths the app manages. read_web_server_config is therefore not a general
file reader.

An unknown id is a validation error rather than a fallback to nginx: silently
treating a typo as nginx would operate on a server the caller did not name. An
unsupported brand is rejected BEFORE any path is derived, so Apache fails as
'OpenVHost cannot do this' instead of an empty read that reads as 'this server
has no configuration'.

Note list_web_servers probes the version, so opening the page spawns nginx -v —
the spawn surface is navigation, not only the Validate button."
```

---

## Task 4: frontend data layer

**Files:**
- Create: `apps/desktop/src/lib/webservers.derive.ts`
- Create: `apps/desktop/src/lib/webservers.svelte.ts`
- Modify: `apps/desktop/src/lib/ipc/index.ts` (typed wrappers)
- Test: `apps/desktop/src/lib/webservers.derive.test.ts`, `apps/desktop/src/lib/webservers.svelte.test.ts`

**Interfaces:**
- Consumes: `listWebServers`, `readWebServerConfig`, `validateWebServerConfig` from `bindings.ts` (Task 3), wrapped in `$lib/ipc` following exactly how `listSites`/`createSite` are wrapped there today; the shared `servicesStore` from `$lib/services.shared.svelte`.
- Produces, for Task 5:
  ```ts
  // webservers.derive.ts
  export function statusFor(services: ServiceStatus[], serviceId: string | null): ServiceStatus['state']['kind'] | null
  export function hotReloadLabel(supportsHotReload: boolean): string
  // webservers.svelte.ts
  class WebServersStore {
    servers: WebServerDto[]; error: IpcError | null;
    configText: Record<string, string>; configError: Record<string, string>;
    reports: Record<string, ValidationReportDto>; validating: Record<string, boolean>;
    load(): Promise<void>;
    showConfig(id: string): Promise<void>;
    validate(id: string): Promise<void>;
  }
  export const webServersStore: WebServersStore
  ```

- [ ] **Step 1: Write the failing tests**

Create `apps/desktop/src/lib/webservers.derive.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { statusFor, hotReloadLabel } from './webservers.derive';
import type { ServiceStatus } from '$lib/ipc';
// NOTE: `ServiceState` is NOT exported from `$lib/ipc` (only `ServiceStateEvent`
// and `ServiceStatus`), and `StatusPill` takes `kind: StateKind` rather than a
// state object — so `statusFor` returns the kind STRING, indexed off the
// exported `ServiceStatus` type. That satisfies both without touching the barrel.

const svc = (id: string, kind: 'running' | 'stopped'): ServiceStatus => ({
	id,
	displayName: id,
	endpoint: null,
	pid: kind === 'running' ? 1 : null,
	state: { kind }
});

describe('statusFor', () => {
	it('finds the supervised service a row correlates with', () => {
		expect(statusFor([svc('nginx', 'running')], 'nginx')).toBe('running');
	});

	// Apache has no supervised service, so a row with no serviceId must render
	// "no status" rather than borrowing another row's state.
	it('is null for a row that is not a supervised service', () => {
		expect(statusFor([svc('nginx', 'running')], null)).toBeNull();
	});

	it('is null when the service is not in the snapshot yet', () => {
		expect(statusFor([], 'nginx')).toBeNull();
	});
});

describe('hotReloadLabel', () => {
	it('states support plainly in both directions', () => {
		expect(hotReloadLabel(true)).toBe('Supported');
		expect(hotReloadLabel(false)).toBe('Not supported');
	});
});
```

Create `apps/desktop/src/lib/webservers.svelte.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from 'vitest';
import { WebServersStore, type WebServersApi } from './webservers.svelte';
import type { WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	supportsHotReload: true,
	configPath: '/home/.openvhost/conf/nginx.conf'
};

function api(over: Partial<WebServersApi> = {}): WebServersApi {
	return {
		listWebServers: vi.fn(async () => [nginx]),
		readWebServerConfig: vi.fn(async () => 'daemon off;'),
		validateWebServerConfig: vi.fn(async () => ({ ok: true, stderr: 'syntax is ok' })),
		...over
	};
}

describe('WebServersStore', () => {
	it('loads rows', async () => {
		const store = new WebServersStore(api());
		await store.load();
		expect(store.servers).toEqual([nginx]);
		expect(store.error).toBeNull();
	});

	it('renders a load failure instead of showing an empty page', async () => {
		const store = new WebServersStore(
			api({ listWebServers: vi.fn(async () => { throw { kind: 'core', message: 'boom' }; }) })
		);
		await store.load();
		expect(store.error).toEqual({ kind: 'core', message: 'boom' });
		expect(store.servers).toEqual([]);
	});

	it('keeps a config read failure on the row, not the page banner', async () => {
		const store = new WebServersStore(
			api({ readWebServerConfig: vi.fn(async () => { throw { kind: 'core', message: 'no such file' }; }) })
		);
		await store.showConfig('nginx');
		expect(store.configError.nginx).toContain('no such file');
		// A per-row failure must not blank the whole page.
		expect(store.error).toBeNull();
	});

	it('exposes the validator stderr verbatim', async () => {
		const store = new WebServersStore(
			api({
				validateWebServerConfig: vi.fn(async () => ({
					ok: false,
					stderr: 'nginx: [emerg] unknown directive "bogus"'
				}))
			})
		);
		await store.validate('nginx');
		expect(store.reports.nginx.ok).toBe(false);
		expect(store.reports.nginx.stderr).toBe('nginx: [emerg] unknown directive "bogus"');
	});

	// A spawn failure is an IpcError, not a report. It must still surface — and it
	// must land on the ROW, so assert that channel specifically rather than
	// accepting either one (an assertion that can pass two ways pins neither).
	it('surfaces a validator that could not be launched, on the row', async () => {
		const store = new WebServersStore(
			api({ validateWebServerConfig: vi.fn(async () => { throw { kind: 'core', message: 'could not be launched' }; }) })
		);
		await store.validate('nginx');
		expect(store.configError.nginx).toContain('could not be launched');
		expect(store.error).toBeNull();
		expect(store.reports.nginx).toBeUndefined();
	});

	it('clears the validating flag even when validation throws', async () => {
		const store = new WebServersStore(
			api({ validateWebServerConfig: vi.fn(async () => { throw { kind: 'core', message: 'x' }; }) })
		);
		await store.validate('nginx');
		expect(store.validating.nginx).not.toBe(true);
	});
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm -C apps/desktop test src/lib/webservers 2>&1 | tail -15`

Expected: failure — the modules do not exist (`Failed to resolve import`).

- [ ] **Step 3: Implement**

Create `apps/desktop/src/lib/webservers.derive.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import type { ServiceStatus } from '$lib/ipc';

/**
 * The supervised state-kind a row shows, or `null` when the row has no
 * supervised service (Apache) or the snapshot has not arrived yet. Never falls
 * back to another row's state — a row showing a neighbour's status would be a
 * lie about what is running.
 *
 * Returns the KIND rather than the whole state object for two reasons:
 * `ServiceState` is not exported from `$lib/ipc`, and `StatusPill` takes
 * `kind: StateKind`. Indexing off the exported `ServiceStatus` keeps this in
 * step with the binding without widening the barrel.
 */
export function statusFor(
	services: ServiceStatus[],
	serviceId: string | null
): ServiceStatus['state']['kind'] | null {
	if (serviceId === null) return null;
	return services.find((s) => s.id === serviceId)?.state.kind ?? null;
}

export function hotReloadLabel(supportsHotReload: boolean): string {
	return supportsHotReload ? 'Supported' : 'Not supported';
}
```

Create `apps/desktop/src/lib/webservers.svelte.ts`, following the api-injected shape of `sites.svelte.ts` so the tests above can fake IPC:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import {
	listWebServers,
	readWebServerConfig,
	validateWebServerConfig,
	type IpcError,
	type ValidationReportDto,
	type WebServerDto
} from '$lib/ipc';

export interface WebServersApi {
	listWebServers: () => Promise<WebServerDto[]>;
	readWebServerConfig: (id: string) => Promise<string>;
	validateWebServerConfig: (id: string) => Promise<ValidationReportDto>;
}

export class WebServersStore {
	servers = $state<WebServerDto[]>([]);
	/** Page-level failure (the list could not be loaded at all). */
	error = $state<IpcError | null>(null);
	configText = $state<Record<string, string>>({});
	/** PER-ROW failure. Kept off `error` so one row's problem cannot blank the page. */
	configError = $state<Record<string, string>>({});
	reports = $state<Record<string, ValidationReportDto>>({});
	validating = $state<Record<string, boolean>>({});

	constructor(private api: WebServersApi) {}

	async load(): Promise<void> {
		this.error = null;
		try {
			this.servers = await this.api.listWebServers();
		} catch (e) {
			this.error = e as IpcError;
		}
	}

	async showConfig(id: string): Promise<void> {
		this.configError = { ...this.configError, [id]: '' };
		try {
			const text = await this.api.readWebServerConfig(id);
			this.configText = { ...this.configText, [id]: text };
		} catch (e) {
			const message = (e as IpcError & { message?: string }).message ?? String(e);
			this.configError = { ...this.configError, [id]: message };
		}
	}

	async validate(id: string): Promise<void> {
		this.validating = { ...this.validating, [id]: true };
		this.configError = { ...this.configError, [id]: '' };
		try {
			const report = await this.api.validateWebServerConfig(id);
			this.reports = { ...this.reports, [id]: report };
		} catch (e) {
			// A validator that could not even be launched is an IpcError, not a
			// report. It must still reach the row rather than vanishing.
			const message = (e as IpcError & { message?: string }).message ?? String(e);
			this.configError = { ...this.configError, [id]: message };
		} finally {
			this.validating = { ...this.validating, [id]: false };
		}
	}
}

export const webServersStore = new WebServersStore({
	listWebServers,
	readWebServerConfig,
	validateWebServerConfig
});
```

Add the three wrappers to `apps/desktop/src/lib/ipc/index.ts` following exactly the existing `listSites` pattern there (each unwraps the generated result and normalizes rejections to `IpcError`), and re-export `WebServerDto` / `ValidationReportDto` from `bindings.ts` the way `SiteDto` is re-exported.

- [ ] **Step 4: Run to verify they pass**

Run: `pnpm -C apps/desktop test src/lib/webservers 2>&1 | grep -E "Tests |FAIL"`

Expected: `Tests  10 passed (10)`.

- [ ] **Step 5: Commit**

```bash
pnpm -C apps/desktop lint && pnpm -C apps/desktop check
git add apps/desktop/src/lib/webservers.derive.ts apps/desktop/src/lib/webservers.derive.test.ts apps/desktop/src/lib/webservers.svelte.ts apps/desktop/src/lib/webservers.svelte.test.ts apps/desktop/src/lib/ipc/index.ts
git commit -s -m "feat(ui): web-server data layer — derive helpers and store

Per-row failures (config read, validator launch) live on configError rather than
the page-level error, so one row's problem cannot blank the whole page. A
validator that could not be launched is an IpcError rather than a report, and is
routed to the row so it cannot vanish. statusFor never falls back to another
row's state — a row showing a neighbour's status would misreport what is
running."
```

---

## Task 5: the page, the row, and the rail entry

**Files:**
- Create: `apps/desktop/src/lib/components/WebServerRow.svelte`
- Create: `apps/desktop/src/lib/components/WebServerPanel.svelte`
- Create: `apps/desktop/src/routes/web-server/+page.svelte`
- Modify: `apps/desktop/src/lib/components/Rail.svelte`
- Modify: `apps/desktop/src/lib/components/AppShell.svelte`
- Test: `apps/desktop/src/lib/components/webserver.panel.test.ts`

**Interfaces:**
- Consumes: `webServersStore` and the derive helpers (Task 4); `servicesStore` from `$lib/services.shared.svelte`; existing `StatusPill.svelte` and `Button.svelte`.
- Produces: the `/web-server` route and a rail item pointing at it.

**Reference:** style the row on the existing `ServiceRow.svelte` / `SiteListRow.svelte` treatments and `docs/design/mock.css`'s `.row`/`.panel`/`.mono` recipes. Tokens only.

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/lib/components/webserver.panel.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
//
// SSR-rendered (`svelte/server`), so no DOM is needed and this runs in the
// existing `node` vitest project. Interactive behaviour — the config disclosure
// toggling, the Validate round-trip — is out of reach here and is on the PR's
// click-through list.
import { beforeEach, describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import WebServerPanel from './WebServerPanel.svelte';
import type { WebServerDto } from '$lib/ipc';

const nginx: WebServerDto = {
	id: 'nginx',
	displayName: 'nginx',
	supported: true,
	serviceId: 'nginx',
	binaryPath: '/opt/homebrew/opt/nginx/bin/nginx',
	version: '1.27.3',
	supportsHotReload: true,
	configPath: '/x/.openvhost/conf/nginx.conf'
};
const apache: WebServerDto = {
	id: 'apache',
	displayName: 'Apache',
	supported: false,
	serviceId: null,
	binaryPath: null,
	version: null,
	supportsHotReload: false,
	configPath: null
};

function html(props: Record<string, unknown>): string {
	return render(WebServerPanel, {
		props: {
			servers: [nginx, apache],
			services: [],
			configText: {},
			configError: {},
			reports: {},
			validating: {},
			onShowConfig: () => {},
			onValidate: () => {},
			...props
		}
	}).body;
}

function text(s: string): string {
	return s.replace(/<[^>]*>/g, '').replace(/\s+/g, ' ').trim();
}

describe('WebServerPanel', () => {
	it('shows the resolved binary, version and config path for nginx', () => {
		const t = text(html({}));
		expect(t).toContain('/opt/homebrew/opt/nginx/bin/nginx');
		expect(t).toContain('1.27.3');
		expect(t).toContain('/x/.openvhost/conf/nginx.conf');
	});

	// An unknown version must read as unknown, not as an empty gap the user
	// cannot interpret.
	it('says the version is unknown rather than rendering a blank', () => {
		const t = text(html({ servers: [{ ...nginx, version: null }] }));
		expect(t.toLowerCase()).toContain('unknown');
	});

	it('states plainly that Apache is not served yet', () => {
		expect(text(html({})).toLowerCase()).toContain('cannot serve apache');
	});

	it('offers neither the config view nor Validate for an unsupported brand', () => {
		const body = html({ servers: [apache] });
		expect(body).not.toContain('data-testid="validate-apache"');
		expect(body).not.toContain('data-testid="show-config-apache"');
	});

	it('offers both for nginx', () => {
		const body = html({ servers: [nginx] });
		expect(body).toContain('data-testid="validate-nginx"');
		expect(body).toContain('data-testid="show-config-nginx"');
	});

	it('renders a per-row failure on that row', () => {
		const t = text(html({ configError: { nginx: 'cannot read /x/nginx.conf' } }));
		expect(t).toContain('cannot read /x/nginx.conf');
	});

	// nginx's own diagnostic is the useful part; it must not be summarized away.
	it('shows the validator stderr verbatim', () => {
		const t = text(
			html({ reports: { nginx: { ok: false, stderr: 'nginx: [emerg] unknown directive "bogus"' } } })
		);
		expect(t).toContain('unknown directive');
	});

	it('shows the config text once it has been read', () => {
		const t = text(html({ configText: { nginx: 'daemon off; worker_processes 1;' } }));
		expect(t).toContain('worker_processes 1;');
	});
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `pnpm -C apps/desktop test src/lib/components/webserver.panel 2>&1 | tail -10`

Expected: failure — `WebServerPanel.svelte` does not exist.

- [ ] **Step 3: Implement the components**

Create `WebServerRow.svelte` and `WebServerPanel.svelte`. `WebServerPanel` is presentational: it takes `servers`, `services`, `configText`, `configError`, `reports`, `validating`, `onShowConfig`, `onValidate` as props (that is what makes the SSR test above possible) and renders a page head plus one `WebServerRow` per server.

Each row must render:
- the display name, and `<StatusPill kind={k} />` when `const k = statusFor(services, serviceId)` is non-null (it returns the kind string `StatusPill` expects, not a state object);
- `Version`, showing the string or the literal word `Unknown` when `version === null`;
- `Hot reload`, via `hotReloadLabel`;
- binary path and config path in the `.mono` treatment;
- for a supported brand only: a `Show config` control with `data-testid="show-config-{id}"` and a `Validate` button with `data-testid="validate-{id}"`, the latter disabled while `validating[id]`;
- `configText[id]` in a `<pre>` when present, and `configError[id]` in the `.field-error` treatment when non-empty;
- `reports[id]` as an ok/failed line plus its `stderr` in a `<pre>` — verbatim;
- for an unsupported brand: the sentence `OpenVHost cannot serve Apache sites yet — it only generates nginx config.`, matching the site editor's existing hint so the product says one thing in both places.

Create `apps/desktop/src/routes/web-server/+page.svelte`, following `routes/services/+page.svelte`'s shape: render inside `AppShell` with `active="web-server"`, an `sr-only` `<h1>OpenVHost — Web Server</h1>`, call `webServersStore.load()` in `onMount`, and pass `servicesStore.services` down for status.

In `AppShell.svelte`, widen the prop union to `active?: 'services' | 'sites' | 'web-server'`. In `Rail.svelte`, widen the same union and turn Web Server into a real `<a>` with `href={resolve('/web-server')}` and `aria-current={active === 'web-server' ? 'page' : undefined}`, placed after Services, keeping Logs and Settings inert. Use an existing-style inline SVG icon consistent with the other rail glyphs.

- [ ] **Step 4: Run to verify it passes**

Run: `pnpm -C apps/desktop test src/lib/components/webserver.panel 2>&1 | grep -E "Tests |FAIL"`

Expected: `Tests  8 passed (8)`.

- [ ] **Step 5: Run the full gate and commit**

```bash
pnpm -C apps/desktop lint; echo "lint EXIT=$?"
pnpm -C apps/desktop check 2>&1 | tail -1
pnpm -C apps/desktop test 2>&1 | grep -E "Test Files|Tests "
pnpm -C apps/desktop build 2>&1 | grep -E "Wrote site"
ls apps/desktop/build/*.html   # index.html, services.html AND web-server.html
bash scripts/check-spdx.sh
git add apps/desktop/src/lib/components/WebServerRow.svelte apps/desktop/src/lib/components/WebServerPanel.svelte apps/desktop/src/lib/components/webserver.panel.test.ts apps/desktop/src/routes/web-server/+page.svelte apps/desktop/src/lib/components/Rail.svelte apps/desktop/src/lib/components/AppShell.svelte
git commit -s -m "feat(ui): Web Server page and rail entry

Presentational panel + row take everything as props, which is what lets the SSR
test assert the whole surface without a DOM. Status comes from the shared
services store, so there is no second status source to drift.

An unknown version renders the word Unknown rather than a blank gap, the
validator's stderr is shown verbatim because nginx's own diagnostic is the
useful part, and an unsupported brand offers neither the config view nor
Validate while stating why in the same words the site editor uses."
```

---

## Task 6: gate suite, security-auditor package, PR

**Files:**
- Modify: `docs/OPENVHOST_MASTER_PLAN.md` only if it lists the command surface and is now stale (check; do not invent an edit).

- [ ] **Step 1: Run the complete gate suite from raw pipes**

```bash
cargo fmt --check && echo FMT_OK
cargo clippy --workspace --all-targets -- -D warnings; echo "clippy EXIT=$?"
cargo test --workspace > /tmp/ct.txt 2>&1; echo "cargo test EXIT=$?"; grep -c "test result: ok" /tmp/ct.txt
cargo deny check licenses advisories
pnpm -C apps/desktop lint; echo "lint EXIT=$?"
pnpm -C apps/desktop check 2>&1 | tail -1
pnpm -C apps/desktop test 2>&1 | grep -E "Test Files|Tests "
pnpm -C apps/desktop build 2>&1 | grep -E "Wrote site"
bash scripts/check-spdx.sh
```

Every one must pass. `cargo deny` matters because Task 3 adds `openvhost-conf` to the desktop crate's dependencies, which changes `Cargo.lock`.

- [ ] **Step 2: Verify the security-relevant invariants by hand, and record the evidence**

These are the claims the auditor will check; verify them yourself first so the review starts from facts:

```bash
# No materialize anywhere in this slice.
git diff main...HEAD | grep -n "materialize" || echo "no materialize — correct"
# Every nginx invocation passes -e.
grep -n '"-e"\|arg("-e")\|\.arg("-e")' crates/openvhost-conf/src/inspect.rs
# No unwrap/expect outside cfg(test) in the new Rust.
grep -nE "\.unwrap\(\)|\.expect\(" crates/openvhost-conf/src/inspect.rs apps/desktop/src-tauri/src/commands.rs | grep -v "cfg(test)"
# The capability file must be UNCHANGED — this slice adds no capability.
git diff main...HEAD -- apps/desktop/src-tauri/capabilities/ | wc -l    # expect 0
# bindings.ts changed only by regeneration (3 commands + 2 types).
git diff main...HEAD -- apps/desktop/src/lib/ipc/bindings.ts | head -40
```

- [ ] **Step 3: Open the PR**

Follow `.github/PULL_REQUEST_TEMPLATE.md`. The body must state:
- what shipped, and that it is read-only;
- the §2 finding (live vs generated config) as the reason the page shows the live file;
- that **`list_web_servers` spawns `nginx -v`, so navigation — not only the Validate button — causes a spawn**;
- the golden-rule-4 reading (one-shot tool invocations spawn directly, per the shipped P0-7 validator; supervised services go through `openvhost-proc`) flagged for the auditor to confirm or reject;
- that no capability changed and no `materialize` is called;
- the owed human click-through: the rail entry navigates; nginx's real version appears; Show config reveals the live file; Validate on a good config reports ok; Validate on a deliberately broken config shows nginx's error; Apache offers neither control and explains itself.
- Check the **Security-sensitive paths touched → security-auditor APPROVE linked** box as pending.

- [ ] **Step 4: Dispatch the security-auditor**

Merge-blocking per CLAUDE.md golden rule 2 (new IPC commands; UI-triggered process spawn). Give it the base/head range, and ask it specifically about: the closed-list brand parsing and whether any client input can reach argv or a path; whether `read_web_server_config` can be aimed at an arbitrary file; the spawn surface including the navigation-time probe; the timeout and `kill_on_drop`; the mandatory `-e`; the golden-rule-4 reading; and whether returning `stderr` verbatim to the webview can leak anything sensitive.

- [ ] **Step 5: Address findings, then request the merge decision from the owner**

---

## Self-Review

**Spec coverage.** §1 in-scope items → Tasks 1, 3, 5. §1 out-of-scope items → not implemented anywhere (verified: no settings persistence, no diff, no `ApacheAdapter`). §2's live-config decision → Task 3's `read_web_server_config` reads `p.nginx_conf` (the registered live path) and Task 1's `validate_live` avoids `materialize`. §3.1 → Task 1, including the mandatory `-e`, stderr-derived version, timeout, and the golden-rule-4 note carried into Task 6's auditor brief. §3.2 → Task 3, including the unsupported-brand rejection. §3.3's status reuse → Task 4's `statusFor` + Task 5's props; its binary-path correction → Task 2. §4 → Task 5. §5 → tests in every task, with the untestable set named in Task 5's test header and Task 6's click-through. §6 → Task 6. §7's five follow-ups → deliberately not implemented; Task 6 does not touch them.

**Placeholder scan.** One deliberate `todo!()` in Task 1 Step 1, explicitly scoped to that step and required gone by Step 3 — it is the RED state, not a placeholder. Task 6 Step 1 says "check; do not invent an edit" for the master plan rather than asserting an edit exists. No "add appropriate error handling"-style instructions: every error path names its variant and its destination.

**Type consistency.** `ValidationReport { ok, stderr }` (Rust, existing) → `ValidationReportDto { ok, stderr }` (Task 3) → `ValidationReportDto` in TS (Task 4) → `reports[id]` (Task 5): consistent. `WebServerDto`'s snake_case Rust fields with `#[serde(rename_all = "camelCase")]` produce the camelCase TS names used in Tasks 4 and 5 — checked field by field: `display_name`/`displayName`, `service_id`/`serviceId`, `binary_path`/`binaryPath`, `supports_hot_reload`/`supportsHotReload`, `config_path`/`configPath`. `StackPaths { home, nginx_bin, nginx_conf }` defined in Task 2 and consumed with those exact names in Task 3. `statusFor`/`hotReloadLabel` defined in Task 4 and used with those names in Task 5. `PROBE_TIMEOUT`, `probe_nginx_version`, `validate_live`, `ConfError::ValidatorTimeout` defined in Task 1 and used in Task 3 and Task 6's verification step.

**One gap found and fixed during review:** Task 3's `list_web_servers` calls `NginxAdapter.supports_hot_reload()`, which requires the `WebServerAdapter` trait in scope — an easy compile error to hit and a confusing one. Task 3 Step 3 now says so explicitly.
