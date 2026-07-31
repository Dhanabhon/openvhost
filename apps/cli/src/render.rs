// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning one [`Exchange`] into bytes (spec D5).
//!
//! Two surfaces, one rule: whatever the mode, output goes to stderr **exactly
//! when** the exit code is non-zero. In `--json` that means stderr stays
//! empty, because the error envelope goes to stdout so a `jq` pipeline still
//! parses on failure.

use std::fmt::Write as _;
use std::path::Path;

use openvhost_proc::control::{Disposition, Response, envelope_json};
use openvhost_proc::events::{ServiceState, ServiceStatus};

use crate::cli::Header;
use crate::exchange::{Exchange, SupervisorReport};

/// What to write where. Neither string is ever partially flushed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Written to stdout verbatim.
    pub out: String,
    /// Written to stderr verbatim.
    pub err: String,
}

/// Column heading of the service table, also its minimum widths.
const COLUMNS: [&str; 4] = ["ID", "STATE", "PID", "ENDPOINT"];

/// Placeholder for an absent optional column value.
const NONE: &str = "-";

/// One single-line JSON object — the whole of stdout in `--json` mode.
///
/// The envelope comes from [`envelope_json`], the protocol's own builder, so
/// what this prints for a server answer is byte-for-byte what the server sent,
/// and what it prints for an answer the CLI had to synthesize (no app running)
/// has the identical shape. The only additions are the `header` keys, and only
/// for the two verbs that report on the supervisor itself (spec D3/D4).
pub fn json_line(command: &str, header: Header, home: &Path, ex: &Exchange) -> String {
    let mut value = envelope_json(command, &ex.response);
    if let Some(obj) = value.as_object_mut() {
        match header {
            Header::Bare => {}
            Header::Supervisor => {
                obj.insert("supervisor".into(), ex.supervisor.as_str().into());
            }
            Header::Status => {
                obj.insert("supervisor".into(), ex.supervisor.as_str().into());
                obj.insert("home".into(), home.display().to_string().into());
                obj.insert("version".into(), env!("CARGO_PKG_VERSION").into());
            }
        }
    }
    // `to_string` never emits a raw newline: everything inside a string value
    // is escaped, so a multi-line stderr tail stays on this one line.
    serde_json::to_string(&value).unwrap_or_else(|e| {
        serde_json::json!({
            "schemaVersion": openvhost_proc::control::SCHEMA_VERSION,
            "ok": false,
            "command": command,
            "error": { "code": "operationFailed",
                       "message": format!("could not encode the answer: {e}") },
        })
        .to_string()
    })
}

/// The human rendering.
///
/// Output lands on stderr exactly when the exit code will be non-zero, so a
/// quiet run means a successful one.
pub fn human(header: Header, home: &Path, ex: &Exchange) -> Rendered {
    let mut r = Rendered::default();
    if header == Header::Status {
        let _ = writeln!(r.out, "OpenVHost {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(r.out, "home:       {}", home.display());
        let _ = writeln!(r.out, "supervisor: {}", ex.supervisor.describe());
        let _ = writeln!(r.out);
    }
    match &ex.response {
        Response::Services { services } => render_services(&mut r, ex, services),
        Response::Transition {
            service,
            disposition,
        } => render_transition(&mut r, service, *disposition),
        Response::StopAll { stragglers } => {
            if stragglers.is_empty() {
                let _ = writeln!(r.out, "All services stopped.");
            } else {
                let _ = writeln!(
                    r.err,
                    "openvhost: these services did not stop within the deadline: {}",
                    stragglers.join(", ")
                );
            }
        }
        Response::Error { code: _, message } => {
            let _ = writeln!(r.err, "openvhost: {message}");
        }
    }
    r
}

/// The service table, plus a detail block for anything that failed.
fn render_services(r: &mut Rendered, ex: &Exchange, services: &[ServiceStatus]) {
    if let Some(note) = &ex.note {
        // Only reachable for `status`/`list` with no app: D3's loud human
        // line, on stdout because this is the answer and the exit code is 0.
        // Capitalized because the same string is also used after an
        // `openvhost: ` prefix, where a leading capital would read wrong.
        let _ = writeln!(r.out, "{}", sentence_case(note));
    }
    if services.is_empty() {
        if ex.supervisor == SupervisorReport::Running {
            let _ = writeln!(r.out, "No services are registered.");
        } else if ex.note.is_none() {
            let _ = writeln!(
                r.out,
                "No services: the supervisor is {}.",
                ex.supervisor.describe()
            );
        }
        return;
    }
    let rows: Vec<[String; 4]> = services
        .iter()
        .map(|s| {
            [
                s.id.clone(),
                state_label(&s.state),
                s.pid.map_or_else(|| NONE.to_owned(), |p| p.to_string()),
                s.endpoint.clone().unwrap_or_else(|| NONE.to_owned()),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..COLUMNS.len())
        .map(|i| {
            rows.iter()
                .map(|row| row[i].chars().count())
                .chain(std::iter::once(COLUMNS[i].chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let _ = writeln!(r.out, "{}", pad_row(&COLUMNS.map(str::to_owned), &widths));
    for row in &rows {
        let _ = writeln!(r.out, "{}", pad_row(row, &widths));
    }
    for s in services.iter() {
        if let ServiceState::Failed { exit, stderr_tail } = &s.state {
            let _ = writeln!(r.out);
            write_failure(&mut r.out, &s.id, *exit, stderr_tail);
        }
    }
}

/// One `start` / `stop` / `restart` answer.
fn render_transition(r: &mut Rendered, service: &ServiceStatus, disposition: Disposition) {
    if let ServiceState::Failed { exit, stderr_tail } = &service.state {
        // The run failed, so this is stderr's business (exit 70).
        write_failure(&mut r.err, &service.id, *exit, stderr_tail);
        return;
    }
    let state = state_label(&service.state);
    let pid = service
        .pid
        .map_or_else(String::new, |p| format!(" (pid {p})"));
    match disposition {
        Disposition::Changed => {
            let _ = writeln!(r.out, "{} is now {state}{pid}", service.id);
        }
        Disposition::Unchanged => {
            let _ = writeln!(
                r.out,
                "{} is already {state}{pid}; nothing to do",
                service.id
            );
        }
    }
}

/// The failure detail block: what exited, and the real stderr it left behind.
fn write_failure(into: &mut String, id: &str, exit: Option<i32>, stderr_tail: &[String]) {
    let code = exit.map_or_else(|| "no exit status".to_owned(), |c| format!("exit {c}"));
    if stderr_tail.is_empty() {
        let _ = writeln!(into, "{id} failed ({code}) with no captured output");
        return;
    }
    let _ = writeln!(into, "{id} failed ({code}):");
    for line in stderr_tail {
        let _ = writeln!(into, "    {line}");
    }
}

/// One cell per column, space-padded; the last column is never padded so
/// there is no trailing whitespace to confuse a `diff`.
fn pad_row(row: &[String; 4], widths: &[usize]) -> String {
    let mut out = String::new();
    for (i, cell) in row.iter().enumerate() {
        if i + 1 == row.len() {
            out.push_str(cell);
        } else {
            let pad = widths
                .get(i)
                .copied()
                .unwrap_or(0)
                .saturating_sub(cell.chars().count());
            let _ = write!(out, "{cell}{:pad$}  ", "", pad = pad);
        }
    }
    out
}

/// Upper-case the first character, leaving the rest alone.
///
/// Unicode-aware via `to_uppercase`, which can expand one character into
/// several (German sharp s), so the first character is replaced rather than
/// overwritten in place.
fn sentence_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The `STATE` cell. Exhaustive over [`ServiceState`] — a new state must be
/// given a spelling here rather than silently rendering as something else.
fn state_label(state: &ServiceState) -> String {
    match state {
        ServiceState::Stopped => "stopped".to_owned(),
        ServiceState::Starting => "starting".to_owned(),
        ServiceState::Running => "running".to_owned(),
        // Spelled the same way as the detail block below the table: a bare
        // `failed (1)` reads as a count rather than an exit status.
        ServiceState::Failed { exit, .. } => match exit {
            Some(c) => format!("failed (exit {c})"),
            None => "failed".to_owned(),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::exit::{Exit, exit_for};
    use openvhost_proc::SupervisorPresence;
    use openvhost_proc::control::{ControlError, ErrorCode, SCHEMA_VERSION};

    fn home() -> &'static Path {
        Path::new("/Users/x/.openvhost")
    }

    fn svc(id: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.into(),
            display_name: id.into(),
            endpoint: Some(format!("http://127.0.0.1/{id}")),
            pid: Some(4242),
            state,
        }
    }

    fn not_running() -> ControlError {
        ControlError::NotRunning {
            path: home().join("run/control.sock"),
        }
    }

    fn failed() -> ServiceState {
        ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec![
                "nginx: [emerg] bind() to 0.0.0.0:80 failed (48: Address already in use)".into(),
                "nginx: configuration file test failed".into(),
            ],
        }
    }

    /// Every response shape the renderer can meet, with the header it would
    /// arrive under.
    fn every_rendering() -> Vec<(&'static str, Header, Exchange)> {
        vec![
            (
                "list",
                Header::Supervisor,
                Exchange::answered(Response::Services {
                    services: vec![
                        svc("mysql-8.4", ServiceState::Running),
                        svc("nginx", failed()),
                        svc("php-fpm-8.4", ServiceState::Stopped),
                    ],
                }),
            ),
            (
                "status",
                Header::Status,
                Exchange::answered(Response::Services { services: vec![] }),
            ),
            (
                "start",
                Header::Bare,
                Exchange::answered(Response::Transition {
                    service: svc("nginx", ServiceState::Running),
                    disposition: Disposition::Changed,
                }),
            ),
            (
                "stop",
                Header::Bare,
                Exchange::answered(Response::Transition {
                    service: svc("nginx", ServiceState::Stopped),
                    disposition: Disposition::Unchanged,
                }),
            ),
            (
                "start",
                Header::Bare,
                Exchange::answered(Response::Transition {
                    service: svc("nginx", failed()),
                    disposition: Disposition::Changed,
                }),
            ),
            (
                "stopAll",
                Header::Bare,
                Exchange::answered(Response::StopAll { stragglers: vec![] }),
            ),
            (
                "stopAll",
                Header::Bare,
                Exchange::answered(Response::StopAll {
                    stragglers: vec!["mysql-8.4".into()],
                }),
            ),
            (
                "start",
                Header::Bare,
                Exchange::answered(Response::error(ErrorCode::UnknownService, "no such thing")),
            ),
            (
                "status",
                Header::Status,
                Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent)
                    .absent_supervisor_is_an_answer(),
            ),
            (
                "list",
                Header::Supervisor,
                Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent)
                    .absent_supervisor_is_an_answer(),
            ),
            (
                "start",
                Header::Bare,
                Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent),
            ),
        ]
    }

    /// D5's hardest rule: one object, one line, whatever is in the payload.
    #[test]
    fn json_is_always_exactly_one_line() {
        for (command, header, ex) in every_rendering() {
            let line = json_line(command, header, home(), &ex);
            assert!(!line.is_empty(), "empty JSON for {command}");
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "{command} rendered {line:?} across more than one line"
            );
            let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(parsed["schemaVersion"], SCHEMA_VERSION);
            assert_eq!(parsed["command"], command);
        }
    }

    /// A stderr tail full of newlines is exactly the payload that would break
    /// the one-line rule if it were interpolated rather than encoded.
    #[test]
    fn a_message_containing_newlines_still_renders_as_one_line() {
        let ex = Exchange::answered(Response::error(
            ErrorCode::OperationFailed,
            "nginx: [emerg] bind() failed\nnginx: configuration file test failed\n",
        ));
        let line = json_line("start", Header::Bare, home(), &ex);
        assert!(!line.contains('\n'), "{line:?}");
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(
            parsed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("configuration file test failed")
        );
    }

    /// The envelope is the protocol's own builder plus the CLI's header keys —
    /// never a hand-rolled parallel shape.
    #[test]
    fn the_envelope_is_the_protocols_own_with_only_the_header_keys_added() {
        let ex = Exchange::answered(Response::StopAll { stragglers: vec![] });
        let bare: serde_json::Value =
            serde_json::from_str(&json_line("stopAll", Header::Bare, home(), &ex)).unwrap();
        assert_eq!(bare, envelope_json("stopAll", &ex.response));
    }

    /// `list` reports the supervisor; `status` adds the home and version
    /// header (spec D4); control verbs pass the server's envelope through
    /// untouched.
    #[test]
    fn each_header_adds_exactly_its_documented_keys() {
        let ex = Exchange::answered(Response::Services { services: vec![] });
        let keys = |header| {
            let v: serde_json::Value =
                serde_json::from_str(&json_line("status", header, home(), &ex)).unwrap();
            let mut k: Vec<String> = v
                .as_object()
                .unwrap()
                .keys()
                .map(ToString::to_string)
                .collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(Header::Bare),
            ["command", "ok", "result", "schemaVersion"]
        );
        assert_eq!(
            keys(Header::Supervisor),
            ["command", "ok", "result", "schemaVersion", "supervisor"]
        );
        assert_eq!(
            keys(Header::Status),
            [
                "command",
                "home",
                "ok",
                "result",
                "schemaVersion",
                "supervisor",
                "version"
            ]
        );
    }

    /// The one field a script is meant to branch on when there is no app.
    #[test]
    fn an_absent_supervisor_is_reported_as_not_running_with_an_empty_list() {
        let ex = Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent)
            .absent_supervisor_is_an_answer();
        for header in [Header::Supervisor, Header::Status] {
            let v: serde_json::Value =
                serde_json::from_str(&json_line("status", header, home(), &ex)).unwrap();
            assert_eq!(v["ok"], true);
            assert_eq!(v["supervisor"], "notRunning");
            assert_eq!(v["result"]["kind"], "services");
            assert_eq!(v["result"]["services"], serde_json::json!([]));
        }
    }

    /// The invariant that makes shell use predictable: noise on stderr means
    /// and only means failure.
    #[test]
    fn stderr_is_used_exactly_when_the_exit_code_is_non_zero() {
        for (command, header, ex) in every_rendering() {
            let r = human(header, home(), &ex);
            let failed = exit_for(&ex.response) != Exit::Ok;
            assert_eq!(
                !r.err.is_empty(),
                failed,
                "{command} exit={:?} out={:?} err={:?}",
                exit_for(&ex.response),
                r.out,
                r.err
            );
        }
    }

    /// The `STATE` cell and the detail block must not disagree about how a
    /// failure is spelled.
    #[test]
    fn the_state_column_names_the_exit_status_the_same_way_the_detail_block_does() {
        let ex = Exchange::answered(Response::Services {
            services: vec![svc("nginx", failed())],
        });
        let out = human(Header::Supervisor, home(), &ex).out;
        assert_eq!(
            out.matches("failed (exit 1)").count(),
            2,
            "once in the table, once in the detail block: {out}"
        );
    }

    /// The reason `ServiceStatus` is reused verbatim: the real stderr has to
    /// reach the human who ran the command.
    #[test]
    fn a_failed_service_shows_its_stderr_tail() {
        let listing = Exchange::answered(Response::Services {
            services: vec![svc("nginx", failed())],
        });
        let r = human(Header::Supervisor, home(), &listing);
        assert!(
            r.out.contains("Address already in use"),
            "listing: {:?}",
            r.out
        );
        assert!(
            r.out.contains("configuration file test failed"),
            "{:?}",
            r.out
        );

        let transition = Exchange::answered(Response::Transition {
            service: svc("nginx", failed()),
            disposition: Disposition::Changed,
        });
        let r = human(Header::Bare, home(), &transition);
        assert!(
            r.err.contains("Address already in use"),
            "transition: {:?}",
            r.err
        );
    }

    /// The table has to be a table: one header row, one row per service, in
    /// the order given.
    #[test]
    fn the_service_table_has_a_header_and_one_row_per_service() {
        let ex = Exchange::answered(Response::Services {
            services: vec![
                svc("mysql-8.4", ServiceState::Running),
                svc("php-fpm-8.4", ServiceState::Stopped),
            ],
        });
        let r = human(Header::Supervisor, home(), &ex);
        let lines: Vec<&str> = r.out.lines().collect();
        assert!(
            lines[0].contains("ID") && lines[0].contains("STATE"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("mysql-8.4"), "{lines:?}");
        assert!(lines[1].contains("running"), "{lines:?}");
        assert!(lines[1].contains("4242"), "{lines:?}");
        assert!(lines[2].starts_with("php-fpm-8.4"), "{lines:?}");
    }

    /// D3's "loud human line". Without it, `openvhost list` on a machine with
    /// no app running would print an empty table and look broken.
    #[test]
    fn no_app_prints_a_loud_line_on_stdout_for_both_reporting_verbs() {
        let ex = Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent)
            .absent_supervisor_is_an_answer();
        for header in [Header::Supervisor, Header::Status] {
            let r = human(header, home(), &ex);
            assert!(r.err.is_empty(), "{header:?} err={:?}", r.err);
            assert!(r.out.contains("not running"), "{header:?} out={:?}", r.out);
        }
    }

    #[test]
    fn sentence_case_only_touches_the_first_character() {
        assert_eq!(
            sentence_case("the app is not running"),
            "The app is not running"
        );
        assert_eq!(sentence_case("OpenVHost is fine"), "OpenVHost is fine");
        assert_eq!(sentence_case(""), "");
        assert_eq!(
            sentence_case("\u{00e9}chec du d\u{00e9}marrage"),
            "\u{00c9}chec du d\u{00e9}marrage"
        );
    }

    /// The loud line is a sentence in its own right, not a fragment following
    /// an `openvhost: ` prefix.
    #[test]
    fn the_loud_no_app_line_reads_as_a_sentence() {
        let ex = Exchange::from_client_error(&not_running(), || SupervisorPresence::Absent)
            .absent_supervisor_is_an_answer();
        let out = human(Header::Supervisor, home(), &ex).out;
        assert!(
            out.starts_with("The OpenVHost app is not running."),
            "{out:?}"
        );
    }

    /// The probe's wording is why `SupervisorPresence` has three states; it
    /// has to reach the terminal.
    #[test]
    fn the_probes_wording_reaches_the_human_output() {
        let ex = Exchange::from_client_error(&not_running(), || SupervisorPresence::Present)
            .absent_supervisor_is_an_answer();
        let r = human(Header::Status, home(), &ex);
        assert!(r.out.contains("may still be starting"), "{:?}", r.out);
    }

    /// `status` carries the header spec D4 asks for; `list` is the table
    /// alone, not a silent alias.
    #[test]
    fn status_prints_a_header_and_list_does_not() {
        let ex = Exchange::answered(Response::Services {
            services: vec![svc("nginx", ServiceState::Running)],
        });
        let status = human(Header::Status, home(), &ex).out;
        assert!(status.contains("/Users/x/.openvhost"), "{status}");
        assert!(status.contains(env!("CARGO_PKG_VERSION")), "{status}");
        assert!(status.contains("supervisor"), "{status}");

        let list = human(Header::Supervisor, home(), &ex).out;
        assert!(!list.contains("/Users/x/.openvhost"), "{list}");
    }

    /// `unchanged` is a success and has to read like one.
    #[test]
    fn an_unchanged_transition_says_so_without_sounding_like_a_failure() {
        let ex = Exchange::answered(Response::Transition {
            service: svc("nginx", ServiceState::Running),
            disposition: Disposition::Unchanged,
        });
        let r = human(Header::Bare, home(), &ex);
        assert!(r.err.is_empty());
        assert!(r.out.contains("already"), "{:?}", r.out);
        assert!(r.out.contains("nginx"), "{:?}", r.out);
    }
}
