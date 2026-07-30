-- SPDX-License-Identifier: GPL-3.0-or-later
CREATE TABLE mysql_instances (
    major          TEXT    PRIMARY KEY NOT NULL,
    root_password  TEXT    NOT NULL,
    initialized_at INTEGER NOT NULL
) STRICT;
