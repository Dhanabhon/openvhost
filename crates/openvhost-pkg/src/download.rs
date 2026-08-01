// SPDX-License-Identifier: GPL-3.0-or-later
//! Streaming download + SHA-256 verification. The hash is computed over the
//! exact wire bytes (transport auto-decompression is disabled at the reqwest
//! feature level and we send Accept-Encoding: identity — S3). Verification
//! happens BEFORE the handle is returned to the extractor, on the SAME open
//! file (S8); nothing is ever re-opened by path.
//!
//! Liveness is bounded by SILENCE, not by total duration: see
//! [`STALL_TIMEOUT`] for why a fixed wall clock on the whole request is a
//! minimum-bandwidth requirement in disguise.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::error::PkgError;
use crate::request::{Progress, validate_https_url};

const SIZE_CAP: u64 = 1024 * 1024 * 1024; // 1 GiB (S4)
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Transport-level backstop, applied per read operation and reset after each
/// successful one (reqwest 0.12 `ClientBuilder::read_timeout`). It covers the
/// phases the streaming loop below cannot see — connect, TLS, response head —
/// and is deliberately kept STRICTLY LARGER than [`STALL_TIMEOUT`] so that
/// during the body phase OUR stall check fires first. That ordering is
/// load-bearing: reqwest's timeout surfaces as an opaque "operation timed
/// out" mapped to [`PkgError::Network`], which is exactly the
/// indistinguishable-from-a-dead-network message this module now avoids.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle timeout on the body: how long the transfer may make NO progress
/// before it is declared stalled. Resets on every chunk, so it bounds
/// silence, never total duration.
///
/// This replaced a fixed 900 s wall clock on the whole request
/// (`ClientBuilder::timeout`), which reqwest applies to the response body as
/// well as the head. A constant wall clock on a whole transfer is not a
/// timeout, it is a MINIMUM BANDWIDTH requirement wearing a network error's
/// clothes: 900 s against the real 167,977,240-byte MySQL 8.4 tarball is a
/// hard floor of ~1.5 Mbit/s sustained, and a user below it saw something
/// indistinguishable from "the network failed". Worse, the floor silently
/// rises with every payload we add. Bounding SILENCE instead is independent
/// of payload size, and total bytes stay bounded by [`SIZE_CAP`].
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Download limits. Production values are [`Limits::PRODUCTION`]; tests
/// override one field at a time (`Limits { cap: 1024, ..PRODUCTION }`) so a
/// test never silently exercises a different regime than it means to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Hard byte ceiling on the response (S4).
    pub(crate) cap: u64,
    /// Maximum time the body may make no progress before it is a stall.
    pub(crate) stall: Duration,
}

impl Limits {
    pub(crate) const PRODUCTION: Limits = Limits {
        cap: SIZE_CAP,
        stall: STALL_TIMEOUT,
    };
}

/// Download `url`, verify its SHA-256 against `sha256`, and return the open,
/// rewound file handle (S8). Pins the production limits; see
/// [`download_with`] for the limit-parameterized core.
pub(crate) async fn download_and_verify(
    url: &url::Url,
    sha256: &str,
    staging_dir: &Path,
    progress: impl FnMut(Progress),
) -> Result<fs::File, PkgError> {
    download_with(url, sha256, staging_dir, Limits::PRODUCTION, progress).await
}

/// Limit-parameterized core (the wrapper pins production values; tests pass
/// a tiny cap or a short stall window). In production `url` is already
/// `https`; in debug builds a loopback `http` URL is permitted so hermetic
/// tests need no TLS (S2 — compiled out of release).
pub(crate) async fn download_with(
    url: &url::Url,
    sha256: &str,
    staging_dir: &Path,
    limits: Limits,
    mut progress: impl FnMut(Progress),
) -> Result<fs::File, PkgError> {
    let cap = limits.cap;
    check_scheme_result(url)?;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                return attempt.error(io_msg("too many redirects"));
            }
            match check_scheme_result(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(_) => attempt.error(io_msg("redirect to disallowed url")),
            }
        }))
        .build()
        .map_err(|e| PkgError::Network(e.to_string()))?;

    let resp = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| PkgError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| PkgError::Network(e.to_string()))?;

    tracing::info!(url = %url, final_url = %resp.url(), "download start");

    let declared = resp.content_length();
    if let Some(len) = declared
        && len > cap
    {
        return Err(PkgError::TooLarge { cap });
    }
    progress(Progress::Started { total: declared });

    let archive_path = staging_dir.join("archive");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&archive_path)
        .map_err(|e| PkgError::io("create_new", &archive_path, e))?;

    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    let started = Instant::now();
    loop {
        // The stall window applies to EACH wait for the next chunk and is
        // reset by every chunk that arrives, so a slow-but-progressing
        // transfer runs as long as it needs to. `next()` is only ever
        // cancelled on the path that abandons the whole download, so its
        // cancel-safety is not relied on.
        let chunk = match tokio::time::timeout(limits.stall, stream.next()).await {
            Err(_elapsed) => {
                let elapsed_secs = started.elapsed().as_secs_f64();
                drop(file);
                let _ = fs::remove_file(&archive_path);
                tracing::warn!(
                    received = total,
                    expected = ?declared,
                    elapsed_secs,
                    stall_secs = limits.stall.as_secs_f64(),
                    "download stalled"
                );
                return Err(PkgError::DownloadStalled {
                    received: total,
                    expected: declared,
                    elapsed_secs,
                    stall_secs: limits.stall.as_secs_f64(),
                });
            }
            Ok(None) => break,
            Ok(Some(chunk)) => chunk.map_err(|e| PkgError::Network(e.to_string()))?,
        };
        total = total.saturating_add(chunk.len() as u64);
        if total > cap {
            drop(file);
            let _ = fs::remove_file(&archive_path);
            return Err(PkgError::TooLarge { cap });
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| PkgError::io("write", &archive_path, e))?;
        progress(Progress::Downloaded { bytes: total });
    }

    // S4's second clause: if a Content-Length was declared, the actual byte
    // count must match it exactly, not just stay under the cap. In practice
    // hyper's own HTTP/1 framing already errors the stream (surfaced above
    // via the `chunk?` map_err as `PkgError::Network`) on a premature close
    // before the declared length is reached, and it never yields bytes past
    // the declared boundary — so this is a defense-in-depth belt-and-suspenders
    // check (also caught by the SHA-256 verification below regardless), kept
    // explicit so the invariant is visible in code, not just an argument
    // about transport-library internals.
    if let Some(len) = declared
        && total != len
    {
        drop(file);
        let _ = fs::remove_file(&archive_path);
        return Err(PkgError::Network(format!(
            "declared content-length {len} but received {total} bytes"
        )));
    }

    file.sync_all()
        .map_err(|e| PkgError::io("sync", &archive_path, e))?;

    let actual = hex::encode(hasher.finalize());
    if actual != sha256 {
        tracing::warn!(expected = %sha256, actual = %actual, "sha256 mismatch");
        drop(file);
        let _ = fs::remove_file(&archive_path);
        return Err(PkgError::HashMismatch {
            expected: sha256.to_string(),
            actual,
        });
    }
    tracing::info!(bytes = total, "download verified");
    progress(Progress::Verified);
    file.seek(SeekFrom::Start(0))
        .map_err(|e| PkgError::io("seek", &archive_path, e))?;
    Ok(file)
}

fn check_scheme_result(url: &url::Url) -> Result<(), PkgError> {
    // Shared with `request.rs::validate_https_url` (S1/S2): the initial
    // request URL (`InstallRequest::new`) and every redirect hop here go
    // through the exact same check, including its debug-only loopback
    // carve-out — one validator, not two copies that could silently drift
    // apart.
    validate_https_url(url)
}

fn io_msg(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(std::io::Error::other(msg.to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Minimal one-shot HTTP/1.1 server. `handler` receives the request line
    /// and returns raw response bytes. Returns the bound URL. Loopback + http
    /// is permitted only under debug_assertions (S2).
    ///
    /// A write timeout is set on the accepted socket so this thread can never
    /// block forever: `enforces_size_cap` deliberately sends a body far
    /// larger than the client is willing to read (the client aborts as soon
    /// as it sees the over-cap `Content-Length`, before consuming any body
    /// bytes), and a naive blocking `write_all` of the full body can stall
    /// until the OS send buffer drains — which, on a client that already
    /// returned without reading, may not happen promptly. Confirmed
    /// empirically: without this timeout, this test hung indefinitely under
    /// load (reproduced via a direct, non-`cargo test` run of the compiled
    /// test binary — `ps`/`lsof` showed the connection ESTABLISHED but idle
    /// at 0% CPU past 60s). The timeout bounds the server thread's own
    /// worst case; `download_capped`'s behavior is unaffected either way
    /// (the client-side abort-on-declared-length-check already happened).
    fn serve(body: Vec<u8>, extra_headers: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/archive", addr.port());
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}\r\n",
                    body.len(),
                    extra_headers
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.write_all(&body);
            }
        });
        (url, handle)
    }

    /// Like `serve`, but with no `Content-Length` header at all: the
    /// response is close-delimited (`Connection: close`; the body ends when
    /// the connection closes), which is the only way to make
    /// `download_capped`'s `declared` come back `None` so its running
    /// per-chunk counter — not the early declared-length check — is what
    /// has to catch an over-cap body.
    fn serve_no_content_length(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/archive", addr.port());
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let _ = sock.write_all(&body);
            }
        });
        (url, handle)
    }

    /// One-shot server that writes the response head, then the body in
    /// `chunks` pieces with `gap` of silence AFTER each piece. If
    /// `then_hang` it declares a longer body than it sends and simply stops
    /// writing, holding the connection open — the wire picture of a transfer
    /// that dies mid-flight without an RST or a FIN.
    ///
    /// The thread does not sleep out `hold` blindly: it blocks in `read`
    /// with that as a timeout, so it returns the instant the client hangs
    /// up (which is what the stall tests make it do) instead of pinning a
    /// thread for the whole hold.
    fn serve_dribble(
        body: Vec<u8>,
        chunks: usize,
        gap: std::time::Duration,
        then_hang: bool,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/archive", addr.port());
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                // Declared length always names the FULL body, so a hang is a
                // genuine mid-transfer stall rather than a completed short
                // response.
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes());
                let send = if then_hang { 1 } else { chunks };
                let size = body.len().div_ceil(chunks.max(1));
                for piece in body.chunks(size).take(send) {
                    if sock.write_all(piece).is_err() {
                        return;
                    }
                    let _ = sock.flush();
                    std::thread::sleep(gap);
                }
                if then_hang {
                    let _ = sock.set_read_timeout(Some(std::time::Duration::from_secs(10)));
                    let mut sink = [0u8; 64];
                    let _ = sock.read(&mut sink); // returns 0 the moment the client drops
                }
            }
        });
        (url, handle)
    }

    /// One-shot server that replies with a 302 redirect to `location`. Used
    /// to prove the custom redirect policy rejects a hostile hop (S1)
    /// BEFORE ever connecting to it: the redirect target is evaluated as a
    /// plain `Url` value inside the policy closure, with no network access
    /// to it, so `location` never needs to be an actually-reachable host.
    fn serve_redirect(location: &'static str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/start", addr.port());
        let handle = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let _ = sock.set_write_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });
        (url, handle)
    }

    /// Join a server thread from async test code WITHOUT blocking the
    /// current-thread test runtime's only OS thread. Needed specifically
    /// for the "abandoned body" tests (`enforces_size_cap` and its
    /// no-Content-Length sibling): `download_capped` returns as soon as it
    /// detects the over-cap condition, without ever reading the rest of the
    /// (multi-MiB) body, and a plain synchronous `h.join()` right after
    /// that — with no intervening `.await` — parks the runtime's only
    /// thread, so it can never poll reqwest's connection-driver task again
    /// to notice the abandoned response and close the connection. The raw
    /// test-server thread is then left writing into a socket nobody is
    /// draining, and can block on `write_all` until its own write timeout
    /// fires. `spawn_blocking` + `.await` moves the actual blocking join
    /// onto a separate tokio blocking-pool thread, which lets the runtime
    /// keep servicing other tasks (including that connection cleanup)
    /// while we wait — confirmed empirically: this resolves promptly
    /// rather than waiting out the full write timeout every time.
    async fn join_server(h: std::thread::JoinHandle<()>) {
        tokio::task::spawn_blocking(move || h.join())
            .await
            .unwrap()
            .unwrap();
    }

    fn sha_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let body = b"hello openvhost".to_vec();
        let sha = sha_hex(&body);
        let (url, h) = serve(body.clone(), "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut f = download_and_verify(&u, &sha, dir.path(), |_| {})
            .await
            .unwrap();
        let mut got = Vec::new();
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        h.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_hash_mismatch() {
        let (url, h) = serve(b"tampered".to_vec(), "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_and_verify(&u, &"0".repeat(64), dir.path(), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, PkgError::HashMismatch { .. }));
        // staging archive must be gone on failure
        assert!(!dir.path().join("archive").exists());
        h.join().unwrap();
    }

    #[tokio::test]
    async fn enforces_size_cap() {
        // 2 MiB body against a tiny cap via the test-only constructor. The
        // client aborts as soon as it sees the over-cap Content-Length,
        // before reading any body bytes — see `join_server` for why the
        // thread join is offloaded rather than a direct blocking `h.join()`.
        let body = vec![0u8; 2 * 1024 * 1024];
        let (url, h) = serve(body, "");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_with(
            &u,
            &"0".repeat(64),
            dir.path(),
            Limits {
                cap: 1024,
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PkgError::TooLarge { .. }));
        join_server(h).await;
    }

    #[tokio::test]
    async fn enforces_size_cap_via_running_counter_without_content_length() {
        // S4's PRIMARY branch: the running byte counter must abort the
        // stream mid-flight, independent of Content-Length. No
        // Content-Length header is sent at all here (close-delimited
        // HTTP/1.1 framing — `Connection: close`, body ends when the
        // connection closes), so `declared` is `None` and the early
        // declared-length check in `download_capped` never runs; only the
        // `total > cap` check inside the streaming loop can catch this.
        let body = vec![0u8; 2 * 1024 * 1024];
        let (url, h) = serve_no_content_length(body);
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_with(
            &u,
            &"0".repeat(64),
            dir.path(),
            Limits {
                cap: 1024,
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PkgError::TooLarge { .. }));
        // staging archive must be gone on failure, same as the other abort paths
        assert!(!dir.path().join("archive").exists());
        join_server(h).await;
    }

    #[tokio::test]
    async fn rejects_redirect_to_downgraded_scheme() {
        // example.com is NOT loopback, so the S2 debug-loopback carve-out
        // in `check_scheme_result` does not apply to it — this actually
        // exercises the https-only check on the REDIRECT TARGET. A
        // `http://127.0.0.1/...` target would prove nothing here, since
        // that is exactly what S2 permits for the hermetic-test bypass.
        // No connection to example.com is ever made: the custom redirect
        // policy evaluates `Attempt::url()` (a plain `Url` value) and
        // rejects before reqwest attempts to connect to it.
        let (url, h) = serve_redirect("http://example.com/evil");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_with(
            &u,
            &"0".repeat(64),
            dir.path(),
            Limits {
                cap: 1024,
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();
        // Prove the REDIRECT POLICY rejected the hop (reqwest surfaces a
        // redirect-kind error whose Display contains "redirect") rather than a
        // downstream connect/send failure to the target. This makes the test
        // self-proving regardless of outbound network: a regressed scheme
        // check that actually followed the redirect would fail with a
        // connect/send error ("error sending request…"), never "redirect".
        let PkgError::Network(msg) = &err else {
            panic!("expected Network error, got {err:?}");
        };
        assert!(
            msg.to_lowercase().contains("redirect"),
            "expected a redirect-policy rejection, got: {msg}"
        );
        assert!(!dir.path().join("archive").exists());
        h.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_redirect_with_userinfo() {
        // Bogus port (never actually connected to — the policy rejects on
        // the URL's userinfo alone, before any connection attempt).
        let (url, h) = serve_redirect("https://user:pw@127.0.0.1:9/evil");
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_with(
            &u,
            &"0".repeat(64),
            dir.path(),
            Limits {
                cap: 1024,
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();
        let PkgError::Network(msg) = &err else {
            panic!("expected Network error, got {err:?}");
        };
        assert!(
            msg.to_lowercase().contains("redirect"),
            "expected a redirect-policy rejection, got: {msg}"
        );
        assert!(!dir.path().join("archive").exists());
        h.join().unwrap();
    }

    // ======================================================================
    // Stall detection. These three share ONE fixture — a server that dribbles
    // a body in timed pieces — and differ only in the stall window, so the
    // pair below cannot both be satisfied by a degenerate implementation:
    // a stall check that never fires fails `stalled_transfer_...`, and one
    // that fires on elapsed time rather than on silence fails
    // `slow_but_progressing_...`.
    // ======================================================================

    /// The transfer dies mid-body. The error must SAY that — how far it got,
    /// how fast it was going, how long it was silent — rather than arriving
    /// as a generic network fault indistinguishable from a slow link.
    #[tokio::test]
    async fn a_transfer_that_goes_silent_is_reported_as_a_stall_with_the_observed_rate() {
        let body = vec![7u8; 4096];
        // 4 chunks declared, only the first actually sent, then silence.
        let (url, h) = serve_dribble(body, 4, std::time::Duration::ZERO, true);
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let t0 = std::time::Instant::now();
        let err = download_with(
            &u,
            &"0".repeat(64),
            dir.path(),
            Limits {
                stall: std::time::Duration::from_millis(250),
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();

        let PkgError::DownloadStalled {
            received, expected, ..
        } = &err
        else {
            panic!("expected DownloadStalled, got {err:?}");
        };
        assert!(
            *received > 0 && *received < 4096,
            "should have received a partial body, got {received}"
        );
        assert_eq!(*expected, Some(4096), "the declared length must be carried");
        // The message is the whole point of this variant: it has to let a
        // user tell "too slow" from "disconnected".
        let msg = err.to_string();
        assert!(
            msg.contains("stalled"),
            "message must name the stall: {msg}"
        );
        assert!(msg.contains("KiB/s"), "message must name the rate: {msg}");
        assert!(
            msg.contains("of 4096 bytes"),
            "message must name how far it got: {msg}"
        );
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(5),
            "the stall window, not the 60s transport backstop, must be what fires"
        );
        assert!(
            !dir.path().join("archive").exists(),
            "partial must be swept"
        );
        join_server(h).await;
    }

    /// THE regression this fix exists for: a transfer far slower than any
    /// fixed wall clock would have allowed still SUCCEEDS, because the clock
    /// measures silence and every chunk resets it. Total transfer time here
    /// is ~2.4x the stall window; under the old `ClientBuilder::timeout` a
    /// transfer outlasting the configured wall clock failed no matter how
    /// steadily it was progressing.
    #[tokio::test]
    async fn a_slow_but_progressing_transfer_outlasts_its_own_stall_window_and_still_succeeds() {
        let body: Vec<u8> = (0..1200u32).map(|i| (i % 251) as u8).collect();
        let sha = sha_hex(&body);
        let gap = std::time::Duration::from_millis(120);
        let (url, h) = serve_dribble(body.clone(), 10, gap, false);
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let t0 = std::time::Instant::now();
        let mut f = download_with(
            &u,
            &sha,
            dir.path(),
            Limits {
                // 5x each individual gap, but well under the total.
                stall: std::time::Duration::from_millis(600),
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .expect("a steadily-progressing transfer must not be treated as a stall");
        let elapsed = t0.elapsed();

        let mut got = Vec::new();
        use std::io::{Seek, SeekFrom};
        f.seek(SeekFrom::Start(0)).unwrap();
        f.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        assert!(
            elapsed > std::time::Duration::from_millis(600),
            "fixture is not exercising the reset: the whole transfer ({elapsed:?}) \
             finished inside one stall window"
        );
        join_server(h).await;
    }

    /// Non-vacuity twin of the test above: SAME server, same 120 ms gaps,
    /// only the stall window changes (600 ms -> 30 ms). If the stall check
    /// were inert, this would succeed exactly like its twin.
    #[tokio::test]
    async fn the_same_dribble_is_a_stall_once_the_window_is_shorter_than_the_gap() {
        let body: Vec<u8> = (0..1200u32).map(|i| (i % 251) as u8).collect();
        let sha = sha_hex(&body);
        let (url, h) = serve_dribble(body, 10, std::time::Duration::from_millis(120), false);
        let u = url::Url::parse(&url).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = download_with(
            &u,
            &sha,
            dir.path(),
            Limits {
                stall: std::time::Duration::from_millis(30),
                ..Limits::PRODUCTION
            },
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, PkgError::DownloadStalled { .. }),
            "expected DownloadStalled, got {err:?}"
        );
        assert!(
            !dir.path().join("archive").exists(),
            "partial must be swept"
        );
        join_server(h).await;
    }

    /// Hermetic, network-free proof of the per-hop validation LOGIC itself
    /// (S1) — complements the live-socket redirect tests, which prove reqwest
    /// wires this check into its redirect policy. A regression in
    /// `check_scheme_result` is caught here instantly, with zero dependence on
    /// outbound network reachability.
    #[test]
    fn check_scheme_result_rejects_hostile_targets() {
        let ok = |s: &str| check_scheme_result(&url::Url::parse(s).unwrap()).is_ok();
        // Rejected: scheme downgrade to a non-loopback host, userinfo, IP-literal.
        assert!(
            !ok("http://example.com/evil"),
            "non-loopback http must be rejected"
        );
        assert!(
            !ok("https://user:pw@example.com/evil"),
            "userinfo must be rejected"
        );
        assert!(
            !ok("https://1.2.3.4/evil"),
            "IP-literal host must be rejected"
        );
        // Accepted: plain https to a domain.
        assert!(
            ok("https://www.php.net/x.tar.gz"),
            "plain https domain must be accepted"
        );
        // Debug-only loopback-http carve-out (S2) — active under `cargo test`.
        #[cfg(debug_assertions)]
        assert!(
            ok("http://127.0.0.1:8080/x"),
            "debug loopback-http carve-out"
        );
    }
}
