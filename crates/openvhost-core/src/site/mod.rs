// SPDX-License-Identifier: GPL-3.0-or-later
//! The `Site` domain: the aggregate plus its validated newtypes ([`model`])
//! and its persistence seam ([`repo`]).
//!
//! `model` and `repo` are siblings on purpose. Rust makes a private field
//! visible to its defining module *and every descendant module* — so if the
//! newtypes lived in this parent module, `repo` (a child) could write e.g.
//! `Domain(raw)` directly and bypass `Domain::parse`, defeating the
//! parse-don't-validate guarantee with no compiler error. Keeping the newtypes
//! in the sibling `model` puts their private fields out of `repo`'s reach:
//! persistence must go through the public `parse`/`as_str` API like any other
//! consumer.

pub mod apply;
pub mod model;
pub mod repo;
pub mod scaffold;

pub use model::{Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};
