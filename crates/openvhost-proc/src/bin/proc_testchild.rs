// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only child binary for this crate's integration tests.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match openvhost_proc::testchild::parse(&args) {
        Ok(a) => std::process::exit(openvhost_proc::testchild::run(a)),
        Err(e) => {
            eprintln!("proc_testchild: {e}");
            std::process::exit(64);
        }
    }
}
