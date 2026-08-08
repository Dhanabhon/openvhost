-- SPDX-License-Identifier: GPL-3.0-or-later
-- Which PHP major the catch-all serves, when someone has actually chosen one.
--
-- A singleton, mirroring `web_server_settings` (0002): `id` can only ever be 1,
-- so "which row is the real one" is not a question any query has to answer.
--
-- Deliberately NOT a column on `web_server_settings`. That table holds nginx
-- directives and nothing else, and its values reach exactly one generated file
-- (the main config); this one selects a php-fpm pool for a different file. See
-- the design doc's D1.
--
-- `default_major` is NULLABLE, and the NULL is load-bearing: it is "nobody has
-- chosen", which is what every machine looks like today and what makes the
-- historical first-installed rule still apply. It is NOT "8.1" written down by
-- a migration — that would silently promote an accident into a decision.
CREATE TABLE php_settings (
    id            INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    default_major TEXT,
    updated_at    INTEGER NOT NULL
) STRICT;
