// SPDX-License-Identifier: GPL-3.0-or-later
//! Create a log directory at the mode spec D5 requires.
//!
//! Before this existed, three independent call sites each created a log
//! directory their own way: `site::apply::commit` (an Apply's log
//! directories), `platform::macos::demo_stack::provision_home` (seeding
//! `logs/sites`/`logs/services` up front), and the desktop crate's own
//! `stack::ensure_php_fpm_log_dir` (which can run before either of the other
//! two, for a PHP major discovered after the last Apply). [`ensure_log_dir`]
//! is now the ONE function all three go through, so "log directories are
//! `0700`" is a single decision rather than one re-implemented — and
//! possibly reproduced inconsistently — at each site.
//!
//! `openvhost-conf`'s `NginxAdapter::validate`/`PhpFpmRuntime::validate`
//! still create a directory independently, NOT through this function —
//! deliberately: `openvhost-conf` cannot depend on this crate (this crate
//! depends on conf, so the reverse would cycle), and their directory is
//! always inside a THROWAWAY validation home used only to shape-check a
//! render (`ctx.home MUST be a throwaway validation home`, per each of
//! their own doc comments), never the persistent home spec D5 is protecting.
//! Each of those two already funnels its own generate/validate pair through
//! one shared per-file helper, so there is still exactly one derivation per
//! concern — just not this one, and not `0700` (an ephemeral, deleted-with-
//! the-tempdir shape-check scratch directory is not the "any other local
//! account can read your logs" threat model this mode defends against).

use std::path::Path;

/// Create `dir` (and any missing parents) if it does not already exist, then
/// set its mode to exactly `0700` — UNCONDITIONALLY, whether this call just
/// created the directory or it already existed at some other mode. Mirrors
/// `platform::macos::demo_stack`'s `lock_down_home`, which applies the same
/// "unconditional, not merely on creation" discipline to `<home>` itself: an
/// install that predates this function must be tightened the next time it
/// runs, not only a fresh one.
///
/// Spec D5: log directories are `0700` explicitly, not merely inherited from
/// `<home>`'s own mode, so a future loosening of the home's mode cannot
/// silently carry into `logs/`. This is the boundary that actually matters —
/// the log FILES nginx/php-fpm create inside these directories are written
/// under THEIR OWN umask, which this function cannot control without a race
/// (stated, not papered over) — so confining who can even reach the
/// directory is the real control this function provides.
#[cfg(unix)]
pub fn ensure_log_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Windows ACL model is deferred (macOS-first, plan §7) — plain
/// `create_dir_all` only, mirroring `demo_stack::lock_down_home`'s identical
/// `#[cfg(not(unix))]` posture.
#[cfg(not(unix))]
pub fn ensure_log_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_missing_directory_including_its_parents() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("logs/sites/app.localhost");
        assert!(!dir.exists());

        ensure_log_dir(&dir).unwrap();

        assert!(dir.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn creates_a_fresh_directory_at_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("logs/services/php-fpm-8.4");

        ensure_log_dir(&dir).unwrap();

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a freshly created log directory must be 0700");
    }

    /// Mirrors `demo_stack::lock_down_home`'s own "even when it already
    /// existed looser" test: an install that predates this fix left the
    /// directory at whatever the ambient umask gave `create_dir_all`
    /// (typically 0755), and that must be TIGHTENED the next time this runs
    /// — not left alone merely because the directory already existed.
    #[cfg(unix)]
    #[test]
    fn tightens_a_pre_existing_looser_directory_to_0700() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("logs/sites/app.localhost");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_log_dir(&dir).unwrap();

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "a pre-existing looser directory must be tightened, not left alone"
        );
    }

    #[test]
    fn calling_it_twice_is_a_harmless_no_op() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("logs/sites/app.localhost");

        ensure_log_dir(&dir).unwrap();
        ensure_log_dir(&dir).unwrap();

        assert!(dir.is_dir());
    }
}
