// SPDX-License-Identifier: GPL-3.0-or-later
//! Authenticated local control channel — the seam the `openvhost` CLI talks
//! to the running app through (spec `docs/superpowers/specs/2026-07-31-p1-cli-design.md`).
//!
//! # Shape
//!
//! One app-owned `SOCK_STREAM` unix socket at `<home>/run/control.sock`,
//! bound **inside** the `Ok(Some(lock))` arm of app startup so the socket
//! exists *iff* a supervisor exists (D1). One bounded JSON request line in,
//! one JSON response line out, connection closed. No session state, no
//! multiplexing.
//!
//! **Removing it is the quit path's job, not [`serve`]'s.** A unix socket is
//! not unlinked when its process exits, and the app serves for its whole
//! process lifetime ([`std::future::pending`]), so `serve`'s own unlink is
//! unreachable in production. Whoever quits must unlink
//! [`ControlListener::socket`] — otherwise every quit leaves a path that
//! `connect(2)` refuses, and the CLI reports "not accepting control
//! connections" (exit 69) where the truth is "not running" (exit 0).
//!
//! # Layering
//!
//! Transport, parsing and authorization live here. *Policy* lives in the
//! desktop app behind [`ControlHandler`] — the bulk lock, `quit::stop_all`,
//! the supervisor itself. Nothing moves out of the desktop crate, and the
//! trait seam is also what lets the CLI be tested against a fake handler
//! (D6).
//!
//! # Security posture (D2)
//!
//! - `<home>/run` is `0700` and the socket is `0600`.
//! - The peer's effective uid must equal ours, checked by
//!   [`peer_is_authorized`] **before the request is read**, via
//!   [`tokio::net::UnixStream::peer_cred`] — no `unsafe`, no hand-rolled
//!   `LOCAL_PEERCRED`.
//! - Ingress is capped at [`MAX_REQUEST_BYTES`] **and** [`READ_TIMEOUT`], so
//!   a peer that connects and then says nothing cannot pin a task.
//! - [`Request`] cannot express a path, an argv, a pid or an environment.
//!   That is the containment invariant: this channel cannot make the
//!   supervisor spawn anything `stack.rs` did not already register.
//!
//! What this does **not** defend against, stated plainly so its absence is
//! not read as an oversight: a **same-uid** rogue process. It can read
//! `state.db`, read any token out of the same `0700` directory, or simply run
//! `nginx` itself — a token here would be security theatre. Peer credentials
//! are kept as defence in depth against a future permission regression. The
//! answer to a hostile same-uid process is the Phase 3 privileged helper, not
//! this socket.
//!
//! # Platform
//!
//! Unix only in v1, matching the project's macOS-first posture. Windows
//! (named pipe + `GetNamedPipeClientProcessId`) is deferred; the non-unix
//! build keeps every signature and returns
//! [`ControlError::UnsupportedPlatform`], the same explicit-refusal shape
//! [`InstanceLock::acquire`](crate::InstanceLock::acquire) already uses.

use std::path::{Path, PathBuf};
use std::time::Duration;

mod client;
mod error;
mod protocol;
mod server;

pub use error::ControlError;
pub use protocol::{
    Disposition, ErrorCode, MAX_SERVICE_ID_BYTES, Request, Response, SCHEMA_VERSION, ServiceId,
    envelope_json,
};
pub use server::{ControlHandler, ControlListener, ControlSocket, bind, serve};

pub use client::{CLIENT_RESPONSE_TIMEOUT, MAX_RESPONSE_BYTES, request};

/// Re-exported so an implementer of [`ControlHandler`] does not need its own
/// `async-trait` dependency:
///
/// ```ignore
/// #[openvhost_proc::control::async_trait]
/// impl ControlHandler for DesktopHandler {
///     async fn execute(&self, req: Request) -> Response { /* … */ }
/// }
/// ```
///
/// A hand-written `-> Pin<Box<dyn Future<Output = Response> + Send + '_>>`
/// works identically; the macro exists because `dyn ControlHandler` rules out
/// native `async fn` in traits (async-fn-in-trait is not dyn-compatible).
pub use async_trait::async_trait;

/// File name of the control socket inside `<home>/run`.
pub const SOCKET_FILE_NAME: &str = "control.sock";

/// Largest request accepted from a peer, mirroring `FileRegistry`'s own
/// 64 KiB ceiling. A request is a handful of short fields; anything larger is
/// either a bug or an attempt to make the server allocate.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// How long the server waits for a *complete* request line before giving up.
///
/// Also bounds the response write, so neither a peer that connects and stays
/// silent nor one that never reads its answer can pin a task (D2). This is a
/// deadline on the ingress only — the handler's own work (a `start` that
/// waits for readiness) is deliberately not bounded by it.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// `sun_path` ceiling: macOS caps it at 104 bytes including the NUL, so 103
/// usable bytes.
///
/// **Deliberately duplicated** from `openvhost_core::site::apply::MAX_SOCKET_PATH_BYTES`,
/// not shared: `openvhost-core` depends on `openvhost-proc`, not the reverse,
/// so core's copy is unreachable from here and inverting a crate dependency
/// for one integer would be absurd. 103 is a fixed Darwin ABI constant that
/// cannot drift, and de-duplicating it would mean editing the audited apply
/// path and the MySQL datadir guard for zero behavioural gain (spec D1).
pub const MAX_SOCKET_PATH_BYTES: usize = 103;

/// `<home>/run/control.sock`, length-checked against [`MAX_SOCKET_PATH_BYTES`].
///
/// A hermetic test with a tempdir `OPENVHOST_HOME` can genuinely approach the
/// ceiling — the php-fpm and mysqld sockets already do — so this fails with a
/// typed [`ControlError::SocketPathTooLong`] naming the real cause instead of
/// letting `bind(2)` surface an `EINVAL` surprise.
pub fn socket_path(home: &Path) -> Result<PathBuf, ControlError> {
    let path = home.join("run").join(SOCKET_FILE_NAME);
    let len = path.as_os_str().as_encoded_bytes().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(ControlError::SocketPathTooLong {
            path,
            len,
            max: MAX_SOCKET_PATH_BYTES,
        });
    }
    Ok(path)
}

/// The whole authorization decision, as a pure function so it is testable
/// without a socket (D2/D7).
///
/// Strict equality, and root is deliberately **not** special-cased: if the
/// app runs as root then only root may drive it, and if it runs as you then
/// even root has to become you first. The uid we compare against is the
/// socket's own owner — see [`ControlListener::owner_uid`].
pub fn peer_is_authorized(peer_uid: u32, our_uid: u32) -> bool {
    peer_uid == our_uid
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_run_control_sock_under_home() {
        let p = socket_path(Path::new("/Users/x/.openvhost")).unwrap();
        assert_eq!(p, Path::new("/Users/x/.openvhost/run/control.sock"));
    }

    #[test]
    fn socket_path_rejects_an_over_long_home_with_a_typed_error() {
        // 100 bytes of home + "/run/control.sock" (17) = 117 > 103.
        let home = PathBuf::from(format!("/{}", "h".repeat(99)));
        let err = socket_path(&home).unwrap_err();
        match err {
            ControlError::SocketPathTooLong { len, max, .. } => {
                assert_eq!(max, MAX_SOCKET_PATH_BYTES);
                assert_eq!(len, 117);
            }
            other => panic!("expected SocketPathTooLong, got {other:?}"),
        }
    }

    #[test]
    fn socket_path_accepts_a_home_that_lands_exactly_on_the_ceiling() {
        // 86 bytes of home + 17 = 103, the last accepted length.
        let home = PathBuf::from(format!("/{}", "h".repeat(85)));
        let p = socket_path(&home).unwrap();
        assert_eq!(
            p.as_os_str().as_encoded_bytes().len(),
            MAX_SOCKET_PATH_BYTES
        );
    }

    #[test]
    fn peer_is_authorized_only_on_exact_uid_equality() {
        assert!(peer_is_authorized(501, 501));
        assert!(!peer_is_authorized(502, 501));
        // Root is not special-cased in either direction.
        assert!(!peer_is_authorized(0, 501));
        assert!(!peer_is_authorized(501, 0));
        assert!(peer_is_authorized(0, 0));
    }
}
