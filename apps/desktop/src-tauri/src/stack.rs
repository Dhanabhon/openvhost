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

/// The paths the stack actually registered, so the Web Server page can report
/// them instead of re-probing and possibly disagreeing. Read out of the managed
/// `Option<StackPaths>` state by `commands::list_web_servers` and friends.
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

/// Build the two supervised stack rows. Provision errors are logged and
/// non-fatal (rows register; Start surfaces the problem honestly). Only a
/// home-resolution failure skips the rows entirely — without a home there
/// are no config paths to point at.
pub fn macos_stack() -> MacosStack {
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("stack: cannot resolve OPENVHOST_HOME, skipping nginx/php-fpm rows: {e}");
            return MacosStack {
                specs: vec![],
                paths: None,
            };
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
                    nginx_conf.into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
    ];
    MacosStack {
        specs,
        paths: Some(paths),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
        let c = args
            .iter()
            .position(|a| a == "-c")
            .expect("nginx spawns with -c");
        assert_eq!(args[c + 1], paths.nginx_conf.to_string_lossy());
    }
}
