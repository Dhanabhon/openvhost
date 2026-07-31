// SPDX-License-Identifier: GPL-3.0-or-later
//! Client side of the control channel.
//!
//! Deliberately **synchronous** `std` (spec D6): the CLI makes exactly one
//! round trip, so an async runtime would buy nothing and cost startup time on
//! every `openvhost list`.

use std::path::Path;
use std::time::Duration;

use super::error::ControlError;
use super::protocol::{Request, Response};

/// How long the client waits for the server's answer.
///
/// Generous on purpose: with `wait` (the default) the server holds the
/// connection open for the whole transition, and the slowest one this
/// codebase already ships is MySQL's 15 s readiness deadline plus the 18 s
/// bulk-stop timeout. Bounded anyway, so a wedged server cannot hang a CI
/// script forever.
pub const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

/// Ceiling on the response the client will buffer.
///
/// Looser than [`MAX_REQUEST_BYTES`](super::MAX_REQUEST_BYTES) because the
/// two directions have different threat models: the request comes from an
/// untrusted peer, the response comes from our own supervisor. It is still
/// bounded rather than trusted — a `list` carrying stderr tails for a dozen
/// failed services is a few KiB, so this is pure headroom.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[cfg(not(unix))]
/// Always [`ControlError::UnsupportedPlatform`] off unix (v1 is macOS-first).
pub fn request(_home: &Path, _req: &Request) -> Result<Response, ControlError> {
    Err(ControlError::UnsupportedPlatform)
}

#[cfg(unix)]
/// Send one request to the app's control socket and read its answer.
///
/// Distinguishes the three "no answer" cases the CLI has to report
/// differently (spec D3):
///
/// - no socket at all → [`ControlError::NotRunning`] — the app is not
///   running. `status`/`list` report that and exit 0.
/// - something that is not a socket at the path → [`ControlError::NotASocket`],
///   never followed and never connected through.
/// - a socket that refuses the connection → [`ControlError::Unreachable`] —
///   typically a force-quit leftover, or an app still starting up.
///
/// The connect result is authoritative;
/// [`InstanceLock::probe`](crate::InstanceLock::probe) only exists to improve
/// the wording.
pub fn request(home: &Path, req: &Request) -> Result<Response, ControlError> {
    use std::io::Write as _;
    use std::os::unix::fs::FileTypeExt as _;

    let path = super::socket_path(home)?;
    // Pre-flight on the path itself: turns the common "app is not running"
    // case into a clear answer instead of a raw ENOENT, and refuses to
    // connect *through* a symlink someone dropped in place of the socket.
    match std::fs::symlink_metadata(&path) {
        Ok(md) if md.file_type().is_socket() => {}
        Ok(_) => return Err(ControlError::NotASocket { path }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ControlError::NotRunning { path });
        }
        Err(e) => return Err(e.into()),
    }
    let mut stream = match std::os::unix::net::UnixStream::connect(&path) {
        Ok(s) => s,
        // Raced with the app removing the socket on an orderly shutdown.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ControlError::NotRunning { path });
        }
        Err(e) => return Err(ControlError::Unreachable { path, source: e }),
    };
    stream.set_write_timeout(Some(CLIENT_RESPONSE_TIMEOUT))?;
    stream.set_read_timeout(Some(CLIENT_RESPONSE_TIMEOUT))?;
    let mut line = super::protocol::encode_request(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    // Half-close: the newline already frames the request, but an explicit EOF
    // means a server reading to EOF works too, and it makes a client that
    // dies mid-request unambiguous.
    stream.shutdown(std::net::Shutdown::Write)?;
    let raw = read_line_capped(&mut stream, MAX_RESPONSE_BYTES)?;
    super::protocol::decode_response(&raw)
}

/// Read up to one newline (or EOF), refusing to buffer more than `max`.
///
/// The synchronous twin of the server's reader; kept separate rather than
/// shared because this one is deliberately not async.
#[cfg(unix)]
fn read_line_capped<R: std::io::Read>(reader: &mut R, max: usize) -> Result<Vec<u8>, ControlError> {
    let mut line: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            return Ok(line);
        }
        let read = &chunk[..n];
        match read.iter().position(|b| *b == b'\n') {
            Some(pos) => {
                line.extend_from_slice(&read[..pos]);
                if line.len() > max {
                    return Err(ControlError::Protocol(format!(
                        "the response exceeded {max} bytes"
                    )));
                }
                return Ok(line);
            }
            None => {
                line.extend_from_slice(read);
                if line.len() > max {
                    return Err(ControlError::Protocol(format!(
                        "the response exceeded {max} bytes"
                    )));
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn read_line_capped_stops_at_the_newline() {
        let mut r: &[u8] = b"{\"ok\":true}\ntrailing";
        assert_eq!(
            read_line_capped(&mut r, MAX_RESPONSE_BYTES).unwrap(),
            b"{\"ok\":true}".to_vec()
        );
    }

    #[test]
    fn read_line_capped_accepts_a_response_terminated_by_eof() {
        let mut r: &[u8] = b"{\"ok\":true}";
        assert_eq!(
            read_line_capped(&mut r, MAX_RESPONSE_BYTES).unwrap(),
            b"{\"ok\":true}".to_vec()
        );
    }

    #[test]
    fn read_line_capped_refuses_an_oversized_response() {
        let payload = vec![b'x'; 65];
        let mut r: &[u8] = &payload;
        match read_line_capped(&mut r, 64) {
            Err(ControlError::Protocol(m)) => assert!(m.contains("64"), "{m}"),
            other => panic!("expected a Protocol error, got {other:?}"),
        }
    }

    #[test]
    fn request_reports_not_running_when_there_is_no_socket() {
        let home = tempfile::tempdir().unwrap();
        match request(home.path(), &Request::List) {
            Err(ControlError::NotRunning { .. }) => {}
            other => panic!("expected NotRunning, got {other:?}"),
        }
    }

    #[test]
    fn request_refuses_a_socket_path_that_is_a_regular_file() {
        let home = tempfile::tempdir().unwrap();
        let path = super::super::socket_path(home.path()).unwrap();
        std::fs::create_dir_all(home.path().join("run")).unwrap();
        std::fs::write(&path, b"not a socket").unwrap();
        match request(home.path(), &Request::List) {
            Err(ControlError::NotASocket { path: p }) => assert_eq!(p, path),
            other => panic!("expected NotASocket, got {other:?}"),
        }
    }
}
