-- SPDX-License-Identifier: GPL-3.0-or-later
CREATE TABLE sites (
    id          TEXT    PRIMARY KEY NOT NULL,
    name        TEXT    NOT NULL UNIQUE,
    domain      TEXT    NOT NULL UNIQUE,
    docroot     TEXT    NOT NULL,
    web_server  TEXT    NOT NULL,
    php_version TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
