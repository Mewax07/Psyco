//! The standard library files the compiler merges into every program.
//!
//! They are read from `psycoc/std/` when the sources are next to this crate
//! (so editing them is seen without rebuilding the server), and otherwise
//! come from the copy embedded at build time, like the compiler does.

use std::path::{Path, PathBuf};

pub const ALLOC_LINUX: &str = "std/alloc_linux.psy";

/// In the order the driver loads them.
pub const FILES: &[(&str, &str)] = &[
    ("std/prelude.psy", include_str!("../../../psycoc/std/prelude.psy")),
    ("std/alloc.psy", include_str!("../../../psycoc/std/alloc.psy")),
    (ALLOC_LINUX, include_str!("../../../psycoc/std/alloc_linux.psy")),
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../psycoc")
}

/// `std/x.psy` for an import of a standard library file, as the driver
/// matches them (by the end of the path).
pub fn name_of(import: &str) -> Option<&'static str> {
    let import = import.replace('\\', "/");
    FILES
        .iter()
        .map(|(name, _)| *name)
        .find(|name| import == *name || (import.starts_with("std/") && import.ends_with(name)))
}

/// Where the file lives in the compiler sources.
pub fn path(name: &str) -> PathBuf {
    let mut p = dir();
    p.extend(name.split('/'));
    p
}

/// The text of a standard library file: the open document or the file on
/// disk when there is one, the embedded copy otherwise.
pub fn source(name: &str, read: &dyn Fn(&Path) -> Option<String>) -> String {
    read(&path(name)).unwrap_or_else(|| {
        FILES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or_else(String::new, |(_, text)| text.to_string())
    })
}
