// SPDX-License-Identifier: GPL-3.0-or-later
//! Server side of the control channel: bind, authorize, bound the ingress,
//! dispatch to a [`ControlHandler`], answer, close.

use std::sync::Arc;

use super::error::ControlError;
use super::protocol::{Request, Response};

/// The policy seam (spec D6).
///
/// Transport, parsing and authorization are this crate's job; deciding what a
/// verb *does* is the desktop app's. `openvhost-proc` never learns about the
/// bulk lock, `quit::stop_all`, or Tauri managed state — it hands a parsed,
/// authorized [`Request`] to whoever implements this and writes back whatever
/// [`Response`] comes out.
///
/// Implementations must be cheap to clone into a per-connection task
/// (`Arc<dyn ControlHandler>`) and must not panic: a panicking handler takes
/// down only its own connection task, but the caller gets no answer at all.
///
/// ```ignore
/// #[openvhost_proc::control::async_trait]
/// impl ControlHandler for DesktopHandler {
///     async fn execute(&self, req: Request) -> Response {
///         match req { /* exhaustive — no wildcard arm */ }
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait ControlHandler: Send + Sync {
    /// Execute one already-authorized request. Failures are values
    /// ([`Response::Error`]), not `Err` — every outcome is something the CLI
    /// prints and maps to an exit code.
    async fn execute(&self, req: Request) -> Response;
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    /// A bound control socket. Unconstructible off unix — [`bind`] always
    /// refuses there, so [`serve`] discharges it with an empty `match`
    /// rather than an `unreachable!`.
    pub enum ControlListener {}

    /// Always [`ControlError::UnsupportedPlatform`] off unix (v1 is
    /// macOS-first), mirroring
    /// [`InstanceLock::acquire`](crate::InstanceLock::acquire)'s explicit
    /// refusal instead of silently pretending success.
    pub fn bind(_home: &std::path::Path) -> Result<ControlListener, ControlError> {
        Err(ControlError::UnsupportedPlatform)
    }

    /// Unreachable off unix: [`ControlListener`] has no variants, so there is
    /// no way to call this with a value.
    pub async fn serve<S>(
        listener: ControlListener,
        _handler: Arc<dyn ControlHandler>,
        _shutdown: S,
    ) where
        S: std::future::Future<Output = ()> + Send,
    {
        match listener {}
    }
}

#[cfg(unix)]
mod imp {
    use std::future::Future;
    use std::io;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Semaphore;
    use tokio::time::timeout;

    use super::*;
    use crate::control::protocol::{ErrorCode, UNKNOWN_COMMAND, decode_request, envelope_json};
    use crate::control::{MAX_REQUEST_BYTES, READ_TIMEOUT, peer_is_authorized, socket_path};

    /// The response write shares the ingress deadline: a peer that never
    /// reads its answer must not pin a task any longer than one that never
    /// sends a request.
    const WRITE_TIMEOUT: Duration = READ_TIMEOUT;

    /// Connections served at once. Each is already bounded in size and time,
    /// so this is not what stops abuse — it stops an unbounded number of
    /// spawned tasks, and pushes the excess back into the listen backlog
    /// where the kernel bounds it for us.
    const MAX_CONCURRENT_CONNECTIONS: usize = 32;

    /// Pause after a failed `accept` so a persistent error (`EMFILE`) degrades
    /// into a slow log rather than a hot spin.
    const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

    /// A bound, listening control socket, not yet being served.
    ///
    /// Deliberately a **`std`** listener rather than a `tokio` one: [`bind`]
    /// is called from app startup, which is not inside a tokio runtime
    /// context, and `tokio::net::UnixListener::bind` panics there ("there is
    /// no reactor running"). The conversion happens inside [`serve`], which
    /// is by definition running on the runtime.
    #[derive(Debug)]
    pub struct ControlListener {
        listener: std::os::unix::net::UnixListener,
        path: PathBuf,
        owner_uid: u32,
        dev: u64,
        ino: u64,
    }

    impl ControlListener {
        /// Where the socket is bound.
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// The uid every peer is checked against.
        ///
        /// Read from the socket file we just created, whose owner *is* our
        /// effective uid — which is how this crate learns its own uid with
        /// **no `unsafe`** (there is no safe `getuid` in `std`, and this is a
        /// security-sensitive path where a hand-rolled `libc` call is exactly
        /// what the design forbids). It also fails closed: a socket somehow
        /// owned by someone else authorizes nobody, including us.
        pub fn owner_uid(&self) -> u32 {
            self.owner_uid
        }
    }

    /// Bind `<home>/run/control.sock`: create and tighten the run dir, clear a
    /// stale socket, bind, `chmod 0600`.
    ///
    /// **Stale-socket rule (spec D1).** The path is unlinked *only* when
    /// `symlink_metadata` proves it is a socket. A symlink, a regular file, a
    /// directory or a fifo is refused with [`ControlError::NotASocket`] and
    /// never followed — so this cannot be turned into an arbitrary-unlink
    /// primitive. Holding the exclusive instance `flock` (the caller binds
    /// inside the `Ok(Some(lock))` arm) is what makes that unlink provably
    /// safe rather than a racy connect-probe.
    ///
    /// The window between `bind(2)` and the `chmod` leaves the socket at the
    /// umask-derived mode for an instant; it is not reachable by another user
    /// because the enclosing directory is `0700`, which this function
    /// enforces first.
    ///
    /// Safe to call outside a tokio runtime — see [`ControlListener`].
    pub fn bind(home: &Path) -> Result<ControlListener, ControlError> {
        let path = socket_path(home)?;
        let run_dir = match path.parent() {
            Some(d) => d.to_path_buf(),
            None => {
                return Err(ControlError::Io(io::Error::other(format!(
                    "control socket path has no parent directory: {}",
                    path.display()
                ))));
            }
        };
        // Same 0700 posture, set the same way, as `InstanceLock::acquire`:
        // `set_permissions` pins the exact bits regardless of the ambient
        // umask, which `DirBuilder::mode` would not.
        std::fs::create_dir_all(&run_dir)?;
        std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700))?;
        match std::fs::symlink_metadata(&path) {
            Ok(md) if md.file_type().is_socket() => std::fs::remove_file(&path)?,
            Ok(_) => return Err(ControlError::NotASocket { path }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = std::os::unix::net::UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let md = std::fs::symlink_metadata(&path)?;
        Ok(ControlListener {
            listener,
            path,
            owner_uid: md.uid(),
            dev: md.dev(),
            ino: md.ino(),
        })
    }

    /// Accept and serve control connections until `shutdown` resolves, then
    /// unlink the socket.
    ///
    /// Pass [`std::future::pending()`] for "serve for the process lifetime";
    /// pass a real future if the socket should be removed on an orderly
    /// shutdown. Each connection is handled in its own task, so a `start`
    /// that waits 15 s for readiness does not stall the next caller.
    ///
    /// ```ignore
    /// let listener = control::bind(&home)?;                 // outside the runtime is fine
    /// let handler: Arc<dyn ControlHandler> = Arc::new(DesktopHandler::new(sup));
    /// tauri::async_runtime::spawn(control::serve(listener, handler, std::future::pending()));
    /// ```
    pub async fn serve<S>(listener: ControlListener, handler: Arc<dyn ControlHandler>, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        let ControlListener {
            listener: std_listener,
            path,
            owner_uid,
            dev,
            ino,
        } = listener;
        let tokio_listener = match tokio::net::UnixListener::from_std(std_listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, path = %path.display(), "control: could not register the socket with the runtime");
                remove_socket_if_ours(&path, dev, ino);
                return;
            }
        };
        tracing::info!(path = %path.display(), uid = owner_uid, "control: serving");
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        tokio::pin!(shutdown);
        loop {
            let permit = tokio::select! {
                biased;
                () = &mut shutdown => break,
                p = Arc::clone(&permits).acquire_owned() => match p {
                    Ok(p) => p,
                    // Only reachable if the semaphore were closed, which
                    // nothing here does; stopping is the safe reading.
                    Err(_) => break,
                },
            };
            let accepted = tokio::select! {
                biased;
                () = &mut shutdown => break,
                a = tokio_listener.accept() => a,
            };
            match accepted {
                Ok((stream, _addr)) => {
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        let _permit = permit;
                        handle_connection(stream, handler, owner_uid).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "control: accept failed");
                    drop(permit);
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            }
        }
        remove_socket_if_ours(&path, dev, ino);
    }

    /// Unlink the socket on shutdown, but only if the path still holds the
    /// very inode we bound. Same posture as the stale-socket rule in
    /// [`bind`]: never unlink something that is not ours.
    fn remove_socket_if_ours(path: &Path, dev: u64, ino: u64) {
        match std::fs::symlink_metadata(path) {
            Ok(md) if md.file_type().is_socket() && md.dev() == dev && md.ino() == ino => {
                if let Err(e) = std::fs::remove_file(path) {
                    tracing::warn!(error = %e, path = %path.display(), "control: could not remove the socket");
                }
            }
            Ok(_) => tracing::warn!(
                path = %path.display(),
                "control: leaving the socket path alone — it is no longer the inode we bound"
            ),
            Err(_) => {}
        }
    }

    /// One request/response exchange. Authorization happens **before** a
    /// single byte of the request is read.
    async fn handle_connection(
        mut stream: tokio::net::UnixStream,
        handler: Arc<dyn ControlHandler>,
        our_uid: u32,
    ) {
        let peer_uid = match stream.peer_cred() {
            Ok(cred) => cred.uid(),
            Err(e) => {
                // Without credentials there is no authorization decision to
                // make, so fail closed and say nothing to the peer.
                tracing::warn!(error = %e, "control: could not read peer credentials; dropping");
                return;
            }
        };
        if !peer_is_authorized(peer_uid, our_uid) {
            tracing::warn!(
                peer_uid,
                our_uid,
                "control: refused a peer owned by another uid"
            );
            respond(
                &mut stream,
                UNKNOWN_COMMAND,
                &Response::error(
                    ErrorCode::Unauthorized,
                    "the control socket only accepts connections from the user running OpenVHost",
                ),
            )
            .await;
            return;
        }
        let raw = match timeout(
            READ_TIMEOUT,
            read_line_capped(&mut stream, MAX_REQUEST_BYTES),
        )
        .await
        {
            Ok(Ok(ReadOutcome::Line(bytes))) => bytes,
            Ok(Ok(ReadOutcome::TooLarge)) => {
                respond(
                    &mut stream,
                    UNKNOWN_COMMAND,
                    &Response::error(
                        ErrorCode::BadRequest,
                        format!("the request exceeded {MAX_REQUEST_BYTES} bytes"),
                    ),
                )
                .await;
                return;
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "control: failed reading the request");
                return;
            }
            Err(_elapsed) => {
                respond(
                    &mut stream,
                    UNKNOWN_COMMAND,
                    &Response::error(
                        ErrorCode::BadRequest,
                        format!(
                            "no complete request line arrived within {}s",
                            READ_TIMEOUT.as_secs()
                        ),
                    ),
                )
                .await;
                return;
            }
        };
        let (command, response) = match decode_request(&raw) {
            Ok(req) => {
                let command = req.command_name().to_owned();
                (command, handler.execute(req).await)
            }
            Err(rejection) => (
                rejection.command,
                Response::Error {
                    code: rejection.code,
                    message: rejection.message,
                },
            ),
        };
        respond(&mut stream, &command, &response).await;
    }

    /// Write one envelope line and close the write half, under
    /// [`WRITE_TIMEOUT`].
    async fn respond(stream: &mut tokio::net::UnixStream, command: &str, response: &Response) {
        let mut line = envelope_json(command, response).to_string();
        line.push('\n');
        let write = async {
            stream.write_all(line.as_bytes()).await?;
            stream.flush().await?;
            stream.shutdown().await
        };
        match timeout(WRITE_TIMEOUT, write).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "control: failed writing the response"),
            Err(_elapsed) => tracing::warn!("control: the peer did not read its response in time"),
        }
    }

    /// What a bounded read produced.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum ReadOutcome {
        /// A complete line (newline stripped), or everything received before
        /// EOF — being liberal about a missing trailing newline costs nothing
        /// and the decoder rejects anything that is not a request anyway.
        Line(Vec<u8>),
        /// The peer sent more than `max` bytes without a newline.
        TooLarge,
    }

    /// Read up to one newline, refusing to buffer more than `max` bytes.
    ///
    /// Generic over [`tokio::io::AsyncRead`] so the cap can be tested against
    /// a hand-fed reader with no socket involved. The deadline is the
    /// caller's ([`READ_TIMEOUT`]): this function has no notion of time, so a
    /// reader that never yields simply never returns — which is exactly what
    /// makes it composable with `timeout`.
    pub(crate) async fn read_line_capped<R>(reader: &mut R, max: usize) -> io::Result<ReadOutcome>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut line: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = reader.read(&mut chunk).await?;
            if n == 0 {
                return Ok(ReadOutcome::Line(line));
            }
            let read = &chunk[..n];
            match read.iter().position(|b| *b == b'\n') {
                Some(pos) => {
                    line.extend_from_slice(&read[..pos]);
                    if line.len() > max {
                        return Ok(ReadOutcome::TooLarge);
                    }
                    // Anything after the newline is discarded: one request
                    // per connection, by design.
                    return Ok(ReadOutcome::Line(line));
                }
                None => {
                    line.extend_from_slice(read);
                    if line.len() > max {
                        return Ok(ReadOutcome::TooLarge);
                    }
                }
            }
        }
    }
}

pub use imp::{ControlListener, bind, serve};

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::imp::{ReadOutcome, read_line_capped};
    use super::*;
    use crate::control::{MAX_REQUEST_BYTES, socket_path};

    #[tokio::test]
    async fn read_line_capped_returns_the_line_without_its_newline() {
        let mut r: &[u8] = b"{\"command\":\"list\"}\n";
        let out = read_line_capped(&mut r, MAX_REQUEST_BYTES).await.unwrap();
        assert_eq!(out, ReadOutcome::Line(b"{\"command\":\"list\"}".to_vec()));
    }

    #[tokio::test]
    async fn read_line_capped_accepts_a_final_line_terminated_by_eof() {
        let mut r: &[u8] = b"{\"command\":\"list\"}";
        let out = read_line_capped(&mut r, MAX_REQUEST_BYTES).await.unwrap();
        assert_eq!(out, ReadOutcome::Line(b"{\"command\":\"list\"}".to_vec()));
    }

    #[tokio::test]
    async fn read_line_capped_discards_anything_after_the_first_newline() {
        let mut r: &[u8] = b"one\ntwo\n";
        let out = read_line_capped(&mut r, MAX_REQUEST_BYTES).await.unwrap();
        assert_eq!(out, ReadOutcome::Line(b"one".to_vec()));
    }

    #[tokio::test]
    async fn read_line_capped_refuses_more_than_max_without_a_newline() {
        let payload = vec![b'x'; MAX_REQUEST_BYTES + 1];
        let mut r: &[u8] = &payload;
        let out = read_line_capped(&mut r, MAX_REQUEST_BYTES).await.unwrap();
        assert_eq!(out, ReadOutcome::TooLarge);
    }

    #[tokio::test]
    async fn read_line_capped_accepts_exactly_max_bytes() {
        let mut payload = vec![b'x'; MAX_REQUEST_BYTES];
        payload.push(b'\n');
        let mut r: &[u8] = &payload;
        let out = read_line_capped(&mut r, MAX_REQUEST_BYTES).await.unwrap();
        assert_eq!(out, ReadOutcome::Line(vec![b'x'; MAX_REQUEST_BYTES]));
    }

    /// The cap must apply to a *stream*, not just to one buffer: a peer that
    /// drips 4 KiB at a time must still be cut off.
    #[tokio::test]
    async fn read_line_capped_refuses_an_oversized_body_delivered_in_chunks() {
        let (client, server) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            let mut client = client;
            let chunk = vec![b'x'; 8 * 1024];
            for _ in 0..16 {
                if client.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        });
        let mut server = server;
        let out = read_line_capped(&mut server, MAX_REQUEST_BYTES)
            .await
            .unwrap();
        assert_eq!(out, ReadOutcome::TooLarge);
        drop(server);
        let _ = writer.await;
    }

    /// A reader that never yields data must never return on its own — that is
    /// what makes the caller's `timeout` the only thing bounding it.
    #[tokio::test]
    async fn read_line_capped_never_completes_for_a_silent_peer() {
        let (client, server) = tokio::io::duplex(64);
        let mut server = server;
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_millis(120),
            read_line_capped(&mut server, MAX_REQUEST_BYTES),
        )
        .await;
        assert!(elapsed.is_err(), "must still be waiting, not have returned");
        drop(client);
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The seam Task 2 depends on: the app binds from its `setup` closure,
    /// which is **not** inside a tokio runtime. `tokio::net::UnixListener::bind`
    /// panics there ("there is no reactor running"), which is why
    /// [`ControlListener`] holds a `std` listener and the conversion is
    /// deferred to [`serve`]. This is a plain `#[test]`, deliberately not a
    /// `#[tokio::test]` — converting it would silently delete the guarantee.
    #[test]
    fn bind_works_outside_a_tokio_runtime() {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "this test only means something with no runtime in scope"
        );
        let home = tempdir();
        let listener = bind(home.path()).unwrap();
        assert!(listener.path().exists());
    }

    #[test]
    fn bind_creates_a_0600_socket_in_a_0700_run_dir() {
        let home = tempdir();
        let listener = bind(home.path()).unwrap();
        let md = std::fs::symlink_metadata(listener.path()).unwrap();
        assert_eq!(md.permissions().mode() & 0o777, 0o600);
        let run = std::fs::metadata(home.path().join("run")).unwrap();
        assert_eq!(run.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn bind_unlinks_a_stale_socket_and_succeeds() {
        let home = tempdir();
        let first = bind(home.path()).unwrap();
        let path = first.path().to_path_buf();
        // Dropping a listener does NOT unlink the path — this is exactly the
        // force-quit leftover the rule exists for.
        drop(first);
        assert!(path.exists(), "the stale socket must still be on disk");
        let second = bind(home.path()).unwrap();
        assert_eq!(second.path(), path);
    }

    #[test]
    fn bind_refuses_a_symlink_at_the_socket_path() {
        let home = tempdir();
        let run = home.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let decoy = home.path().join("decoy");
        std::fs::write(&decoy, b"do not unlink me").unwrap();
        let path = socket_path(home.path()).unwrap();
        std::os::unix::fs::symlink(&decoy, &path).unwrap();
        match bind(home.path()) {
            Err(ControlError::NotASocket { path: p }) => assert_eq!(p, path),
            other => panic!("expected NotASocket, got {other:?}"),
        }
        assert!(decoy.exists(), "the symlink target must not be unlinked");
        assert!(
            std::fs::symlink_metadata(&path).is_ok(),
            "the symlink itself must not be unlinked"
        );
    }

    #[test]
    fn bind_refuses_a_regular_file_at_the_socket_path() {
        let home = tempdir();
        let run = home.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let path = socket_path(home.path()).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"not a socket").unwrap();
        drop(f);
        match bind(home.path()) {
            Err(ControlError::NotASocket { path: p }) => assert_eq!(p, path),
            other => panic!("expected NotASocket, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");
    }

    #[test]
    fn bind_refuses_a_directory_at_the_socket_path() {
        let home = tempdir();
        let path = socket_path(home.path()).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        match bind(home.path()) {
            Err(ControlError::NotASocket { path: p }) => assert_eq!(p, path),
            other => panic!("expected NotASocket, got {other:?}"),
        }
    }

    #[test]
    fn bind_rejects_an_over_long_home_before_touching_the_filesystem() {
        let home = tempdir();
        // Pad the home out past the sun_path ceiling with a real directory,
        // so the refusal is provably the length check and not an ENOENT.
        let deep = home.path().join("x".repeat(90));
        match bind(&deep) {
            Err(ControlError::SocketPathTooLong { .. }) => {}
            other => panic!("expected SocketPathTooLong, got {other:?}"),
        }
        assert!(
            !Path::new(&deep).exists(),
            "a refused bind must not have created anything"
        );
    }

    #[test]
    fn owner_uid_is_the_socket_owner() {
        let home = tempdir();
        let listener = bind(home.path()).unwrap();
        let md = std::fs::symlink_metadata(listener.path()).unwrap();
        assert_eq!(
            listener.owner_uid(),
            std::os::unix::fs::MetadataExt::uid(&md)
        );
    }
}
