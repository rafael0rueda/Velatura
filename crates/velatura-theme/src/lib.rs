//! Theme model and colour maths for Velatura.
//!
//! This crate performs no I/O. Everything a theme file contributes to a
//! generated config passes through [`Color`], which can only be turned into
//! text by a closed set of formatters.

mod accent;
pub mod bundled;
mod color;
pub mod editor;
pub mod ghostty;
pub mod gtk;
pub mod kitty;
pub mod model;
pub mod nvim;
pub mod shell;
pub mod starship;
pub mod tmux;

pub use accent::{Accent, nearest_accent};
pub use color::{Color, ParseColorError};
pub use model::{Appearance, Diagnostic, Invalid, Loaded, Theme, Ui, Variant, parse_theme};

use sha2::{Digest, Sha256};

/// SHA-256 of `bytes` as 64 lower-case hex digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// One variant of one theme, ready to render.
#[derive(Clone, Copy, Debug)]
pub struct Resolved<'a> {
    pub theme: &'a str,
    pub variant: &'a str,
    pub colors: &'a Variant,
}

impl<'a> Resolved<'a> {
    /// Pick `variant`, or the theme's default when `None`.
    pub fn new(theme: &'a Theme, variant: Option<&str>) -> Option<Self> {
        let key = variant.unwrap_or_else(|| theme.default_variant());
        let (key, colors) = theme.variants.get_key_value(key)?;
        Some(Self {
            theme: &theme.id,
            variant: key,
            colors,
        })
    }

    /// `theme/variant`: slugs only, safe to write into a generated file.
    pub fn label(&self) -> String {
        format!("{}/{}", self.theme, self.variant)
    }

    /// Identifies the colours, not the file they came from: 16 hex digits.
    pub fn fingerprint(&self) -> String {
        let mut text = String::new();
        for (_, c) in self.colors.ui.roles() {
            text.push_str(&c.hex());
        }
        for c in self.colors.ansi {
            text.push_str(&c.hex());
        }
        sha256_hex(text.as_bytes())[..16].to_owned()
    }
}
