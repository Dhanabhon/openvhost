// SPDX-License-Identifier: GPL-3.0-or-later
//! Native-validator plumbing: locate the Homebrew binaries (never via PATH —
//! ServBay shadows nginx/php-fpm there), materialize generated files into a
//! throwaway home, and run the validator capturing stderr. `ok` is derived
//! from the exit code alone.

use std::path::{Path, PathBuf};

use crate::GeneratedFile;
use crate::error::ConfError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewStack {
    pub nginx: PathBuf,
    pub php_fpm: PathBuf,
}

/// Probe the standard Homebrew prefixes (Apple Silicon, then Intel). NEVER
/// resolves via PATH — a ServBay install shadows `nginx`/`php-fpm` there.
pub fn find_brew_binaries() -> Option<BrewStack> {
    for prefix in [Path::new("/opt/homebrew"), Path::new("/usr/local")] {
        let nginx = prefix.join("opt/nginx/bin/nginx");
        let php_fpm = prefix.join("opt/php/sbin/php-fpm");
        if nginx.is_file() && php_fpm.is_file() {
            return Some(BrewStack { nginx, php_fpm });
        }
    }
    None
}

/// Write each generated file to disk under its `path`, creating parents.
///
/// `ctx.home` MUST be a throwaway validation home — this writes files into
/// it NON-ATOMICALLY (plain writes, no tmp+rename). It must never be
/// pointed at a live home; the apply/swap pipeline (deferred) owns atomic
/// installation.
pub(crate) fn materialize(files: &[GeneratedFile]) -> Result<(), ConfError> {
    for f in files {
        if let Some(parent) = f.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfError::Io {
                op: "create_dir",
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(&f.path, &f.contents).map_err(|e| ConfError::Io {
            op: "write",
            path: f.path.clone(),
            source: e,
        })?;
    }
    Ok(())
}
