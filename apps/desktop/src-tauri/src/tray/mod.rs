// SPDX-License-Identifier: GPL-3.0-or-later
//! The tray / menu-bar quick-controls slice (P1 tray design
//! `docs/superpowers/specs/2026-07-31-p1-tray-design.md`).
//!
//! `model` is Phase A: the pure menu model — no tray, no menu, no AppKit,
//! nothing wired to a real tray yet (spec D9). That wiring (a real
//! `muda`/`tray-icon` menu, snapshot reconciliation, the click router) is
//! Phase B's job and lands in later commits alongside this module.

// `model`'s whole surface (types + the four pure functions) has no
// production caller yet — only its own test module exercises it — so every
// item in it would otherwise warn as dead code. The later Phase B commit
// that builds the real tray calls `tray_model`/`toggle_action`/
// `bulk_start_ids`/`aggregate_icon` from `setup()`/the click router, at
// which point this allow comes off, mirroring the precedent at
// `openvhost-proc`'s `log::RingBuffer::len` and the P0-8
// `ProcessRegistry`/`platform` allows (both dropped the moment a real
// caller landed). An attribute on this `mod` item covers the whole
// separate-file module beneath it, so `model.rs` itself stays free of
// lint-suppression clutter.
#[allow(dead_code)]
pub mod model;
