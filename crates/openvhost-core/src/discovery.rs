// SPDX-License-Identifier: GPL-3.0-or-later
//! What one discovery pass saw — including what it could NOT identify.

use std::path::PathBuf;

/// The result of scanning the Homebrew prefixes for one family of runtime.
///
/// The two fields are not interchangeable, and collapsing them into a bare
/// `Vec<Runtime>` was a real, reproduced bug. An empty `runtimes` reads as
/// "nothing is installed", and every caller treated it that way — but a
/// candidate formula directory that holds the right binaries and whose VERSION
/// could not be read is installed. Reporting it as absent is how a successful
/// `brew install mysql@8.4` came back as "not detected": the version probe was
/// killed at its 5 s bound during macOS's ~11.5 s first-run scan of the freshly
/// extracted `mysqld`, and `Ok(vec![])` could not tell the caller the
/// difference between "no MySQL here" and "I could not tell".
///
/// This is the fifth time in this codebase that one value has been asked to
/// carry two states. Callers may not reconstruct the distinction by
/// re-scanning; they read [`Discovery::unidentified`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery<T> {
    /// Every runtime this pass positively identified, deduplicated and sorted
    /// by major.
    pub runtimes: Vec<T>,
    /// Candidate formula directories that hold the binaries this app needs but
    /// whose version could be read neither from Homebrew's own keg path nor
    /// from a version probe. Non-empty means the answer above is INCOMPLETE.
    pub unidentified: Vec<PathBuf>,
}

impl<T> Discovery<T> {
    /// Whether every candidate was accounted for — i.e. whether an empty
    /// [`Discovery::runtimes`] genuinely means "nothing is installed".
    pub fn is_complete(&self) -> bool {
        self.unidentified.is_empty()
    }
}

// Hand-written rather than derived: `#[derive(Default)]` on a generic struct
// demands `T: Default`, which no runtime type has or should have.
impl<T> Default for Discovery<T> {
    fn default() -> Self {
        Self {
            runtimes: Vec::new(),
            unidentified: Vec::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_scan_and_an_unreadable_one_are_different_values() {
        // The whole reason this type exists: these two must not compare equal,
        // because a caller that cannot tell them apart reports a successful
        // install as a failure.
        let nothing_installed: Discovery<u8> = Discovery::default();
        let could_not_tell: Discovery<u8> = Discovery {
            runtimes: vec![],
            unidentified: vec![PathBuf::from("/opt/homebrew/opt/mysql@8.4")],
        };
        assert_ne!(nothing_installed, could_not_tell);
        assert!(nothing_installed.is_complete());
        assert!(!could_not_tell.is_complete());
    }
}
