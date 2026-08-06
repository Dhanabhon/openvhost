// SPDX-License-Identifier: GPL-3.0-or-later
//! `-p`'s target for every LIVE nginx invocation — deliberately NOT `home`
//! itself. See docs/superpowers/specs/2026-08-06-p2-nginx-discovery-design.md
//! (D4) for why `-p` is mandatory at all, and the 4B fix-wave audit (item 1)
//! for why `home` was the wrong value to give it.
//!
//! # The finding this closes
//!
//! `main.conf.tera` explicitly invites the user to author their own nginx
//! files (`include "{{ custom_sites_glob }}"`), so "nothing we GENERATE is
//! relative" is true but insufficient — nothing INCLUDED is under our
//! control. The audit reproduced it live, same config, same relative
//! `root .;`: with `-p home`, `GET /state.db` returned the file body
//! verbatim; `state.db` holds MySQL/MariaDB root credentials at rest, and
//! its mode `0600` does not help, because nginx runs as the same user.
//!
//! A dedicated, empty, provisioned directory holds nothing a relative root
//! could ever expose, which is the only fix that does not depend on every
//! present and future custom nginx file staying absolute.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `-p`'s target for every LIVE nginx invocation (the supervised spawn, the
/// version probe, `nginx -t` against the installed config): a dedicated,
/// empty, provisioned subdirectory of `home`, never `home` itself.
///
/// THE one place this path is computed. Every call site that used to pass a
/// live, resolved home straight to nginx's `-p` flag must go through this
/// function instead, or `nginx -t` stops testing what actually runs —
/// [`crate::platform::macos::demo_stack::provision_home`] creates this
/// directory the way it provisions its siblings (`run`, `run/nginx`).
///
/// NOT for a THROWAWAY validation home: `openvhost-conf`'s
/// `webserver.rs::NginxAdapter::validate` and
/// `settings::check::check_settings` already render into a scratch directory
/// holding nothing of value, so the whole directory already IS the safe
/// target `-p` needs — see each's own doc comment for why they stay as they
/// are rather than routing through this function.
pub fn nginx_prefix_dir(home: &Path) -> PathBuf {
    home.join("run/nginx-prefix")
}

/// The exact argv every supervised, LONG-RUNNING nginx is spawned with:
/// `-e <err_log> -p <prefix> -c <nginx_conf>`.
///
/// THE production argv. `apps/desktop/src-tauri/src/stack.rs`'s `nginx_spec`
/// (the app's own supervisor) and every live-proof test that spawns a real
/// nginx server build their `Command`'s args from this SAME function, so the
/// two cannot drift apart the way they did before this existed (4B
/// fix-wave, item 3): the e2e tests used to hand-copy `-e`/`-c` and had
/// silently dropped `-p` entirely, with nothing in the regression net able
/// to notice.
///
/// `err_log` is derived from `home` via [`crate::LogPaths::nginx_error`] —
/// the one formula every nginx invocation in this app uses for it, never a
/// second spelling — and `-p` from [`nginx_prefix_dir`], never `home`
/// itself.
pub fn nginx_spawn_argv(home: &Path, nginx_conf: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-e"),
        crate::LogPaths::new(home).nginx_error().into_os_string(),
        OsString::from("-p"),
        nginx_prefix_dir(home).into_os_string(),
        OsString::from("-c"),
        nginx_conf.to_path_buf().into_os_string(),
    ]
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn is_a_dedicated_subdirectory_of_home_not_home_itself() {
        let home = Path::new("/tmp/ovh");
        let prefix = nginx_prefix_dir(home);
        assert_ne!(
            prefix, home,
            "the prefix must never equal home itself — that is the exact bug this closes"
        );
        assert!(prefix.starts_with(home), "{prefix:?} escaped {home:?}");
    }

    #[test]
    fn is_rooted_at_the_given_home_not_a_fixed_path() {
        // A mutation that hardcoded a fixed path would still pass the test
        // above; this one changes `home` and requires the output to follow.
        assert_eq!(
            nginx_prefix_dir(Path::new("/elsewhere")),
            PathBuf::from("/elsewhere/run/nginx-prefix")
        );
    }

    #[test]
    fn spawn_argv_carries_e_p_and_c_with_p_pointing_at_the_dedicated_prefix() {
        let home = Path::new("/tmp/ovh");
        let conf = Path::new("/tmp/ovh/config/generated/nginx/nginx.conf");
        let argv = nginx_spawn_argv(home, conf);
        let args: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        let e = args
            .iter()
            .position(|a| a == "-e")
            .expect("nginx spawns with -e");
        assert_eq!(
            args[e + 1],
            crate::LogPaths::new(home).nginx_error().to_string_lossy()
        );

        let p = args
            .iter()
            .position(|a| a == "-p")
            .expect("nginx spawns with -p");
        assert_eq!(args[p + 1], nginx_prefix_dir(home).to_string_lossy());
        assert_ne!(
            args[p + 1],
            home.to_string_lossy(),
            "-p must never be home itself"
        );

        let c = args
            .iter()
            .position(|a| a == "-c")
            .expect("nginx spawns with -c");
        assert_eq!(args[c + 1], conf.to_string_lossy());
    }
}
