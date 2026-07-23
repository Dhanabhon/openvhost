// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared helpers for this crate's integration tests. Included via `mod common;`
//! from more than one test binary, so not every item is used by every file —
//! hence the file-level dead_code allow.
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// Grab a free ephemeral TCP port: bind :0, read the assignment, release it.
/// Standard test pattern (see `macos_stack.rs`); the tiny reuse race surfaces
/// as a loud `Failed`/deadline failure, never a hang.
pub fn ephemeral_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind ephemeral")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Poll a raw HTTP GET on `127.0.0.1:port` until a response arrives or the
/// deadline passes. Cross-OS replacement for shelling out to `curl` — no
/// external process, identical on macOS and Windows.
pub fn http_get(port: u16, deadline: Instant) -> Option<String> {
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ =
                stream.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut resp = String::new();
            if stream.read_to_string(&mut resp).is_ok() && !resp.is_empty() {
                return Some(resp);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
