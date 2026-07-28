// SPDX-License-Identifier: GPL-3.0-or-later
//! Ask nginx whether a CANDIDATE set of settings is acceptable, before anyone
//! stores it.
//!
//! Why this exists, and why it is not [`crate::WebServerAdapter::validate`]:
//! that call renders the main config with `WebServerSettings::default()` on
//! purpose, because it answers "is the generated *shape* valid?". It would
//! wave through a user-supplied combination nginx actually rejects. This
//! module renders the user's OWN values and runs the same `nginx -t` the apply
//! pipeline runs (`validate_live`), so the answer is about their values.
//!
//! The problem it closes: [`crate::WebServerAdapter::generate_main_config`] is
//! re-run from the stored settings on EVERY apply. A value that passes the
//! newtypes in [`super::value`] but that nginx refuses is therefore not a
//! one-off failure — once stored, every later apply fails validation and rolls
//! back, including an apply triggered by an unrelated site edit, where the
//! error names an nginx internal and points at no field the user can see. It
//! fails safe and it fails forever. Checking before the row is written turns
//! that into a rejection on the field the user just edited.
//!
//! SCOPE, stated so the apply pipeline's own validation is not mistaken for
//! redundant: this renders the settings ALONE, into a throwaway home whose
//! `include` globs match nothing. It answers "are these values acceptable to
//! nginx", not "does the whole config set apply" — the site configs, their
//! docroots and the php-fpm upstreams are still the apply pipeline's job.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::ConfError;
use crate::settings::WebServerSettings;
use crate::webserver::{NginxAdapter, WebServerAdapter};

/// nginx's verdict on a candidate set of settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCheck {
    /// `nginx -t` exited 0. Its stderr is kept because nginx writes there even
    /// on success.
    Accepted { stderr: String },
    /// `nginx -t` exited non-zero.
    Rejected {
        /// The settings field nginx objected to, when the rejection can be
        /// traced to one — see [`field_for_rejection`]. `None` means the
        /// message belongs in a banner rather than beside a form field, and
        /// is NOT a claim that the settings are fine.
        field: Option<&'static str>,
        stderr: String,
    },
}

/// The directives the Web server page can edit, each spelled exactly as the
/// generated main config writes it.
///
/// This doubles as the map from nginx directive to form field, because the two
/// names are deliberately identical: `WebServerSettingsDto`'s snake_case field
/// names ARE the nginx directive names (`gzip_comp_level`,
/// `client_max_body_size`, ...), which is also what the `fieldErrors` seam is
/// keyed by. Anything NOT in this list — `error_log`, `include`, `types`, the
/// temp paths — is a directive the user cannot edit, so a rejection on one of
/// those lines must never be pinned on a form field.
const EDITABLE_DIRECTIVES: [&str; 10] = [
    "worker_connections",
    "client_max_body_size",
    "keepalive_timeout",
    "tcp_nodelay",
    "fastcgi_connect_timeout",
    "fastcgi_send_timeout",
    "fastcgi_read_timeout",
    "gzip",
    "gzip_comp_level",
    "gzip_types",
];

/// Which settings field nginx rejected, by reading the line number out of its
/// own error message and looking that line up in the config WE rendered.
///
/// nginx's message is not a reliable source for the field name on its own: it
/// names the directive for some failures (`"client_max_body_size" directive
/// invalid value in ...:7`) and not for others (`value must be between 1 and 9
/// in ...:7`). The line number is present in both, and we generated the file,
/// so resolving line → directive is exact where string-matching the prose
/// would be guesswork.
fn field_for_rejection(rendered: &str, stderr: &str) -> Option<&'static str> {
    let line_no = stderr
        .lines()
        .find(|l| l.contains("[emerg]"))
        .and_then(error_line_number)?;
    // nginx counts from 1.
    let line = rendered.lines().nth(line_no.checked_sub(1)?)?;
    let directive = line.trim().split([' ', '\t', ';']).find(|t| !t.is_empty())?;
    EDITABLE_DIRECTIVES.iter().copied().find(|d| *d == directive)
}

/// The `:<line>` nginx appends to an `[emerg]` message.
///
/// Read from the END of the message rather than by splitting on `:`, because
/// the path in the middle is a user-controlled home directory that may itself
/// contain a colon.
fn error_line_number(msg: &str) -> Option<usize> {
    let digits: String = msg
        .trim_end()
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    // Only a trailing `:<digits>` is a line reference; a message merely ending
    // in a number is not.
    let cut = msg.trim_end().len() - digits.len();
    if !msg.trim_end()[..cut].ends_with(':') {
        return None;
    }
    digits.chars().rev().collect::<String>().parse().ok()
}

/// A throwaway directory for one check, created fresh inside `scratch_root`.
///
/// NOT under `/tmp`: that directory is world-writable, and this crate already
/// refuses to put config-adjacent temp files there (see
/// `openvhost_core::site::apply::commit`'s `atomic_write_with_suffix`). The
/// caller passes a root inside the app's own home, so the candidate config is
/// written where every other generated file already lives.
///
/// `create_dir_all` on the ROOT and `create_dir` on the leaf: the leaf must
/// not already exist, so nothing pre-planted can be written through, and the
/// pid+counter name keeps two concurrent saves from picking the same leaf.
fn scratch_dir(scratch_root: &Path) -> Result<PathBuf, ConfError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = scratch_root.join(format!("settings-check-{}-{n}", std::process::id()));
    std::fs::create_dir_all(scratch_root).map_err(|e| ConfError::Io {
        op: "create_dir",
        path: scratch_root.to_path_buf(),
        source: e,
    })?;
    // A leftover from a crashed run must not make the next save fail forever.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).map_err(|e| ConfError::Io {
        op: "create_dir",
        path: dir.clone(),
        source: e,
    })?;
    Ok(dir)
}

/// Render `settings` into a throwaway home and run `nginx -t` against it.
///
/// Writes nothing outside `scratch_root`, and removes its own directory
/// afterwards. The live config is never touched, so a rejection here has no
/// effect on what is currently being served.
pub async fn check_settings(
    bin: &Path,
    scratch_root: &Path,
    settings: &WebServerSettings,
) -> Result<SettingsCheck, ConfError> {
    let dir = scratch_dir(scratch_root)?;
    let outcome = check_in(bin, &dir, settings).await;
    // Best-effort: a check that succeeded must not fail because cleanup did.
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

async fn check_in(
    bin: &Path,
    dir: &Path,
    settings: &WebServerSettings,
) -> Result<SettingsCheck, ConfError> {
    let main = NginxAdapter.generate_main_config(dir, settings)?;
    crate::validate::materialize(std::slice::from_ref(&main))?;
    // The directories `nginx -t` insists on being able to create/open. NOT
    // `www/`: no site is rendered here, so no docroot is referenced.
    for d in ["run", "run/nginx", "logs"] {
        let p = dir.join(d);
        std::fs::create_dir_all(&p).map_err(|e| ConfError::Io {
            op: "create_dir",
            path: p,
            source: e,
        })?;
    }
    let err_log = dir.join("logs/nginx.error.log");
    let report = crate::inspect::validate_live(bin, &main.path, &err_log).await?;
    Ok(if report.ok {
        SettingsCheck::Accepted {
            stderr: report.stderr,
        }
    } else {
        SettingsCheck::Rejected {
            field: field_for_rejection(&main.contents, &report.stderr),
            stderr: report.stderr,
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_rejection_to_the_field_on_the_line_nginx_named() {
        let rendered = "http {\n    gzip on;\n    gzip_comp_level 99;\n}\n";
        let stderr = "nginx: [emerg] value must be between 1 and 9 in /h/nginx.conf:3\n";
        assert_eq!(
            field_for_rejection(rendered, stderr),
            Some("gzip_comp_level"),
            "nginx named no directive here — only the line — so the line is what must be read"
        );
    }

    #[test]
    fn maps_a_rejection_that_does_name_its_directive_by_line_all_the_same() {
        let rendered = "http {\n    client_max_body_size 99999999999999999999g;\n}\n";
        let stderr =
            "nginx: [emerg] \"client_max_body_size\" directive invalid value in /h/nginx.conf:2\n";
        assert_eq!(
            field_for_rejection(rendered, stderr),
            Some("client_max_body_size")
        );
    }

    /// A rejection on a line the user cannot edit must NOT be pinned on a
    /// field. Marking an arbitrary form field for a failure in `include` or
    /// `error_log` would send the user editing a value that is not the
    /// problem — worse than the banner they would otherwise get.
    #[test]
    fn does_not_pin_a_field_when_the_failing_line_is_not_editable() {
        let rendered = "http {\n    include \"/h/sites/*.conf\";\n}\n";
        let stderr = "nginx: [emerg] open() failed in /h/nginx.conf:2\n";
        assert_eq!(field_for_rejection(rendered, stderr), None);
    }

    /// The path nginx echoes back is a user-controlled home directory, which
    /// may contain a colon. Splitting on `:` would read the wrong number.
    #[test]
    fn reads_the_line_number_past_a_colon_in_the_home_path() {
        let rendered = "http {\n    gzip on;\n    gzip_comp_level 99;\n}\n";
        let stderr = "nginx: [emerg] value must be between 1 and 9 in /Users/a/my:dir/nginx.conf:3";
        assert_eq!(
            field_for_rejection(rendered, stderr),
            Some("gzip_comp_level")
        );
    }

    #[test]
    fn no_line_reference_means_no_field() {
        let rendered = "http {\n    gzip_comp_level 99;\n}\n";
        assert_eq!(
            field_for_rejection(rendered, "nginx: [emerg] something went wrong"),
            None
        );
        assert_eq!(
            field_for_rejection(rendered, "nginx: [emerg] failed after 5"),
            None,
            "a message ending in a bare number is not a line reference"
        );
    }

    #[test]
    fn a_line_number_past_the_end_of_the_file_yields_no_field() {
        assert_eq!(
            field_for_rejection("http {\n}\n", "nginx: [emerg] bad in /h/nginx.conf:99"),
            None
        );
        assert_eq!(
            field_for_rejection("http {\n}\n", "nginx: [emerg] bad in /h/nginx.conf:0"),
            None,
            "nginx counts from 1; line 0 must not underflow"
        );
    }

    /// Every editable directive must be reachable by this map. A field that
    /// the generated config writes under a different spelling than the DTO
    /// uses would silently never be marked.
    ///
    /// Rendered with `gzip on`, not with the defaults: see
    /// `the_gzip_sub_directives_are_absent_while_gzip_is_off` for why the
    /// default render is legitimately missing two of them.
    #[test]
    fn every_editable_directive_appears_in_the_generated_config() {
        let settings = WebServerSettings {
            gzip: crate::settings::OnOff::new(true),
            ..WebServerSettings::default()
        };
        let main = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &settings)
            .unwrap()
            .contents;
        for d in EDITABLE_DIRECTIVES {
            assert!(
                main.lines().any(|l| l.trim().starts_with(&format!("{d} "))),
                "the generated main config writes no `{d}` directive, so a rejection on it \
                 could never be mapped back to the {d} form field"
            );
        }
    }

    /// `gzip_comp_level` and `gzip_types` are written only while gzip is on
    /// (`NginxAdapter::gzip_extra`), so with gzip off this check cannot catch
    /// a bad value in either.
    ///
    /// That is the correct behaviour rather than a hole, and the reason is
    /// worth stating: the apply pipeline renders through the SAME
    /// `gzip_extra`, so a directive that is not rendered is one nginx never
    /// parses — it cannot break a later apply either. The guarantee this
    /// module offers is therefore "checked exactly when it is live", which is
    /// as much as any pre-check on generated output can offer. Turning gzip on
    /// renders both directives and brings them under the check in the same
    /// save.
    /// A settings struct that renders every editable directive.
    fn gzip_on() -> WebServerSettings {
        WebServerSettings {
            gzip: crate::settings::OnOff::new(true),
            ..WebServerSettings::default()
        }
    }

    /// The whole plumbing against the REAL binary: render the user's values
    /// into a throwaway home and have nginx accept them.
    ///
    /// This is what proves the throwaway home is complete — that the
    /// directories `nginx -t` insists on exist, and that the `include` globs
    /// pointing at an empty generated-sites directory do not themselves fail.
    /// Any of those wrong and every save would be rejected for a reason that
    /// has nothing to do with the user's values.
    #[tokio::test]
    async fn real_nginx_accepts_a_candidate_render_of_valid_settings() {
        let Some(brew) = crate::validate::find_brew_binaries() else {
            eprintln!("SKIP check_settings: Homebrew nginx not found (brew install nginx)");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let got = check_settings(&brew.nginx, root.path(), &gzip_on())
            .await
            .unwrap();
        assert!(
            matches!(got, SettingsCheck::Accepted { .. }),
            "real nginx rejected a candidate render of valid settings: {got:?}"
        );
    }

    /// `check_settings` must leave nothing behind in the scratch root.
    #[tokio::test]
    async fn a_check_removes_its_own_scratch_directory() {
        let Some(brew) = crate::validate::find_brew_binaries() else {
            eprintln!("SKIP check_settings: Homebrew nginx not found (brew install nginx)");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        check_settings(&brew.nginx, root.path(), &gzip_on())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "the scratch root must be empty once the check has finished"
        );
    }

    /// The line-number mapping, against messages REAL nginx produced.
    ///
    /// The unit tests above assert on stderr strings written by hand, so on
    /// their own they only prove the parser matches my transcription of
    /// nginx's format. This drives the same parser with output nginx actually
    /// emitted, for both message shapes it uses: one that names the offending
    /// directive and one that reports only a line.
    #[tokio::test]
    async fn maps_real_nginx_rejections_back_to_their_field() {
        let Some(brew) = crate::validate::find_brew_binaries() else {
            eprintln!("SKIP check_settings: Homebrew nginx not found (brew install nginx)");
            return;
        };
        // (from, to, expected field). `from` is what the settings render;
        // `to` is a value nginx refuses.
        let cases = [
            // nginx reports this one WITHOUT naming the directive.
            ("gzip_comp_level 1;", "gzip_comp_level 99;", "gzip_comp_level"),
            // ...and this one WITH the directive named.
            (
                "client_max_body_size 256m;",
                "client_max_body_size 99999999999999999999g;",
                "client_max_body_size",
            ),
        ];
        for (from, to, expected) in cases {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path();
            let main = NginxAdapter.generate_main_config(dir, &gzip_on()).unwrap();
            assert!(
                main.contents.contains(from),
                "the generated config no longer contains `{from}`, so this test would \
                 corrupt nothing and could not fail"
            );
            let corrupted = main.contents.replace(from, to);
            std::fs::create_dir_all(main.path.parent().unwrap()).unwrap();
            std::fs::write(&main.path, &corrupted).unwrap();
            for d in ["run", "run/nginx", "logs"] {
                std::fs::create_dir_all(dir.join(d)).unwrap();
            }
            let err_log = dir.join("logs/nginx.error.log");
            let report = crate::inspect::validate_live(&brew.nginx, &main.path, &err_log)
                .await
                .unwrap();
            assert!(!report.ok, "nginx accepted `{to}`; this test proves nothing");
            assert_eq!(
                field_for_rejection(&corrupted, &report.stderr),
                Some(expected),
                "real nginx said:\n{}",
                report.stderr
            );
        }
    }

    /// The rejected path, without needing nginx installed — so CI exercises it
    /// even where the gated tests above skip.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_non_zero_validator_exit_is_a_rejection_on_the_named_field() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let settings = gzip_on();

        // The line the fake nginx will point at, LOOKED UP rather than
        // hardcoded: a hardcoded number silently starts naming whichever
        // directive the next template edit moves onto that line, and the test
        // would keep passing while asserting the wrong thing.
        let rendered = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &settings)
            .unwrap()
            .contents;
        let line_no = 1 + rendered
            .lines()
            .position(|l| l.trim().starts_with("gzip_comp_level "))
            .expect("the main config renders gzip_comp_level while gzip is on");

        let msg = format!("nginx: [emerg] value must be between 1 and 9 in /h/nginx.conf:{line_no}");
        let bin = root.path().join("fake-nginx");
        std::fs::write(&bin, format!("#!/bin/sh\necho '{msg}' >&2\nexit 1\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let got = check_settings(&bin, root.path(), &settings).await.unwrap();
        assert_eq!(
            got,
            SettingsCheck::Rejected {
                field: Some("gzip_comp_level"),
                stderr: format!("{msg}\n"),
            }
        );
    }

    #[test]
    fn the_gzip_sub_directives_are_absent_while_gzip_is_off() {
        let off = WebServerSettings {
            gzip: crate::settings::OnOff::new(false),
            ..WebServerSettings::default()
        };
        let main = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &off)
            .unwrap()
            .contents;
        for d in ["gzip_comp_level", "gzip_types"] {
            assert!(
                !main.lines().any(|l| l.trim().starts_with(&format!("{d} "))),
                "{d} must not be rendered while gzip is off"
            );
        }
    }
}
