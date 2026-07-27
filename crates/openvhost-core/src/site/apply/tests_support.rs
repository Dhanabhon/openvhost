// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures for the `apply` module's test suites. `#[cfg(test)]`-only,
//! so Task 3's (`mod.rs`) and Task 4's (`plan.rs`) tests build identical
//! inputs without duplicating fixture code between them.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use openvhost_conf::WebServerSettings;

use crate::site::model::{Docroot, Domain, PhpVersion, Site, SiteId, SiteName, WebServer};

use super::{ApplyInput, InstalledRuntimes, PhpRuntime};

pub(crate) fn site(name: &str, domain: &str, php: &str, enabled: bool) -> Site {
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

pub(crate) fn runtimes(majors: &[&str]) -> InstalledRuntimes {
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

pub(crate) fn input(sites: Vec<Site>, majors: &[&str]) -> ApplyInput {
    input_with_home(&PathBuf::from("/tmp/ovh"), sites, majors)
}

/// `input`, but with the home directory replaced — the planning tests need a
/// real temp directory on disk, not the fixed `/tmp/ovh` used by Task 3's
/// pure-rendering tests.
pub(crate) fn input_with_home(home: &Path, sites: Vec<Site>, majors: &[&str]) -> ApplyInput {
    ApplyInput {
        home: home.to_path_buf(),
        sites,
        runtimes: runtimes(majors),
        // The defaults, so every existing test keeps asserting on exactly the
        // output it asserted on before settings became an input. Tests that
        // care about a specific value overwrite this field on the returned
        // struct.
        settings: WebServerSettings::default(),
    }
}
