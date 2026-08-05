-- SPDX-License-Identifier: GPL-3.0-or-later
--
-- Its own table, not a discriminator column on `mysql_instances` (spec D4:
-- docs/superpowers/specs/2026-08-04-p1-mariadb-service-design.md). That
-- table's primary key is `major`, so sharing it needs a composite key, a
-- table rewrite, and a name that has become a lie. Nor a second row in it:
-- `major` is the PK, and "11.4" not colliding with "8.4" today is an
-- accident, not a constraint.
CREATE TABLE mariadb_instances (
    major          TEXT    PRIMARY KEY NOT NULL,
    root_password  TEXT    NOT NULL,
    initialized_at INTEGER NOT NULL
) STRICT;
