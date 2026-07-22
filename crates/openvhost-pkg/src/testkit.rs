// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only archive builders. Lets unit tests construct adversarial tar.gz
//! and zip archives in memory so the extractor is exercised against real
//! hostile inputs, not mocks.
#![allow(clippy::unwrap_used)]

use std::io::Write;

/// One tar entry to build via [`targz_bytes`]. Mirrors the archive entry
/// kinds the extractor's pass 1 must classify and validate.
pub(crate) enum TarSpec {
    File {
        path: &'static str,
        data: &'static [u8],
        mode: u32,
    },
    Dir {
        path: &'static str,
    },
    Symlink {
        path: &'static str,
        target: &'static str,
    },
    Hardlink {
        path: &'static str,
        target: &'static str,
    },
    Fifo {
        path: &'static str,
    },
}

/// Write `path` directly into the header's raw (fixed 100-byte) name field,
/// bypassing `tar::Header::set_path`'s own component validation.
///
/// `tar-rs`'s `Builder::append_data` → `Header::set_path` REJECTS `..` and
/// absolute paths at *build* time (confirmed against the vendored
/// `tar-0.4.46` source: `copy_path_into_inner` errors on
/// `Component::ParentDir`/`Component::RootDir` when `is_link_name` is
/// false) — so a hostile fixture built through the normal API would panic
/// in the test harness instead of ever reaching our extractor. Real
/// attackers don't go through this crate's safety rails; a hand-crafted (or
/// GNU-tar-built) archive can carry any bytes in the name field. Writing
/// the field directly reproduces that and lets tests exercise our own
/// `validate_entry_name` against a genuinely hostile archive.
fn set_raw_name(h: &mut tar::Header, path: &str) {
    let bytes = path.as_bytes();
    let name = &mut h.as_old_mut().name;
    assert!(
        bytes.len() < name.len(),
        "fixture path {path:?} too long for the raw tar name field"
    );
    name[..bytes.len()].copy_from_slice(bytes);
}

/// Build a gzip-compressed tar archive in memory from `entries`. Entry names
/// and link targets are written verbatim (see [`set_raw_name`]), so callers
/// can construct genuinely hostile archives — traversal, absolute paths,
/// escaping link targets — to exercise the extractor's own validation.
pub(crate) fn targz_bytes(entries: &[TarSpec]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    let gz = GzEncoder::new(Vec::new(), Compression::fast());
    let mut ar = tar::Builder::new(gz);
    for e in entries {
        let mut h = tar::Header::new_gnu();
        match e {
            TarSpec::File { path, data, mode } => {
                h.set_size(data.len() as u64);
                h.set_mode(*mode);
                h.set_entry_type(tar::EntryType::Regular);
                set_raw_name(&mut h, path);
                h.set_cksum();
                ar.append(&h, &data[..]).unwrap();
            }
            TarSpec::Dir { path } => {
                h.set_size(0);
                h.set_mode(0o755);
                h.set_entry_type(tar::EntryType::Directory);
                set_raw_name(&mut h, path);
                h.set_cksum();
                ar.append(&h, std::io::empty()).unwrap();
            }
            TarSpec::Symlink { path, target } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_link_name(target).unwrap();
                set_raw_name(&mut h, path);
                h.set_cksum();
                ar.append(&h, std::io::empty()).unwrap();
            }
            TarSpec::Hardlink { path, target } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Link);
                h.set_link_name(target).unwrap();
                set_raw_name(&mut h, path);
                h.set_cksum();
                ar.append(&h, std::io::empty()).unwrap();
            }
            TarSpec::Fifo { path } => {
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Fifo);
                set_raw_name(&mut h, path);
                h.set_cksum();
                ar.append(&h, std::io::empty()).unwrap();
            }
        }
    }
    let gz = ar.into_inner().unwrap();
    gz.finish().unwrap()
}

/// Write bytes to a NamedTempFile and return it opened read+write, rewound —
/// mirrors the download module handing extraction an open, verified handle.
pub(crate) fn temp_file_with(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(bytes).unwrap();
    f.flush().unwrap();
    f
}
