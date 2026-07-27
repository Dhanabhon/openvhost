-- SPDX-License-Identifier: GPL-3.0-or-later
-- A singleton: `id` can only ever be 1, so "which row is the real one" is not
-- a question any query has to answer.
CREATE TABLE web_server_settings (
    id                       INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    worker_connections       INTEGER NOT NULL,
    client_max_body_size     TEXT    NOT NULL,
    keepalive_timeout        INTEGER NOT NULL,
    tcp_nodelay              INTEGER NOT NULL,
    fastcgi_connect_timeout  INTEGER NOT NULL,
    fastcgi_send_timeout     INTEGER NOT NULL,
    fastcgi_read_timeout     INTEGER NOT NULL,
    gzip                     INTEGER NOT NULL,
    gzip_comp_level          INTEGER NOT NULL,
    gzip_types               TEXT    NOT NULL,
    updated_at               INTEGER NOT NULL
) STRICT;
