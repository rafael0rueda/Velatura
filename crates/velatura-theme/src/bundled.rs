//! Themes compiled into the binary, so they are found the same way from
//! source, from an install and from a sandbox.

use crate::{Theme, parse_theme};

/// `(id, file content)`. These ids are reserved: a user theme cannot take one.
pub const SOURCES: [(&str, &str); 2] = [
    ("aetherius", include_str!("../themes/aetherius.toml")),
    ("ember", include_str!("../themes/ember.toml")),
];

pub fn is_reserved(id: &str) -> bool {
    SOURCES.iter().any(|(bundled, _)| *bundled == id)
}

/// The bundled themes. They are validated by this crate's tests, so a failure
/// here is a build defect and yields an empty list rather than a panic.
pub fn themes() -> Vec<Theme> {
    SOURCES
        .iter()
        .filter_map(|(_, src)| parse_theme(src).ok().map(|l| l.theme))
        .collect()
}
