// SPDX-License-Identifier: GPL-3.0-or-later
//! The control channel's error type (thiserror in lib crates — master plan §5).

use std::io;
use std::path::PathBuf;

/// Everything that can go wrong binding, connecting to, or talking over the
/// local control socket.
///
/// Deliberately **not** `#[non_exhaustive]`: the CLI maps every variant onto
/// one exit code with an exhaustive `match`, so adding a variant here must be
/// a compile error at the mapping site rather than a silent fall-through into
/// a wildcard arm.
///
/// Suggested exit-code mapping for the CLI (spec `D3`; the CLI owns the final
/// table, this is the intent each variant was written with):
///
/// | Variant | Exit | `error.code` |
/// |---|---|---|
/// | [`ControlError::SocketPathTooLong`] | 64 | `badRequest` |
/// | [`ControlError::InvalidServiceId`] | 64 | `badRequest` |
/// | [`ControlError::NotRunning`] | 69 | `supervisorUnavailable` |
/// | [`ControlError::NotASocket`] | 69 | `controlChannelUnavailable` |
/// | [`ControlError::Unreachable`] | 69 | `controlChannelUnavailable` |
/// | [`ControlError::UnsupportedPlatform`] | 69 | `controlChannelUnavailable` |
/// | [`ControlError::Io`] | 70 | `operationFailed` |
/// | [`ControlError::Protocol`] | 70 | `operationFailed` |
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// `<home>/run/control.sock` would exceed the `sun_path` ceiling
    /// ([`MAX_SOCKET_PATH_BYTES`](super::MAX_SOCKET_PATH_BYTES)). Raised at
    /// [`socket_path`](super::socket_path) time so the failure names the real
    /// cause instead of surfacing as an `EINVAL` from `bind(2)`.
    #[error("control socket path is {len} bytes, over the {max}-byte limit: {path}")]
    SocketPathTooLong {
        /// The path that was too long.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
        /// The ceiling that was exceeded.
        max: usize,
    },
    /// There is no socket at the expected path at all — the supervisor is not
    /// running. This is an *answer*, not a failure of the channel: `status`
    /// and `list` report it and exit 0 (spec D3).
    #[error("no control socket at {path}: the OpenVHost app does not appear to be running")]
    NotRunning {
        /// Where the socket was expected.
        path: PathBuf,
    },
    /// Something exists at the socket path but is not a socket — a symlink, a
    /// regular file, a directory. Never followed and never unlinked: on the
    /// server side [`bind`](super::bind) refuses to clear it, and on the
    /// client side [`request`](super::request) refuses to connect through it.
    #[error("{path} exists but is not a socket; refusing to use it")]
    NotASocket {
        /// The offending path.
        path: PathBuf,
    },
    /// The socket exists but the connection was refused or failed — typically
    /// a stale socket file left by a force-quit, or an app that is still
    /// starting up and has not begun accepting yet.
    #[error("the control socket at {path} is not accepting connections: {source}")]
    Unreachable {
        /// The socket that would not accept.
        path: PathBuf,
        /// The underlying `connect(2)` failure.
        source: io::Error,
    },
    /// An I/O failure while reading or writing the control channel.
    #[error("control channel I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The peer spoke something that is not this protocol: a truncated
    /// response, non-JSON, a `schemaVersion` we do not implement, or a
    /// response body that does not match its envelope.
    #[error("control protocol violation: {0}")]
    Protocol(String),
    /// A service id failed [`ServiceId::parse`](super::ServiceId::parse).
    #[error("invalid service id: {0}")]
    InvalidServiceId(String),
    /// The control channel is unix-only in v1. Windows is deferred
    /// project-wide; this mirrors
    /// [`InstanceLock::acquire`](crate::InstanceLock::acquire)'s explicit
    /// non-unix refusal rather than silently pretending success.
    #[error("the control channel is not implemented on this platform in v1 (macOS-first)")]
    UnsupportedPlatform,
}
