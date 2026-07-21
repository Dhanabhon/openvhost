// SPDX-License-Identifier: GPL-3.0-or-later
//! openservctl — OpenServ CLI (stub: prints version and exits 0).
//! Real verbs (start|stop|restart|status|list --json) land in Phase 1.

fn main() {
    println!("openservctl {}", env!("CARGO_PKG_VERSION"));
}
