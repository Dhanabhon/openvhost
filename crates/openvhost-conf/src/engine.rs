// SPDX-License-Identifier: GPL-3.0-or-later
//! The process-wide Tera instance, built once from the embedded templates.
//! Autoescaping is OFF: these render `.conf` files, not HTML, so `&`/`<`/`>`
//! in a path or directive must pass through verbatim.

use std::sync::OnceLock;

use tera::Tera;

use crate::error::ConfError;

const MAIN_NGINX: &str = include_str!("../templates/nginx/main.conf.tera");
const SITE_NGINX: &str = include_str!("../templates/nginx/site.conf.tera");
const PHP_LOCATION: &str = include_str!("../templates/nginx/php-location.conf.tera");
const DEFAULT_SITE_NGINX: &str = include_str!("../templates/nginx/default-site.conf.tera");
const POOL_FPM: &str = include_str!("../templates/php-fpm/pool.conf.tera");

pub(crate) fn engine() -> &'static Tera {
    static ENGINE: OnceLock<Tera> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut t = Tera::default();
        t.autoescape_on(vec![]); // no HTML escaping for any template
        // The templates are compile-time constants (`include_str!`), so a parse
        // error is a programmer error, not a runtime condition. The workspace
        // denies `expect_used`/`unwrap_used` under `-D warnings`, so use an
        // explicit `panic!` (not restricted) rather than `.expect()`.
        if let Err(e) = t.add_raw_templates(vec![
            ("nginx/main.conf", MAIN_NGINX),
            ("nginx/site.conf", SITE_NGINX),
            ("nginx/php-location.conf", PHP_LOCATION),
            ("nginx/default-site.conf", DEFAULT_SITE_NGINX),
            ("php-fpm/pool.conf", POOL_FPM),
        ]) {
            panic!("embedded templates must parse: {e}");
        }
        t
    })
}

pub(crate) fn render(name: &str, ctx: &tera::Context) -> Result<String, ConfError> {
    engine()
        .render(name, ctx)
        .map_err(|e| ConfError::Render(format!("{name}: {e}")))
}
