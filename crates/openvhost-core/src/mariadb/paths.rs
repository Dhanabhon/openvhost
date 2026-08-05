// SPDX-License-Identifier: GPL-3.0-or-later
//! Where MariaDB's on-disk state lives. The mirror of [`crate::mysql`]'s
//! `mysql_paths`, with one deliberate difference: it takes no series
//! argument. See docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md
//! (D5).

use std::path::{Path, PathBuf};

use crate::error::CoreError;

use super::MARIADB_SERIES;

/// Every generated/state path for the one MariaDB series this build ships,
/// all derived from the resolved OpenVHost home.
///
/// CONFINEMENT ARGUMENT (the Docroot lesson, restated here rather than
/// assumed): every field below is `home.join(...)` joined with
/// [`MARIADB_SERIES`], a compile-time constant. `home` comes only from
/// [`crate::resolve_home`] (an env override or the OS user-home lookup —
/// never IPC input). **Nothing steerable from outside this process reaches
/// any of these paths at all** — which is a stronger statement than
/// [`crate::mysql::MysqlPaths`] can make, because that one takes a
/// `MysqlMajor` and therefore has to argue about what that newtype can hold.
///
/// That difference is why [`mariadb_paths`] takes no series parameter. Spec
/// §13.3 pins exactly one series and spec §4 records the matching decision
/// for the port: a parameter here would be a knob that does not turn, and it
/// would trade a total confinement guarantee for the appearance of symmetry
/// with MySQL. A second series is a slice, not an argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MariadbPaths {
    /// `<home>/data/mariadb/<series>/` — the live datadir once initialized.
    pub datadir: PathBuf,
    /// `<home>/config/generated/mariadb/<series>/my.cnf`.
    pub my_cnf: PathBuf,
    /// `<home>/run/mariadb-<series>.sock` — the running server's socket.
    /// Distinct from `mysql-<major>.sock` by name, as the endpoint is by port
    /// (spec D2): the two engines coexist, so nothing either of them owns may
    /// collide with the other.
    pub socket: PathBuf,
    /// `<home>/run/mariadb-<series>-init.sock` — the network-less temp server
    /// used only during staged init.
    pub init_socket: PathBuf,
    /// `<home>/run/mariadb-<series>.pid` — mariadbd's `pid-file`.
    pub pid_file: PathBuf,
    /// `<home>/config/custom/mariadb/<series>/conf.d` — the user's own
    /// `!includedir`, never written by this app.
    pub custom_confd: PathBuf,
    /// `<home>/data/mariadb/` — parent of both [`Self::datadir`] and every
    /// staging directory, so the finishing `rename` at the end of init is
    /// atomic (same filesystem, same parent). Also the argument
    /// [`crate::mysql::sweep_stale_staging`] expects — that sweeper is
    /// generic in substance despite its module (spec D5), so MariaDB calls it
    /// where it already lives rather than growing a second copy.
    pub staging_parent: PathBuf,
}

impl MariadbPaths {
    /// Guards [`Self::socket`] and [`Self::init_socket`] against Darwin's
    /// `sun_path` ceiling, through the exact same check php-fpm's socket and
    /// mysqld's sockets already go through
    /// (`crate::mysql::datadir::guard_socket_path`, itself a delegation to
    /// [`crate::site::apply::MAX_SOCKET_PATH_BYTES`] and
    /// `CoreError::SocketPathTooLong`). Called across the module boundary on
    /// purpose: the limit is a property of the OS, not of an engine, and a
    /// second spelling of it is how one engine ends up with a different
    /// ceiling from the other. Moving that helper somewhere neutral is a
    /// mechanical follow-up (spec D5), not this slice.
    ///
    /// [`mariadb_paths`] itself never fails (pure path joining); this is the
    /// explicit, separate check callers run before acting on either socket.
    pub fn check_socket_lengths(&self) -> Result<(), CoreError> {
        crate::mysql::guard_socket_path(&self.socket)?;
        crate::mysql::guard_socket_path(&self.init_socket)
    }
}

/// `<home>/data/mariadb` — the parent of the datadir AND of every staging
/// directory (see [`MariadbPaths::staging_parent`]). Split out for the same
/// reason [`crate::mysql::mysql_data_root`] is: the staging sweep runs once
/// per rescan and has no reason to build a whole [`MariadbPaths`] to reach
/// one field.
///
/// **Never `<home>/data/mysql`.** The two engines' trees are siblings, not
/// one tree with two tenants: a shared parent would put a MariaDB staging
/// leftover in the path `crate::mysql::sweep_stale_staging` walks, and both
/// engines' data under one directory whose name names only one of them.
pub fn mariadb_data_root(home: &Path) -> PathBuf {
    home.join("data").join("mariadb")
}

/// Derive every path MariaDB needs from `home`. Pure and infallible — see
/// the CONFINEMENT ARGUMENT on [`MariadbPaths`], and
/// [`MariadbPaths::check_socket_lengths`] for the guard callers must run
/// before using either socket path.
pub fn mariadb_paths(home: &Path) -> MariadbPaths {
    let data_root = mariadb_data_root(home);
    let run_root = home.join("run");
    let series = MARIADB_SERIES;
    MariadbPaths {
        datadir: data_root.join(series),
        my_cnf: home
            .join("config")
            .join("generated")
            .join("mariadb")
            .join(series)
            .join("my.cnf"),
        socket: run_root.join(format!("mariadb-{series}.sock")),
        init_socket: run_root.join(format!("mariadb-{series}-init.sock")),
        pid_file: run_root.join(format!("mariadb-{series}.pid")),
        custom_confd: home
            .join("config")
            .join("custom")
            .join("mariadb")
            .join(series)
            .join("conf.d"),
        staging_parent: data_root,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::mysql::{MysqlMajor, mysql_paths};

    #[test]
    fn every_path_is_derived_under_home() {
        let paths = mariadb_paths(Path::new("/tmp/ovh"));

        assert_eq!(paths.datadir, PathBuf::from("/tmp/ovh/data/mariadb/11.4"));
        assert_eq!(
            paths.my_cnf,
            PathBuf::from("/tmp/ovh/config/generated/mariadb/11.4/my.cnf")
        );
        assert_eq!(
            paths.socket,
            PathBuf::from("/tmp/ovh/run/mariadb-11.4.sock")
        );
        assert_eq!(
            paths.init_socket,
            PathBuf::from("/tmp/ovh/run/mariadb-11.4-init.sock")
        );
        assert_eq!(
            paths.pid_file,
            PathBuf::from("/tmp/ovh/run/mariadb-11.4.pid")
        );
        assert_eq!(
            paths.custom_confd,
            PathBuf::from("/tmp/ovh/config/custom/mariadb/11.4/conf.d")
        );
        assert_eq!(paths.staging_parent, PathBuf::from("/tmp/ovh/data/mariadb"));
    }

    #[test]
    fn the_series_component_is_the_catalogue_constant() {
        // The confinement argument's premise, asserted rather than assumed:
        // MARIADB_SERIES is what lands in every path, and it can never carry
        // a separator or a parent-directory hop.
        assert_eq!(MARIADB_SERIES, "11.4");
        assert!(!MARIADB_SERIES.contains('/'));
        assert!(!MARIADB_SERIES.contains('\\'));
        assert!(!MARIADB_SERIES.contains(".."));
        let paths = mariadb_paths(Path::new("/tmp/ovh"));
        assert!(paths.datadir.ends_with(MARIADB_SERIES));
    }

    #[test]
    fn staging_parent_is_the_direct_parent_of_datadir() {
        // Staging must live in the SAME parent as the final datadir so the
        // finishing rename is atomic (same filesystem, same directory).
        let paths = mariadb_paths(Path::new("/tmp/ovh"));
        assert_eq!(paths.datadir.parent(), Some(paths.staging_parent.as_path()));
        assert_eq!(
            mariadb_data_root(Path::new("/tmp/ovh")),
            paths.staging_parent
        );
    }

    #[test]
    fn pid_file_lives_in_run_alongside_the_sockets() {
        let paths = mariadb_paths(Path::new("/tmp/ovh"));
        assert_eq!(paths.pid_file.parent(), paths.socket.parent());
        assert_eq!(paths.pid_file.parent(), paths.init_socket.parent());
    }

    #[test]
    fn nothing_collides_with_a_mysql_major() {
        // Two engines run at once (spec D2). Every path either engine owns
        // must be distinct from the other's, or one of them silently
        // overwrites the other's socket, pid file or datadir. Checked against
        // BOTH a catalogue major and an out-of-catalogue one, since discovery
        // lists majors this build does not offer to install.
        let home = Path::new("/tmp/ovh");
        let mdb = mariadb_paths(home);
        for major_str in ["8.4", "11.4"] {
            let major = MysqlMajor::from_probe(major_str.to_string()).unwrap();
            let my = mysql_paths(home, &major);
            assert_ne!(mdb.datadir, my.datadir, "major {major_str}");
            assert_ne!(mdb.my_cnf, my.my_cnf, "major {major_str}");
            assert_ne!(mdb.socket, my.socket, "major {major_str}");
            assert_ne!(mdb.init_socket, my.init_socket, "major {major_str}");
            assert_ne!(mdb.pid_file, my.pid_file, "major {major_str}");
            assert_ne!(mdb.custom_confd, my.custom_confd, "major {major_str}");
            assert_ne!(
                mdb.staging_parent, my.staging_parent,
                "a shared staging parent would put MariaDB leftovers in the \
                 directory mysql's sweeper walks (major {major_str})"
            );
        }
    }

    #[test]
    fn a_short_home_passes_the_socket_length_guard() {
        assert!(
            mariadb_paths(Path::new("/tmp/ovh"))
                .check_socket_lengths()
                .is_ok()
        );
    }

    #[test]
    fn a_home_too_deep_for_the_socket_is_refused() {
        // The same constant and the same error php-fpm's and mysqld's sockets
        // are guarded by — reused, not reinvented.
        let deep = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));
        let err = mariadb_paths(&deep).check_socket_lengths().unwrap_err();
        assert!(
            matches!(err, CoreError::SocketPathTooLong { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn the_init_socket_is_guarded_too_not_only_the_live_one() {
        // Vacuity for the guard: `init_socket` is the LONGER of the two, so a
        // guard that checked only `socket` would still pass the test above.
        // Pick a home where the live socket fits and the init socket does not.
        let paths = mariadb_paths(Path::new("/tmp/ovh"));
        let slack = crate::site::apply::MAX_SOCKET_PATH_BYTES
            - paths.socket.as_os_str().as_encoded_bytes().len();
        let home = PathBuf::from(format!("/tmp/ovh{}", "d".repeat(slack)));
        let paths = mariadb_paths(&home);
        assert!(
            paths.socket.as_os_str().as_encoded_bytes().len()
                <= crate::site::apply::MAX_SOCKET_PATH_BYTES,
            "the live socket must still fit, or this proves nothing"
        );
        let err = paths.check_socket_lengths().unwrap_err();
        match err {
            CoreError::SocketPathTooLong { path, .. } => {
                assert_eq!(path, paths.init_socket, "the init socket is the offender");
            }
            other => panic!("expected SocketPathTooLong, got {other:?}"),
        }
    }
}
