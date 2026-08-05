// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal `my.cnf` generation for MariaDB. See spec D3:
//! docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md.
//!
//! Its own template tree and its own concrete functions, per the standing
//! decision recorded at `crate::mysql`'s module doc ("a second implementation,
//! when it arrives, gets its own template tree and its own concrete
//! functions"). No shared trait, and no shared template: `mysqlx=OFF` alone
//! forces the split — MEASURED 2026-08-04 against the real 11.4.9 artifact,
//! `mariadbd … --mysqlx=OFF` logs `[ERROR] … unknown variable 'mysqlx=OFF'`
//! and `[ERROR] Aborting`. It aborts LATE, too — after InnoDB has already
//! created `ibdata1`/`undo00[1-3]` in the datadir — so a shared template would
//! not merely fail, it would fail having already written into the directory it
//! was pointed at.
//!
//! **There is deliberately no `MariadbValidator`.** `mysqld --validate-config`
//! has no MariaDB counterpart: `mariadbd --no-defaults --help --verbose` on
//! 11.4.9 lists zero occurrences of `validate-config`, and `--help` does not
//! parse a `--defaults-file` for unknown variables either (the `--mysqlx=OFF`
//! run above exits 0 under `--help` and only fails on a real start). The
//! definitive validation is therefore the supervised start plus its readiness
//! probe — which `crate::mysql::MysqlValidator`'s own doc comment already
//! calls the last word even where the pre-flight exists.

use std::path::PathBuf;

use crate::GeneratedFile;
use crate::ctx::to_config_path;
use crate::engine::render;
use crate::error::ConfError;

/// Every path MariaDB's `my.cnf` needs, as plain values.
///
/// Deliberately NOT `openvhost_core::mariadb::MariadbPaths` itself, for the
/// reason [`crate::MysqlCtx`] records at length: `openvhost-conf` does not
/// depend on `openvhost-core` (the reverse is true), so importing that type
/// here would invert the workspace's dependency graph. The first six fields
/// mirror `MariadbPaths` name for name so the copy at the call site is a
/// straight line-up rather than a renaming exercise.
///
/// There is no `series` field and no `major` field: spec §13.3 pins exactly
/// one series and the port is a literal (below), so a parameter here would be
/// a knob that does not turn — the identical argument `mariadb_paths` makes
/// for taking no series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MariadbCtx {
    /// Where the rendered file is written. Mirrors `MariadbPaths::my_cnf`
    /// (`<home>/config/generated/mariadb/<series>/my.cnf`).
    pub my_cnf: PathBuf,
    /// Mirrors `MariadbPaths::datadir` (`<home>/data/mariadb/<series>/`).
    pub datadir: PathBuf,
    /// Mirrors `MariadbPaths::socket` (`<home>/run/mariadb-<series>.sock`) —
    /// used for both the `[mariadbd]` listen socket and the `[client]`
    /// default. Distinct from `mysql-<major>.sock` by name, as the endpoint is
    /// by port: the two engines coexist.
    pub socket: PathBuf,
    /// Mirrors `MariadbPaths::pid_file` (`<home>/run/mariadb-<series>.pid`).
    pub pid_file: PathBuf,
    /// Mirrors `MariadbPaths::custom_confd`
    /// (`<home>/config/custom/mariadb/<series>/conf.d`) — the user's own
    /// `!includedir` target, never written by this app.
    pub custom_confd: PathBuf,
    /// The server's installation root — see [`crate::MysqlCtx::basedir`] for
    /// the full argument these four exist for. In short: measured on
    /// 2026-08-04, a running 11.4.9 started with `--no-defaults` reported
    /// `basedir`, `character_sets_dir` and `plugin_dir` all under its
    /// compiled-in `/opt/openvhost-build/mariadb-11.4.9/`, which exists on no
    /// user's machine; with these four pinned it reported the real package
    /// tree instead.
    pub basedir: PathBuf,
    /// `<basedir>/lib/plugin`.
    pub plugin_dir: PathBuf,
    /// `<basedir>/share/charsets` for this package tree — NOT
    /// `share/mysql/charsets`, which is where Homebrew's `mysql@8.4` keeps
    /// them. The layouts differ, which is why this is a value and not a
    /// suffix this crate appends.
    pub character_sets_dir: PathBuf,
    /// `<basedir>/share` — the PARENT of the per-language `errmsg.sys`
    /// directories (`share/english/errmsg.sys`), not that directory itself.
    pub lc_messages_dir: PathBuf,
}

/// Render `my.cnf` for the packaged MariaDB series.
///
/// `port` is the bare literal `3307` in the template, not a Tera variable —
/// spec D2 fixes it, for the reason `crate::mysql`'s `generate_my_cnf` already
/// records for 3306 ("a variable here would be a knob that does not actually
/// turn"). 3307 rather than 3306 is what lets both engines run at once:
/// `tray/model.rs` dedupes services by endpoint string, so a MariaDB service
/// claiming `127.0.0.1:3306` would be silently dropped from "Start all".
///
/// **`mysqlx=OFF` is absent and must stay absent** — MariaDB has never shipped
/// the X Protocol, and the directive is rejected outright (see this module's
/// doc comment). Its absence costs nothing, because the exposure it closes for
/// MySQL does not exist here: measured 2026-08-04, a temp server started with
/// `--no-defaults --skip-networking --socket=<path>` bound EXACTLY the one
/// socket it was told to (`lsof -p <pid>` showed a single `unix` descriptor,
/// `lsof -nP -p <pid> -iTCP -sTCP:LISTEN` showed no row for it at all, and a
/// `find /tmp -maxdepth 1 -type s` sweep found nothing new).
///
/// The section is `[mariadbd]`, not `[mysqld]`: verified live that MariaDB
/// 11.4.9 does read it (`SHOW VARIABLES` reflected every directive below), and
/// it has the useful property that a file meant for one engine cannot be
/// silently half-honoured by the other.
///
/// Pure function of `ctx`: same input, byte-identical output (workspace hard
/// rule). Every path value passes through [`to_config_path`] — the crate's
/// single chokepoint for embedding a path into a config template — matching
/// `my.cnf`'s unquoted-INI family exactly as the MySQL renderer does.
pub fn generate_mariadb_my_cnf(ctx: &MariadbCtx) -> Result<GeneratedFile, ConfError> {
    let datadir = to_config_path(&ctx.datadir)?;
    let socket = to_config_path(&ctx.socket)?;
    let pid_file = to_config_path(&ctx.pid_file)?;
    let custom_confd = to_config_path(&ctx.custom_confd)?;
    let basedir = to_config_path(&ctx.basedir)?;
    let plugin_dir = to_config_path(&ctx.plugin_dir)?;
    let character_sets_dir = to_config_path(&ctx.character_sets_dir)?;
    let lc_messages_dir = to_config_path(&ctx.lc_messages_dir)?;

    let mut tc = tera::Context::new();
    tc.insert("datadir", &datadir);
    tc.insert("socket", &socket);
    tc.insert("pid_file", &pid_file);
    tc.insert("custom_confd", &custom_confd);
    tc.insert("basedir", &basedir);
    tc.insert("plugin_dir", &plugin_dir);
    tc.insert("character_sets_dir", &character_sets_dir);
    tc.insert("lc_messages_dir", &lc_messages_dir);
    let contents = render("mariadb/my.cnf", &tc)?;
    Ok(GeneratedFile {
        path: ctx.my_cnf.clone(),
        contents,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ctx::directive;

    /// The package tree the fixture's server was "discovered" in — the real
    /// shape `packages/mariadb/11.4/11.4.9/` has, with the CONCRETE version
    /// directory spec D5 requires (never `current`).
    const FIXTURE_BASEDIR: &str = "/tmp/ovh/packages/mariadb/11.4/11.4.9";

    fn fixture_ctx() -> MariadbCtx {
        MariadbCtx {
            my_cnf: PathBuf::from("/tmp/ovh/config/generated/mariadb/11.4/my.cnf"),
            datadir: PathBuf::from("/tmp/ovh/data/mariadb/11.4"),
            socket: PathBuf::from("/tmp/ovh/run/mariadb-11.4.sock"),
            pid_file: PathBuf::from("/tmp/ovh/run/mariadb-11.4.pid"),
            custom_confd: PathBuf::from("/tmp/ovh/config/custom/mariadb/11.4/conf.d"),
            basedir: PathBuf::from(FIXTURE_BASEDIR),
            plugin_dir: PathBuf::from("/tmp/ovh/packages/mariadb/11.4/11.4.9/lib/plugin"),
            character_sets_dir: PathBuf::from(
                "/tmp/ovh/packages/mariadb/11.4/11.4.9/share/charsets",
            ),
            lc_messages_dir: PathBuf::from("/tmp/ovh/packages/mariadb/11.4/11.4.9/share"),
        }
    }

    /// The golden file, byte for byte — the same shape the MySQL renderer is
    /// pinned by, so a stray edit to either template is a visible diff rather
    /// than a behavioural surprise.
    const EXPECTED_MY_CNF: &str = "\
# ---------------------------------------------------------------------------
# GENERATED by OpenVHost — DO NOT EDIT. Regenerated idempotently; your edits
# will be lost. To customize, add files under:
#   /tmp/ovh/config/custom/mariadb/11.4/conf.d
# ---------------------------------------------------------------------------
[mariadbd]
basedir=/tmp/ovh/packages/mariadb/11.4/11.4.9
plugin_dir=/tmp/ovh/packages/mariadb/11.4/11.4.9/lib/plugin
character-sets-dir=/tmp/ovh/packages/mariadb/11.4/11.4.9/share/charsets
lc_messages_dir=/tmp/ovh/packages/mariadb/11.4/11.4.9/share
datadir=/tmp/ovh/data/mariadb/11.4
socket=/tmp/ovh/run/mariadb-11.4.sock
pid-file=/tmp/ovh/run/mariadb-11.4.pid
port=3307
bind-address=127.0.0.1
skip-name-resolve
log-warnings=2
!includedir /tmp/ovh/config/custom/mariadb/11.4/conf.d

[client]
socket=/tmp/ovh/run/mariadb-11.4.sock
port=3307
";

    #[test]
    fn mariadb_my_cnf_matches_the_golden_file_exactly() {
        let f = generate_mariadb_my_cnf(&fixture_ctx()).unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/mariadb/11.4/my.cnf")
        );
        assert_eq!(
            f.contents, EXPECTED_MY_CNF,
            "rendered my.cnf did not match the golden file, got:\n{}",
            f.contents
        );
    }

    /// Spec D3, the whole point of this slice's config half.
    ///
    /// VACUITY: proven by editing `FIXTURE_BASEDIR` to `/opt/elsewhere` — all
    /// three `starts_with` arms fail, each naming its own value. Deleting a
    /// line from the template fails that key's `expect` instead of passing.
    #[test]
    fn mariadb_my_cnf_pins_all_four_runtime_directories_inside_the_package_tree() {
        let c = generate_mariadb_my_cnf(&fixture_ctx()).unwrap().contents;

        assert_eq!(directive(&c, "basedir"), Some(FIXTURE_BASEDIR));
        for key in ["plugin_dir", "character-sets-dir", "lc_messages_dir"] {
            let value = directive(&c, key).unwrap_or_else(|| panic!("{key} must be pinned"));
            assert!(
                value.starts_with(&format!("{FIXTURE_BASEDIR}/")),
                "{key}={value} must live inside the package tree {FIXTURE_BASEDIR}"
            );
        }
    }

    /// `mysqlx=OFF` is a real MySQL directive that MariaDB REJECTS — and it
    /// rejects it late, after InnoDB has written into the datadir. Copying the
    /// MySQL template is the obvious mistake this pins against.
    ///
    /// VACUITY: proven by adding `mysqlx=OFF` to the template — fails with the
    /// message below.
    #[test]
    fn mariadb_my_cnf_never_contains_the_mysqlx_directive() {
        let c = generate_mariadb_my_cnf(&fixture_ctx()).unwrap().contents;
        assert!(
            !c.contains("mysqlx"),
            "mariadbd aborts with \"unknown variable 'mysqlx=OFF'\" — measured \
             against real 11.4.9. Got:\n{c}"
        );
    }

    /// Spec D2: two engines coexist only because their endpoints differ. A
    /// MariaDB config claiming 3306 is silently dropped from "Start all" by
    /// the tray's endpoint dedupe, with no error anywhere.
    ///
    /// VACUITY: proven by changing the template's `port=3307` to `3306` —
    /// both arms fail.
    #[test]
    fn mariadb_my_cnf_claims_port_3307_in_both_sections_and_never_3306() {
        let c = generate_mariadb_my_cnf(&fixture_ctx()).unwrap().contents;
        assert_eq!(
            c.matches("port=3307").count(),
            2,
            "both [mariadbd] and [client] must say 3307, got:\n{c}"
        );
        assert!(!c.contains("3306"), "got:\n{c}");
    }

    #[test]
    fn mariadb_generation_is_deterministic() {
        // Workspace hard rule: same input, byte-identical output.
        let a = generate_mariadb_my_cnf(&fixture_ctx()).unwrap();
        let b = generate_mariadb_my_cnf(&fixture_ctx()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn mariadb_every_rendered_line_has_no_stray_trailing_whitespace() {
        // A trailing space becomes part of the option-file VALUE (my.cnf's
        // parser takes the rest of the line verbatim) — invisible in a diff,
        // and it would surface as a mysterious "no such file or directory".
        let c = generate_mariadb_my_cnf(&fixture_ctx()).unwrap().contents;
        for line in c.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in: {line:?}");
        }
    }

    #[test]
    fn mariadb_rejects_a_path_that_would_break_out_of_the_option_file() {
        // The same ingress guard the MySQL renderer relies on: every path
        // goes through `to_config_path`.
        let mut ctx = fixture_ctx();
        ctx.datadir = PathBuf::from("/tmp/ovh/data/mariadb/11.4\nport=3306");
        let e = generate_mariadb_my_cnf(&ctx).unwrap_err();
        assert!(matches!(e, ConfError::InvalidField { .. }), "got {e:?}");
    }
}
