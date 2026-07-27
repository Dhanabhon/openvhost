// SPDX-License-Identifier: GPL-3.0-or-later
//! Editable nginx settings: connection limits, timeouts, upload size, and
//! compression. Pure values — no IO, no database — because every field here
//! ends up inside a generated nginx config file, the same boundary a `$` in
//! a docroot once slipped through (see `openvhost_core::site::model`). Each
//! value is a validated newtype in [`value`]; `parse` is the only public
//! constructor for any of them.

mod value;

pub use value::{BodySize, GzipLevel, GzipTypes, OnOff, Seconds, WorkerConnections};

/// Development-appropriate default `gzip_types` list — a handful of common
/// compressible text formats, not nginx's own (empty) default. See
/// [`WebServerSettings::default`] for why a development-appropriate default
/// is safe to choose here.
const DEFAULT_GZIP_TYPES: &str = "text/plain text/css application/json application/javascript application/xml image/svg+xml font/woff2";

/// The nginx settings the Web server page can edit. Every field is a
/// validated newtype from [`value`], so nothing unparsed can reach the
/// generated config that Task 3 renders from this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebServerSettings {
    pub worker_connections: WorkerConnections,
    pub client_max_body_size: BodySize,
    pub keepalive_timeout: Seconds,
    pub tcp_nodelay: OnOff,
    pub fastcgi_connect_timeout: Seconds,
    pub fastcgi_send_timeout: Seconds,
    pub fastcgi_read_timeout: Seconds,
    pub gzip: OnOff,
    pub gzip_comp_level: GzipLevel,
    pub gzip_types: GzipTypes,
}

impl Default for WebServerSettings {
    /// Development-appropriate rather than nginx's own (spec §5). Safe to
    /// choose because the diff preview shows the user exactly what changes
    /// before it lands — without that, nginx's values would be the only
    /// defensible defaults.
    ///
    /// Built from `new_unchecked` rather than `parse`, because `Default`
    /// cannot fail and threading a fallible constructor through it would hide
    /// which value is real behind error handling that can never fire. Every
    /// constant here is inside the bounds its own `parse` enforces, and
    /// `every_default_would_survive_its_own_parser` is what keeps that true.
    fn default() -> Self {
        Self {
            worker_connections: WorkerConnections::new_unchecked(1024),
            client_max_body_size: BodySize::new_unchecked("256m"),
            keepalive_timeout: Seconds::new_unchecked(65),
            tcp_nodelay: OnOff::new(true),
            fastcgi_connect_timeout: Seconds::new_unchecked(60),
            fastcgi_send_timeout: Seconds::new_unchecked(300),
            fastcgi_read_timeout: Seconds::new_unchecked(300),
            gzip: OnOff::new(false),
            gzip_comp_level: GzipLevel::new_unchecked(1),
            gzip_types: GzipTypes::new_unchecked(DEFAULT_GZIP_TYPES),
        }
    }
}
