// SPDX-License-Identifier: GPL-3.0-or-later
//! The argument surface: `clap` derive, and the pure mapping from a parsed
//! invocation onto a wire [`Request`].

use clap::{Parser, Subcommand};
use openvhost_proc::control::{ControlError, Request, ServiceId};

/// Long-form `--help` epilogue. Documents the exit table and the JSON
/// stability contract (spec D5) where a user will actually look for them.
const AFTER_HELP: &str = "\
EXIT CODES:
   0  success, including an explicit \"already in that state\"
  64  usage error
  66  unknown service id
  69  the app is not running, or its control channel would not answer
  70  the operation failed, or the protocol was violated
  75  a conflicting operation is in flight, or the transition timed out
  77  authorization denied

JSON OUTPUT (--json):
  Exactly one single-line JSON object on stdout and nothing on stderr, on
  success and on failure alike, so a `jq` pipeline still parses when a verb
  fails; the exit code is the primary signal. Within schemaVersion 1, fields
  are added, never removed or retyped, and error.code values are added, never
  repurposed. Ignore unknown fields, do not rely on key order or on human
  messages, and treat an unknown error.code as a generic failure.

NOTE:
  `status` and `list` answer with an empty service list and exit 0 when the
  OpenVHost app is not running -- that is the answer, not an error. Every
  other verb exits 69. This command never launches the app.";

/// One `openvhost` invocation.
#[derive(Debug, Parser)]
#[command(
    name = "openvhost",
    version,
    about = "Control a running OpenVHost app from the shell.",
    after_long_help = AFTER_HELP
)]
pub struct Cli {
    /// Print one single-line JSON object instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Return as soon as the transition is kicked off, without waiting for the
    /// service to reach its terminal state. Ignored by `status`, `list` and
    /// `stop-all`.
    #[arg(long = "no-wait", global = true)]
    pub no_wait: bool,

    /// The verb to run.
    #[command(subcommand)]
    pub verb: Verb,
}

/// The verbs. Ids are exact registered service keys — `nginx`, `php-fpm-8.4`,
/// `mysql-8.4` — never site names (spec D4).
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Verb {
    /// Show the supervisor header and every service row, or just one.
    Status {
        /// Restrict to one service id.
        id: Option<String>,
    },
    /// Show the service table alone, id-sorted.
    List,
    /// Start one service.
    Start {
        /// The service id.
        id: String,
    },
    /// Stop one service.
    Stop {
        /// The service id.
        id: String,
    },
    /// Stop then start one service.
    Restart {
        /// The service id.
        id: String,
    },
    /// Stop every service.
    StopAll,
}

/// Which CLI-added keys an answer carries beyond the server's envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// Control verbs: the server's envelope verbatim, nothing added.
    Bare,
    /// `list`: plus `supervisor`.
    Supervisor,
    /// `status`: plus `supervisor`, `home` and `version` (spec D4's header).
    Status,
}

impl Cli {
    /// The wire request for this invocation.
    ///
    /// Ids are parsed here, at the CLI's own ingress, so a syntactically
    /// impossible one never reaches the socket — the server parses them again
    /// on its side because it does not trust this one.
    pub fn request(&self) -> Result<Request, ControlError> {
        let wait = !self.no_wait;
        Ok(match &self.verb {
            Verb::Status { id } => Request::Status {
                id: id.as_deref().map(ServiceId::parse).transpose()?,
            },
            Verb::List => Request::List,
            Verb::Start { id } => Request::Start {
                id: ServiceId::parse(id)?,
                wait,
            },
            Verb::Stop { id } => Request::Stop {
                id: ServiceId::parse(id)?,
                wait,
            },
            Verb::Restart { id } => Request::Restart {
                id: ServiceId::parse(id)?,
                wait,
            },
            Verb::StopAll => Request::StopAll,
        })
    }

    /// The `command` string echoed in the envelope.
    ///
    /// Needed even when [`Cli::request`] fails — an envelope reporting a
    /// rejected service id still has to say which verb was rejected — so this
    /// cannot simply delegate to [`Request::command_name`], which needs a
    /// built request. `command_name_agrees_with_the_protocols_own_spelling`
    /// ties the two together for every verb so they cannot drift.
    pub fn command_name(&self) -> &'static str {
        match &self.verb {
            Verb::Status { .. } => "status",
            Verb::List => "list",
            Verb::Start { .. } => "start",
            Verb::Stop { .. } => "stop",
            Verb::Restart { .. } => "restart",
            Verb::StopAll => "stopAll",
        }
    }

    /// What this verb adds to the envelope, and therefore whether an absent
    /// supervisor is an *answer* (exit 0) or a failure (exit 69) — spec D3.
    pub fn header(&self) -> Header {
        match &self.verb {
            Verb::Status { .. } => Header::Status,
            Verb::List => Header::Supervisor,
            Verb::Start { .. } | Verb::Stop { .. } | Verb::Restart { .. } | Verb::StopAll => {
                Header::Bare
            }
        }
    }
}

impl Header {
    /// Does this verb report on the supervisor's presence at all? Only those
    /// verbs treat "the app is not running" as a successful answer.
    pub fn reports_supervisor(self) -> bool {
        match self {
            Header::Supervisor | Header::Status => true,
            Header::Bare => false,
        }
    }
}

/// Does this argv ask for JSON?
///
/// Read straight off the raw arguments rather than from the parsed [`Cli`],
/// because a usage error has to be reported *before* parsing succeeds and
/// still has to come out as JSON when JSON was asked for (spec D5: stderr
/// stays empty in JSON mode).
///
/// Exact token match, and only before a `--` separator: after one, `--json` is
/// a positional value (a service id), not our flag.
pub fn json_requested(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "--json")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("openvhost").chain(args.iter().copied())).unwrap()
    }

    fn id(s: &str) -> ServiceId {
        ServiceId::parse(s).unwrap()
    }

    /// Every verb, so a new one cannot be added without turning up here.
    fn every_verb() -> Vec<(&'static [&'static str], Request, &'static str, Header)> {
        vec![
            (
                &["status"],
                Request::Status { id: None },
                "status",
                Header::Status,
            ),
            (
                &["status", "nginx"],
                Request::Status {
                    id: Some(id("nginx")),
                },
                "status",
                Header::Status,
            ),
            (&["list"], Request::List, "list", Header::Supervisor),
            (
                &["start", "nginx"],
                Request::Start {
                    id: id("nginx"),
                    wait: true,
                },
                "start",
                Header::Bare,
            ),
            (
                &["stop", "php-fpm-8.4"],
                Request::Stop {
                    id: id("php-fpm-8.4"),
                    wait: true,
                },
                "stop",
                Header::Bare,
            ),
            (
                &["restart", "mysql-8.4"],
                Request::Restart {
                    id: id("mysql-8.4"),
                    wait: true,
                },
                "restart",
                Header::Bare,
            ),
            (&["stop-all"], Request::StopAll, "stopAll", Header::Bare),
        ]
    }

    #[test]
    fn every_verb_maps_to_its_request_command_name_and_header() {
        for (args, want_req, want_name, want_header) in every_verb() {
            let cli = parse(args);
            assert_eq!(cli.request().unwrap(), want_req, "request for {args:?}");
            assert_eq!(cli.command_name(), want_name, "command name for {args:?}");
            assert_eq!(cli.header(), want_header, "header for {args:?}");
        }
    }

    /// The echoed `command` must be the protocol's own spelling, not a second
    /// hand-maintained list that can drift from it.
    #[test]
    fn command_name_agrees_with_the_protocols_own_spelling() {
        for (args, _, _, _) in every_verb() {
            let cli = parse(args);
            assert_eq!(
                cli.command_name(),
                cli.request().unwrap().command_name(),
                "for {args:?}"
            );
        }
    }

    /// `--no-wait` has to reach the wire, or it is a flag that silently does
    /// nothing.
    #[test]
    fn no_wait_clears_the_wait_flag_on_every_transition_verb() {
        for verb in ["start", "stop", "restart"] {
            let cli = parse(&[verb, "nginx", "--no-wait"]);
            let wait = match cli.request().unwrap() {
                Request::Start { wait, .. }
                | Request::Stop { wait, .. }
                | Request::Restart { wait, .. } => wait,
                other => panic!("{verb} produced {other:?}"),
            };
            assert!(!wait, "{verb} --no-wait must send wait:false");
        }
    }

    /// A syntactically impossible id is rejected before it reaches the socket.
    #[test]
    fn a_malformed_service_id_is_a_typed_error_not_a_request() {
        let cli = parse(&["start", "nginx\nstop"]);
        match cli.request() {
            Err(ControlError::InvalidServiceId(_)) => {}
            other => panic!("expected InvalidServiceId, got {other:?}"),
        }
    }

    #[test]
    fn json_is_global_so_it_may_follow_the_verb() {
        assert!(parse(&["list", "--json"]).json);
        assert!(parse(&["--json", "list"]).json);
        assert!(!parse(&["list"]).json);
    }

    /// Read straight off argv, because the flag has to be known *before*
    /// `clap` gets a chance to fail on a bad verb — an error still has to come
    /// out as JSON when JSON was asked for.
    #[test]
    fn json_requested_reads_the_flag_off_raw_argv() {
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert!(json_requested(&owned(&["list", "--json"])));
        assert!(json_requested(&owned(&["--json", "bogus-verb"])));
        assert!(!json_requested(&owned(&["list"])));
        // Not a prefix match: `--jsonish` is a different flag.
        assert!(!json_requested(&owned(&["list", "--jsonish"])));
        // After a `--` separator it is a positional value, not our flag.
        assert!(!json_requested(&owned(&["start", "--", "--json"])));
    }

    /// `clap`'s own wiring: the derive must actually build a valid command.
    #[test]
    fn the_command_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    /// A bad verb, a missing argument and a missing verb are all usage errors
    /// — never a silent default.
    #[test]
    fn bad_arguments_are_parse_errors() {
        for args in [
            vec!["bogus-verb"],
            vec!["start"],
            vec![],
            vec!["start", "nginx", "extra"],
            vec!["--nope", "list"],
        ] {
            let parsed =
                Cli::try_parse_from(std::iter::once("openvhost").chain(args.iter().copied()));
            assert!(parsed.is_err(), "{args:?} should not parse");
        }
    }

    /// `--help` and `--version` are not failures; they must not become exit
    /// 64. A bare invocation is the trap: `clap` reports it with a *third*
    /// kind that also renders help, and treating that as a display would exit
    /// 0 having run nothing.
    #[test]
    fn help_and_version_are_display_kinds_but_a_bare_invocation_is_not() {
        use clap::error::ErrorKind;
        for (args, want) in [
            (vec!["--help"], ErrorKind::DisplayHelp),
            (vec!["--version"], ErrorKind::DisplayVersion),
        ] {
            let err = Cli::try_parse_from(std::iter::once("openvhost").chain(args.iter().copied()))
                .expect_err("help/version short-circuit parsing");
            assert_eq!(err.kind(), want, "for {args:?}");
        }
        let bare = Cli::try_parse_from(["openvhost"]).expect_err("a bare invocation names no verb");
        assert_ne!(bare.kind(), ErrorKind::DisplayHelp);
        assert_ne!(bare.kind(), ErrorKind::DisplayVersion);
    }

    /// `__testchild` is an internal fixture, handled before `clap` ever runs.
    /// It must not be advertised.
    #[test]
    fn testchild_is_not_advertised_in_help() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("__testchild"), "{help}");
    }

    /// The exit table is a public contract; `--help` is where it is published.
    #[test]
    fn long_help_publishes_the_exit_table_and_the_no_app_rule() {
        let help = Cli::command().render_long_help().to_string();
        for needle in ["69", "66", "exit 0", "never launches"] {
            assert!(help.contains(needle), "long help is missing {needle:?}");
        }
    }
}
