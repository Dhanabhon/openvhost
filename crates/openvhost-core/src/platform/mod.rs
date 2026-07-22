// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform-specific provisioning. Each OS lives in a `#[cfg(target_os)]`
//! submodule; the master-plan §6.2 ownership glob (`src/platform/macos*`)
//! assigns the macOS tree to platform-macos-specialist.

#[cfg(target_os = "macos")]
pub mod macos;
