-- SPDX-License-Identifier: GPL-3.0-or-later
--
-- The install ledger: exactly which package versions THIS app fetched into its
-- own `packages/` tree, and when (MySQL-from-tarball design D4). We asked the
-- compiled-in catalogue for a specific version, so we know what it is —
-- nothing recorded here is ever learned by executing a binary.
--
-- NOT the inventory. The package TREE is: `packages/<name>/<major>/<version>/`
-- exists if and only if that version is installed, and the per-major `current`
-- symlink says which one is selected. A row here can outlive a tree that was
-- removed out from under us, so nothing may read this table to decide what is
-- installed; it records the two facts the tree cannot express — that WE
-- installed it, and when.
--
-- Keyed on (name, major, version), not (name, major): several versions of one
-- major can sit side by side in the tree. Which of them is selected is the
-- `current` symlink's answer, never a column here.
CREATE TABLE installed_packages (
    name         TEXT    NOT NULL,
    major        TEXT    NOT NULL,
    version      TEXT    NOT NULL,
    installed_at INTEGER NOT NULL,
    PRIMARY KEY (name, major, version)
) STRICT;
