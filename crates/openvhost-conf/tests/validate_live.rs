// SPDX-License-Identifier: GPL-3.0-or-later
//! Exit-criterion proof (master plan P0-7): the generated stack passes the
//! native validators on real Homebrew nginx + php-fpm. Auto-skips (loudly)
//! when the binaries are absent. The temp home path deliberately CONTAINS A
//! SPACE to prove the quoting rule (nginx splits unquoted whitespace) end to
//! end, including quoted `include` globs.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use openvhost_conf::{
    BodySize, GzipLevel, GzipTypes, NginxAdapter, OnOff, PhpFpmRuntime, PhpRuntimeAdapter,
    PhpUpstream, RenderCtx, Seconds, WebServerAdapter, WebServerSettings, WorkerConnections,
    find_brew_binaries, probe_nginx_version, validate_live,
};

/// The world both cases need: a temp home whose path CONTAINS A SPACE (see the
/// module doc) plus the `RenderCtx` pointing at it. Returns the `TempDir` so the
/// caller keeps it alive — dropping it deletes the home mid-test.
fn temp_home_ctx() -> (tempfile::TempDir, RenderCtx) {
    // Short /tmp base (sun_path headroom) with a SPACE in the dir name.
    let base = tempfile::Builder::new()
        .prefix("ovh conf ") // <- space is intentional
        .tempdir_in("/tmp")
        .unwrap();
    let home = base.path().to_path_buf();
    let socket = home.join("run/php-fpm.sock");
    let ctx = RenderCtx::new(
        home.clone(),
        "myapp.localhost",
        home.join("www"),
        "127.0.0.1:8080".parse().unwrap(),
        "8.4",
        PhpUpstream::UnixSocket(socket),
        "php_myapp",
    )
    .unwrap();
    (base, ctx)
}

/// Write the main + site config generated from `settings` into `ctx.home`,
/// create the directories `nginx -t` needs, and return the main config's path.
///
/// `NginxAdapter::validate` cannot serve this: it renders with
/// `WebServerSettings::default()` on purpose (it answers "is the SHAPE
/// valid?"), so a non-default settings value never reaches a real nginx
/// through it. In production those values reach nginx through the apply
/// pipeline's `validate_live` on the installed file, which is what this
/// mirrors.
fn materialize_with(ctx: &RenderCtx, settings: &WebServerSettings) -> std::path::PathBuf {
    let main = NginxAdapter
        .generate_main_config(&ctx.home, settings)
        .unwrap();
    let site = NginxAdapter.generate_site_config(ctx).unwrap();
    for f in [&main, &site] {
        std::fs::create_dir_all(f.path.parent().unwrap()).unwrap();
        std::fs::write(&f.path, &f.contents).unwrap();
    }
    for d in ["run", "run/nginx", "logs"] {
        std::fs::create_dir_all(ctx.home.join(d)).unwrap();
    }
    // P1 live-log-viewer: the site config's `access_log`/`error_log` now
    // point under `logs/sites/<domain>/` (mirroring `webserver.rs`'s own
    // `NginxAdapter::validate`, which this helper otherwise stands in for —
    // see the doc comment above). Literal, not a shared helper: this whole
    // function already hardcodes the nginx-globals formula the same way
    // (`ctx.home.join("logs/nginx.error.log")`, repeated at every call site
    // below) — scratch plumbing for a throwaway validation home, not the
    // live path `openvhost_core::logs::LogPaths` owns.
    std::fs::create_dir_all(ctx.home.join("logs/sites").join(&ctx.server_name)).unwrap();
    main.path
}

#[tokio::test]
async fn generated_stack_passes_native_validators() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP validate_live: Homebrew nginx/php-fpm not found (brew install nginx php)");
        return;
    };

    let (_home, ctx) = temp_home_ctx();

    let nginx_report = NginxAdapter.validate(&brew.nginx, &ctx).await.unwrap();
    assert!(nginx_report.ok, "nginx -t failed:\n{}", nginx_report.stderr);

    let fpm_report = PhpFpmRuntime.validate(&brew.php_fpm, &ctx).await.unwrap();
    assert!(fpm_report.ok, "php-fpm -t failed:\n{}", fpm_report.stderr);

    // The php-fpm empty-glob WARNING is expected and must NOT flip ok.
    // (No assertion on stderr emptiness — that is the whole point.)

    // A zero-match `include` glob also passes plain `-t` silently, so `-t`
    // alone can't prove the main->site include seam actually expanded. `-T`
    // test-and-dumps the fully resolved config to stdout instead.
    let main = NginxAdapter
        .generate_main_config(&ctx.home, &WebServerSettings::default())
        .unwrap();
    let err_log = ctx.home.join("logs/nginx.error.log");
    let dump = tokio::process::Command::new(&brew.nginx)
        .arg("-e")
        .arg(&err_log)
        .arg("-T")
        .arg("-c")
        .arg(&main.path)
        .output()
        .await
        .unwrap();
    assert!(
        dump.status.success(),
        "nginx -T failed:\n{}",
        String::from_utf8_lossy(&dump.stderr)
    );
    let dump_out = String::from_utf8_lossy(&dump.stdout);
    assert!(
        dump_out.contains("server_name myapp.localhost"),
        "nginx -T did not show the expanded site include:\n{dump_out}"
    );
    // Live proof for the `map`/named-capture regex specifically (review
    // finding, P1 live-log-viewer): `dump.status.success()` above already
    // proves nginx accepted `map $request_uri $request_path { ~^(?<p>[^?]*)
    // $p; }` as valid syntax -- a bad regex or an unsupported named-capture
    // form is exactly the kind of error nginx -t/-T refuses, not one a unit
    // test rendering a string could ever catch. This goes further and reads
    // nginx's OWN resolved view back, so a map that parsed but silently
    // bound the wrong variable (or was dropped) would still be caught.
    assert!(
        dump_out.contains("map $request_uri $request_path"),
        "nginx -T did not show the map that derives $request_path from \
         $request_uri:\n{dump_out}"
    );
    assert!(
        dump_out.contains("$request_path"),
        "nginx -T did not show log_format referencing $request_path:\n{dump_out}"
    );
}

/// REAL-NGINX proof for condition (6) of `inspect`'s golden-rule-4 reading:
/// `run_bounded` now does `env_clear()` + an allowlist, so both probes run in an
/// assembled environment rather than the app's.
///
/// This needs its own case because the test above CANNOT cover it: that one goes
/// through `webserver.rs`'s `NginxAdapter::validate`, which spawns with its own
/// untimed `.output()` and never touches `run_bounded`. Clearing the environment
/// is the kind of change that can only be disproved by a real binary — a fake
/// `#!/bin/sh` script needs nothing but `PATH` — so if nginx ever does need a
/// variable the allowlist does not carry, THIS is the test that says so instead
/// of a user seeing "Config is not valid" for a config that is fine.
///
/// Both probes are exercised, because both go through `run_bounded`.
#[tokio::test]
async fn both_probes_pass_real_nginx_in_the_assembled_environment() {
    // Deliberately FAILS rather than skipping when the binaries are absent, unlike its
    // sibling above. This is the only check that proves a real nginx still works in the
    // cleared environment `probe_env()` assembles — so if it silently no-ops, the gate
    // reports green for the one thing it was added to verify. GitHub CI is disabled on
    // this repo and local gates ARE the merge gate, so a skip here is indistinguishable
    // from a pass by anyone reading the suite output. Flagged by the security-auditor at
    // the gate that required the environment change.
    let Some(brew) = find_brew_binaries() else {
        panic!(
            "Homebrew nginx/php-fpm not found, so the cleared-environment check could not run. \
             This test must not skip: it is the only proof that env_clear() + probe_env() does \
             not break a real nginx. Install them (brew install nginx php) and re-run."
        );
    };

    let (_home, ctx) = temp_home_ctx();
    // `validate_live` validates a config that ALREADY EXISTS and writes nothing,
    // so the generated files have to be on disk first. `NginxAdapter::validate`
    // materializes them as a side effect, which is exactly the seam under test:
    // the same bytes, the same binary, a different environment.
    let generated = NginxAdapter.validate(&brew.nginx, &ctx).await.unwrap();
    assert!(
        generated.ok,
        "precondition: the generated config must be valid before this proves \
         anything about the environment:\n{}",
        generated.stderr
    );
    let main = NginxAdapter
        .generate_main_config(&ctx.home, &WebServerSettings::default())
        .unwrap();
    let err_log = ctx.home.join("logs/nginx.error.log");

    let live = validate_live(&brew.nginx, &main.path, &err_log, &ctx.home)
        .await
        .unwrap();
    assert!(
        live.ok,
        "real `nginx -t` FAILED under the assembled probe environment while the \
         same config passed under the inherited one — nginx needs an environment \
         variable `inspect::probe_env`'s allowlist does not carry. Report this \
         rather than widening the allowlist blind:\n{}",
        live.stderr
    );

    let version = probe_nginx_version(&brew.nginx, &err_log, &ctx.home).await;
    assert!(
        version.is_some(),
        "real `nginx -v` produced no parseable banner under the assembled probe \
         environment, so every row would read `Version: Unknown`"
    );
}

/// The settings the Web server page edits are rendered as directives at
/// specific scopes, and only a REAL nginx can say whether that placement is
/// legal. Every unit test in `webserver.rs` asserts on the string this crate
/// produced — none of them can tell a valid config from one nginx refuses,
/// which is the entire failure this layer exists to prevent: the newtypes
/// guarantee the VALUES are well formed, the template decides where they GO.
///
/// Non-default across the board (including gzip on with a custom type list and
/// a 900s read timeout), because the default set is the one path most likely to
/// be exercised by accident elsewhere.
#[tokio::test]
async fn non_default_settings_pass_real_nginx() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP non_default_settings_pass_real_nginx: brew nginx not found");
        return;
    };
    let (_home, ctx) = temp_home_ctx();
    let settings = WebServerSettings {
        worker_connections: WorkerConnections::parse(4096).unwrap(),
        client_max_body_size: BodySize::parse("512m").unwrap(),
        keepalive_timeout: Seconds::parse(15).unwrap(),
        tcp_nodelay: OnOff::new(false),
        fastcgi_connect_timeout: Seconds::parse(900).unwrap(),
        fastcgi_send_timeout: Seconds::parse(900).unwrap(),
        fastcgi_read_timeout: Seconds::parse(900).unwrap(),
        gzip: OnOff::new(true),
        gzip_comp_level: GzipLevel::parse(9).unwrap(),
        gzip_types: GzipTypes::parse("text/x-component application/vnd.ms-fontobject").unwrap(),
    };
    let main_path = materialize_with(&ctx, &settings);
    let err_log = ctx.home.join("logs/nginx.error.log");

    let report = validate_live(&brew.nginx, &main_path, &err_log, &ctx.home)
        .await
        .unwrap();
    assert!(
        report.ok,
        "real `nginx -t` REJECTED a config built from settings every newtype \
         accepted — the values parse but the template places them wrongly:\n{}",
        report.stderr
    );

    // `-t` accepts a directive at a legal-but-wrong scope as readily as at the
    // right one only when both are legal; it does NOT prove the values landed.
    // Dump the resolved config and read them back.
    let dump = tokio::process::Command::new(&brew.nginx)
        .arg("-e")
        .arg(&err_log)
        .arg("-T")
        .arg("-c")
        .arg(&main_path)
        .output()
        .await
        .unwrap();
    assert!(dump.status.success());
    let out = String::from_utf8_lossy(&dump.stdout);
    for expected in [
        "worker_connections 4096;",
        "client_max_body_size 512m;",
        "keepalive_timeout 15;",
        "tcp_nodelay off;",
        "fastcgi_read_timeout 900;",
        "gzip on;",
        "gzip_comp_level 9;",
        "gzip_types text/x-component application/vnd.ms-fontobject;",
    ] {
        assert!(
            out.contains(expected),
            "nginx's own dump of the resolved config is missing {expected:?}:\n{out}"
        );
    }
}

/// The FULL DOCUMENTED MAXIMUM of `gzip_types`: [`GzipTypes::MAX_TOKENS`]
/// tokens of exactly [`GzipTypes::MAX_TOKEN_LEN`] bytes each. Every value here
/// is one this crate's own parser accepts and therefore one a user can save
/// from the Web server page.
///
/// This is the case the unit tests structurally cannot cover. The token cap
/// used to be 128 bytes on the theory that it only had to stop a smuggled
/// payload, and every unit test agreed with itself about that number — but
/// nginx's gzip filter hashes the type list into a bucket of
/// `ngx_cacheline_size` (64) that no directive resizes, which fits 46 bytes
/// per type and no more. Anything longer makes nginx reject THE WHOLE
/// CONFIGURATION:
///
/// ```text
/// nginx: [emerg] could not build test_types_hash, you should increase test_types_hash_bucket_size: 64
/// ```
///
/// and `save_web_server_settings` does not run `nginx -t`, so the value lands
/// in `state.db` silently and every later apply — including one for an
/// unrelated site edited on the Sites page — fails validation and rolls back
/// with an error naming nothing the user touched. Only a real nginx can say
/// where that boundary is, so only a real nginx can keep the constant honest.
#[tokio::test]
async fn the_documented_gzip_types_maximum_loads_in_real_nginx() {
    // Deliberately FAILS rather than skipping, like
    // `both_probes_pass_real_nginx_in_the_assembled_environment` above and for
    // the same reason: this is the ONLY check that the token cap is a number
    // nginx can load. GitHub CI is disabled on this repo and local gates are
    // the merge gate, so a skip here reads as a pass for the one fact this
    // test exists to establish.
    let Some(brew) = find_brew_binaries() else {
        panic!(
            "Homebrew nginx not found, so the gzip_types maximum could not be checked against \
             a real nginx. This test must not skip: it is the only proof that \
             GzipTypes::MAX_TOKEN_LEN is a length nginx's fixed test_types bucket accepts. \
             Install it (brew install nginx) and re-run."
        );
    };
    let (_home, ctx) = temp_home_ctx();

    // Unique, MIME-shaped, each exactly MAX_TOKEN_LEN bytes.
    let list = (0..GzipTypes::MAX_TOKENS)
        .map(|i| {
            let suffix = format!("{i:04}");
            let filler = "a".repeat(GzipTypes::MAX_TOKEN_LEN - "text/".len() - suffix.len());
            format!("text/{filler}{suffix}")
        })
        .collect::<Vec<_>>();
    for t in &list {
        assert_eq!(t.len(), GzipTypes::MAX_TOKEN_LEN);
    }
    let joined = list.join(" ");

    let settings = WebServerSettings {
        gzip: OnOff::new(true),
        gzip_types: GzipTypes::parse(&joined).unwrap(),
        ..WebServerSettings::default()
    };
    let main_path = materialize_with(&ctx, &settings);
    let err_log = ctx.home.join("logs/nginx.error.log");

    let report = validate_live(&brew.nginx, &main_path, &err_log, &ctx.home)
        .await
        .unwrap();
    assert!(
        report.ok,
        "real `nginx -t` REJECTED the documented maximum of gzip_types \
         ({} tokens x {} bytes) — a value the Web server page accepts, saves \
         without validation, and then makes every later apply fail:\n{}",
        GzipTypes::MAX_TOKENS,
        GzipTypes::MAX_TOKEN_LEN,
        report.stderr
    );
}

/// The largest `client_max_body_size` values [`BodySize`] accepts, each loaded
/// by a real nginx. Shape alone (`^\d+[kKmMgG]?$`) does not decide this:
/// nginx multiplies digits by unit into an `off_t` and refuses the config with
/// `"client_max_body_size" directive invalid value` past `i64::MAX`, so the
/// four boundaries the parser enforces are only meaningful if nginx agrees
/// with them exactly.
#[tokio::test]
async fn the_largest_body_sizes_we_accept_load_in_real_nginx() {
    // Fails rather than skips, for the same reason as the case above: these
    // numbers are claims about another program's arithmetic.
    let Some(brew) = find_brew_binaries() else {
        panic!(
            "Homebrew nginx not found, so the client_max_body_size boundaries could not be \
             checked against a real nginx. This test must not skip: it is the only proof \
             that BodySize's upper bound matches ngx_conf_set_off_slot. Install it \
             (brew install nginx) and re-run."
        );
    };
    for value in [
        "9223372036854775807",
        "8589934591g",
        "8796093022207m",
        "9007199254740991k",
    ] {
        let (_home, ctx) = temp_home_ctx();
        let settings = WebServerSettings {
            client_max_body_size: BodySize::parse(value).unwrap(),
            ..WebServerSettings::default()
        };
        let main_path = materialize_with(&ctx, &settings);
        let err_log = ctx.home.join("logs/nginx.error.log");
        let report = validate_live(&brew.nginx, &main_path, &err_log, &ctx.home)
            .await
            .unwrap();
        assert!(
            report.ok,
            "real `nginx -t` REJECTED client_max_body_size {value:?}, which \
             `BodySize::parse` accepts — the parser's upper bound is above \
             nginx's:\n{}",
            report.stderr
        );
    }
}

/// An empty `gzip_types` list is a legitimate setting ("compress nothing
/// beyond nginx's built-in text/html"). This is the case that turns into a
/// bare `gzip_types;` if the renderer emits the directive unconditionally —
/// a syntax error only a real nginx reports.
#[tokio::test]
async fn an_empty_gzip_types_list_still_passes_real_nginx() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP an_empty_gzip_types_list_still_passes_real_nginx: brew nginx not found");
        return;
    };
    let (_home, ctx) = temp_home_ctx();
    let settings = WebServerSettings {
        gzip: OnOff::new(true),
        gzip_types: GzipTypes::parse("   ").unwrap(),
        ..WebServerSettings::default()
    };
    let main_path = materialize_with(&ctx, &settings);
    let err_log = ctx.home.join("logs/nginx.error.log");
    let report = validate_live(&brew.nginx, &main_path, &err_log, &ctx.home)
        .await
        .unwrap();
    assert!(
        report.ok,
        "an empty gzip_types list must render as NO directive; a bare \
         `gzip_types;` is what nginx is rejecting here:\n{}",
        report.stderr
    );
}

/// nginx discovery design D4's own proof requirement: `-p <home>` makes a
/// RELATIVE path in the config resolve under our home, not under nginx's
/// compiled-in prefix or `/opt/homebrew/var`.
///
/// Deliberately NOT one of the app's own generated configs: every path this
/// crate's templates emit is already absolute (`to_config_path`), so a
/// relative path has to be hand-written to exist at all — which is the
/// property D4 exists to keep true by construction rather than by accident.
/// `-t` is enough to prove it: nginx creates the files a config's
/// `error_log`/`access_log`/`*_temp_path` name as it parses, even in test
/// mode (the same fact `commands.rs`'s `validate_web_server_config` doc
/// comment relies on for `-e`), so a relative `error_log` directive either
/// lands under `home` or this test sees that it did not.
///
/// VACUITY, confirmed by hand against BOTH builds this app ships — and they
/// disagree, which is worth recording precisely rather than as one "loudest
/// possible signal" claim (4B fix-wave, item 4 — a prior version of this
/// comment made exactly that claim and was wrong about the build actually
/// shipped):
///
/// - **Homebrew 1.31.3**: removing `-p` makes `nginx -t` FAIL outright —
///   `[emerg] open() "<nginx's own Cellar prefix>/logs/relative.log" failed
///   (2: No such file or directory)`, exit 1. The claim this comment used to
///   make in general is true HERE.
/// - **Our own packaged 1.30.4 does not fail at all.** It exits 0, silently,
///   and writes into `/opt/openvhost-build/nginx-1.30.4/logs/` —
///   `build/recipes/nginx.sh`'s `$BUILD_PREFIX`, the build host's staging
///   directory, baked in at `./configure` time (`--prefix=$BUILD_PREFIX`)
///   and shipped inside the tarball unchanged. On the machine that built it
///   that directory still happens to exist, so the relative path resolves
///   there instead of failing.
///
/// So the case for `-p` is STRONGER than "the loudest possible signal", not
/// weaker: for the build this app actually ships, dropping `-p` is a SILENT
/// divergence — nginx keeps running, `-t` keeps exiting 0, and a relative
/// path simply lands in a directory an end user's machine will never even
/// have. There is no loud failure to notice unless you happen to be running
/// against Homebrew. This codebase treats a wrong "why" as worse than none,
/// so this records the measured fact for both builds rather than the
/// convenient one for a single build.
#[tokio::test]
async fn a_relative_path_in_the_config_resolves_under_home_not_under_the_prefix() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!(
            "SKIP a_relative_path_in_the_config_resolves_under_home_not_under_the_prefix: \
             brew nginx not found"
        );
        return;
    };
    let home = tempfile::Builder::new()
        .prefix("ovh conf p-flag ")
        .tempdir_in("/tmp")
        .unwrap();
    let conf = home.path().join("nginx.conf");
    // A directive nginx accepts even with no `http{}`/`server{}` at all — the
    // RELATIVE path is the entire point.
    std::fs::write(&conf, "error_log logs/relative.log;\nevents {}\n").unwrap();
    let err_log = home.path().join("logs/nginx.error.log"); // -e's target, always absolute
    std::fs::create_dir_all(err_log.parent().unwrap()).unwrap();

    let report = validate_live(&brew.nginx, &conf, &err_log, home.path())
        .await
        .unwrap();
    assert!(
        report.ok,
        "nginx rejected a config it accepts unassisted at a shell:\n{}",
        report.stderr
    );

    let resolved = home.path().join("logs/relative.log");
    assert!(
        resolved.is_file(),
        "the config's relative `error_log logs/relative.log;` did not resolve under \
         -p {} — either -p is not reaching nginx, or nginx is ignoring it",
        home.path().display()
    );
}
