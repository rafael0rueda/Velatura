//! Renderer for Starship: colours in, the lines Velatura adds to
//! `starship.toml` out.
//!
//! Starship has no way to load a second file, so these lines go into the
//! user's own, inside Velatura's marked block at the top. They are keys of
//! the file's root, written with dots, so they hold at the top of the file
//! whatever tables follow.

use std::fmt::Write;

use crate::Resolved;

/// Bumped whenever the set of names or their mapping changes.
pub const FORMAT: u32 = 1;

/// The name of the palette, as `palette` selects it.
pub const PALETTE: &str = "velatura";

/// The names the palette defines besides the interface roles: Starship's
/// own slots. A prompt shows a colour only where its settings name it.
pub const SLOTS: [&str; 3] = ["directory", "git", "language"];

pub fn render(theme: &Resolved<'_>) -> String {
    let v = theme.colors;
    let mut out = format!("palette = \"{PALETTE}\"\n");
    let slots = [v.ansi[6], v.ansi[5], v.ansi[2]];
    let named = v.ui.roles().into_iter().chain(
        SLOTS
            .into_iter()
            .zip(slots)
            .map(|(name, fallback)| (name, v.slot("starship", name, fallback))),
    );
    for (name, color) in named {
        let _ = writeln!(out, "palettes.{PALETTE}.{name} = \"{}\"", color.hex());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    #[test]
    fn golden() {
        let themes = bundled::themes();
        let golden = [
            include_str!("../tests/golden/aetherius.starship.toml"),
            include_str!("../tests/golden/ember.starship.toml"),
        ];
        for (theme, expected) in themes.iter().zip(golden) {
            assert_eq!(render(&Resolved::new(theme, None).unwrap()), expected);
        }
    }

    /// The lines are TOML, and hold exactly the palette and its choice.
    #[test]
    fn the_lines_are_toml() {
        for theme in bundled::themes() {
            let text = render(&Resolved::new(&theme, None).unwrap());
            let table: toml::Table = text.parse().unwrap();
            assert_eq!(table["palette"].as_str(), Some(PALETTE));
            let palette = table["palettes"][PALETTE].as_table().unwrap();
            assert_eq!(palette.len(), 17 + SLOTS.len());
            for value in palette.values() {
                assert!(crate::Color::parse(value.as_str().unwrap()).is_ok());
            }
        }
    }
}
