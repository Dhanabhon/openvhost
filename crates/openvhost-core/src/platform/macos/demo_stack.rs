// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS home provisioning.
//!
//! Historically this module also wrote a hand-rolled nginx + php-fpm config
//! set (P0-4 demo stack). That is retired: `site::apply` (portable, not
//! macOS-only) now owns every generated file under
//! `<home>/config/generated/`. What remains here is OS-independent-in-spirit
//! but macOS-specific in location: create the directories the generated
//! config set expects, seed the welcome page, and resolve the Homebrew
//! binaries the supervised services spawn.

use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Homebrew binaries for the stack. Resolve at registration time: the
/// `opt/` symlinks silently retarget on major version bumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewStack {
    pub nginx: PathBuf,
    pub php_fpm: PathBuf,
}

/// Probe the standard Homebrew prefixes (Apple Silicon first, then Intel).
pub fn find_brew_binaries() -> Option<BrewStack> {
    find_brew_binaries_in(&[Path::new("/opt/homebrew"), Path::new("/usr/local")])
}

/// Pure prober: first prefix holding BOTH binaries wins.
pub fn find_brew_binaries_in(prefixes: &[&Path]) -> Option<BrewStack> {
    prefixes.iter().find_map(|p| {
        let nginx = p.join("opt/nginx/bin/nginx");
        let php_fpm = p.join("opt/php/sbin/php-fpm");
        if nginx.is_file() && php_fpm.is_file() {
            Some(BrewStack { nginx, php_fpm })
        } else {
            None
        }
    })
}

const INDEX_PHP: &str = "<?php phpinfo();\n";

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

/// Atomic write: temp file in the SAME directory as the target (same-volume
/// rename — never TMPDIR), then rename over the target.
fn atomic_write(path: &Path, contents: &str) -> Result<(), CoreError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| CoreError::ProvisionIo {
            op: "file_name",
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"),
        })?
        .to_string_lossy()
        .into_owned();
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, contents).map_err(|source| CoreError::ProvisionIo {
        op: "write",
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| CoreError::ProvisionIo {
        op: "rename",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Short-path tempdir: /tmp keeps generated paths far under Darwin's
    /// 104-byte `sun_path` limit (TMPDIR is /var/folders/... and brittle-long).
    fn short_home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ovh")
            .tempdir_in("/tmp")
            .unwrap()
    }

    // The `provision_home` directory/seed/idempotency contract is proved by
    // the integration tests in `tests/macos_stack.rs`, not duplicated here.

    #[test]
    fn brew_prober_requires_both_binaries_in_one_prefix() {
        let fake = short_home();
        let prefix = fake.path();
        std::fs::create_dir_all(prefix.join("opt/nginx/bin")).unwrap();
        std::fs::create_dir_all(prefix.join("opt/php/sbin")).unwrap();
        std::fs::write(prefix.join("opt/nginx/bin/nginx"), "").unwrap();
        // Only nginx present -> None.
        assert!(find_brew_binaries_in(&[prefix]).is_none());
        std::fs::write(prefix.join("opt/php/sbin/php-fpm"), "").unwrap();
        let stack = find_brew_binaries_in(&[prefix]).unwrap();
        assert_eq!(stack.nginx, prefix.join("opt/nginx/bin/nginx"));
        assert_eq!(stack.php_fpm, prefix.join("opt/php/sbin/php-fpm"));
    }
}
