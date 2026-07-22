// SPDX-License-Identifier: GPL-3.0-or-later
//! Exit-criterion proof (master plan P0-7): the generated stack passes the
//! native validators on real Homebrew nginx + php-fpm. Auto-skips (loudly)
//! when the binaries are absent. The temp home path deliberately CONTAINS A
//! SPACE to prove the quoting rule (nginx splits unquoted whitespace) end to
//! end, including quoted `include` globs.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use openvhost_conf::{
    NginxAdapter, PhpFpmRuntime, PhpRuntimeAdapter, PhpUpstream, RenderCtx, WebServerAdapter,
    find_brew_binaries,
};

#[tokio::test]
async fn generated_stack_passes_native_validators() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP validate_live: Homebrew nginx/php-fpm not found (brew install nginx php)");
        return;
    };

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

    let nginx_report = NginxAdapter.validate(&brew.nginx, &ctx).await.unwrap();
    assert!(nginx_report.ok, "nginx -t failed:\n{}", nginx_report.stderr);

    let fpm_report = PhpFpmRuntime.validate(&brew.php_fpm, &ctx).await.unwrap();
    assert!(fpm_report.ok, "php-fpm -t failed:\n{}", fpm_report.stderr);

    // The php-fpm empty-glob WARNING is expected and must NOT flip ok.
    // (No assertion on stderr emptiness — that is the whole point.)

    // A zero-match `include` glob also passes plain `-t` silently, so `-t`
    // alone can't prove the main->site include seam actually expanded. `-T`
    // test-and-dumps the fully resolved config to stdout instead.
    let main = NginxAdapter.generate_main_config(&ctx).unwrap();
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
