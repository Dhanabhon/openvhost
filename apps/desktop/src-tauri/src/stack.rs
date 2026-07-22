// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS demo-stack registration (P0-4). Data-only: binaries from the
//! Homebrew probe (resolved at registration time), configs provisioned
//! under the OpenVHost home. P0-6 swaps the binary source to packages/.

use std::ffi::OsString;
use std::path::PathBuf;

use openvhost_core::platform::macos::demo_stack::{
    BrewStack, find_brew_binaries, provision_macos_demo_stack,
};
use openvhost_proc::{ServiceSpec, SpawnSpec};

const DEMO_PORT: u16 = 8080;

/// Apple Silicon default paths, used when probing finds nothing: the rows
/// still register, and Start yields an honest Failed naming the missing
/// path (the P0-3 spawn-fail contract) instead of the rows vanishing.
fn fallback_brew() -> BrewStack {
    BrewStack {
        nginx: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
        php_fpm: PathBuf::from("/opt/homebrew/opt/php/sbin/php-fpm"),
    }
}

/// Build the two supervised stack rows. Provision errors are logged and
/// non-fatal (rows register; Start surfaces the problem honestly). Only a
/// home-resolution failure skips the rows entirely — without a home there
/// are no config paths to point at.
pub fn macos_stack_specs() -> Vec<ServiceSpec> {
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("stack: cannot resolve OPENVHOST_HOME, skipping nginx/php-fpm rows: {e}");
            return vec![];
        }
    };
    if let Err(e) = provision_macos_demo_stack(&home, DEMO_PORT) {
        eprintln!("stack: provisioning failed (rows registered anyway): {e}");
    }
    let brew = find_brew_binaries().unwrap_or_else(fallback_brew);
    let conf = home.join("conf");
    vec![
        ServiceSpec {
            id: "php-fpm".into(),
            display_name: "PHP-FPM".into(),
            endpoint: Some("run/php-fpm.sock".into()),
            spawn: SpawnSpec {
                program: brew.php_fpm,
                args: vec![
                    OsString::from("-F"),
                    OsString::from("-O"),
                    OsString::from("-n"),
                    OsString::from("-y"),
                    conf.join("php-fpm.conf").into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
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
                    conf.join("nginx.conf").into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
    ]
}
