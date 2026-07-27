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

    let live = validate_live(&brew.nginx, &main.path, &err_log)
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

    let version = probe_nginx_version(&brew.nginx, &err_log).await;
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

    let report = validate_live(&brew.nginx, &main_path, &err_log)
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
    let report = validate_live(&brew.nginx, &main_path, &err_log)
        .await
        .unwrap();
    assert!(
        report.ok,
        "an empty gzip_types list must render as NO directive; a bare \
         `gzip_types;` is what nginx is rejecting here:\n{}",
        report.stderr
    );
}
