// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost — OpenVHost CLI (stub: prints version and exits 0).
//! Real verbs (start|stop|restart|status|list --json) land in Phase 1.
//! `__testchild` is an internal deterministic child for supervisor
//! development and demos — not a public interface.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("__testchild") {
        match openvhost_proc::testchild::parse(&args[1..]) {
            Ok(a) => std::process::exit(openvhost_proc::testchild::run(a)),
            Err(e) => {
                eprintln!("openvhost __testchild: {e}");
                std::process::exit(64);
            }
        }
    }
    println!("openvhost {}", env!("CARGO_PKG_VERSION"));
}
