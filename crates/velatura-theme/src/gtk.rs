//! Renderer for GTK: the named colours libadwaita and adw-gtk3 read.
//!
//! One renderer serves GTK4 and GTK3, because adw-gtk3 accepts the same
//! names. The output is a list of `@define-color` lines and nothing else:
//! no selectors, so it cannot change shapes or spacing, and no syntax that
//! an older GTK in a Flatpak runtime would reject.

use std::fmt::Write;

use crate::{Color, Resolved};

/// Bumped whenever the set of names or their mapping changes.
pub const FORMAT: u32 = 1;

/// Text below this contrast against the background is moved towards the
/// foreground colour until it reaches it.
const READABLE: f64 = 4.5;

fn alpha(color: Color, a: f64) -> Color {
    Color {
        a: (a * 255.0).round() as u8,
        ..color
    }
}

/// Whichever of the two reads better on `on`.
fn readable_on(on: Color, a: Color, b: Color) -> Color {
    if on.contrast(a) >= on.contrast(b) {
        a
    } else {
        b
    }
}

/// `color` as text on `bg`: itself if it is readable, otherwise moved
/// towards `fg` in steps until it is.
fn as_text(color: Color, bg: Color, fg: Color) -> Color {
    (0..=10)
        .map(|i| color.mix(fg, f64::from(i) / 10.0))
        .find(|c| c.contrast(bg) >= READABLE)
        .unwrap_or(fg)
}

/// Every name this renderer defines with its value, in output order.
pub fn colors(theme: &Resolved<'_>) -> Vec<(&'static str, Color)> {
    let ui = &theme.colors.ui;
    let shade = alpha(ui.bg_dim, 0.36);
    let sidebar = ui.bg.mix(ui.bg_dim, 0.5);
    let on = |c: Color| readable_on(c, ui.fg, ui.bg);
    let mut out = vec![
        ("window_bg_color", ui.bg),
        ("window_fg_color", ui.fg),
        ("view_bg_color", ui.bg_dim),
        ("view_fg_color", ui.fg),
        ("headerbar_bg_color", ui.surface),
        ("headerbar_fg_color", ui.fg),
        ("headerbar_border_color", ui.border),
        ("headerbar_backdrop_color", ui.bg),
        ("headerbar_shade_color", shade),
        ("headerbar_darker_shade_color", alpha(ui.bg_dim, 0.9)),
        ("sidebar_bg_color", sidebar),
        ("sidebar_fg_color", ui.fg),
        ("sidebar_backdrop_color", ui.bg_dim),
        ("sidebar_border_color", ui.bg_dim),
        ("sidebar_shade_color", shade),
        ("secondary_sidebar_bg_color", ui.bg_dim),
        ("secondary_sidebar_fg_color", ui.fg),
        ("secondary_sidebar_backdrop_color", ui.bg_dim),
        ("secondary_sidebar_border_color", ui.bg_dim),
        ("secondary_sidebar_shade_color", shade),
        ("card_bg_color", ui.surface),
        ("card_fg_color", ui.fg),
        ("card_shade_color", shade),
        ("dialog_bg_color", ui.surface),
        ("dialog_fg_color", ui.fg),
        ("popover_bg_color", ui.surface),
        ("popover_fg_color", ui.fg),
        ("popover_shade_color", shade),
        ("thumbnail_bg_color", ui.surface),
        ("thumbnail_fg_color", ui.fg),
        ("shade_color", shade),
        ("scrollbar_outline_color", ui.bg_dim),
        ("accent_bg_color", ui.accent),
        ("accent_fg_color", ui.accent_fg),
        ("accent_color", as_text(ui.accent, ui.bg, ui.fg)),
    ];
    for (name, color) in [
        ("destructive", ui.error),
        ("success", ui.success),
        ("warning", ui.warning),
        ("error", ui.error),
    ] {
        // The three names of each group are spelled out so that every name
        // in the output is a literal of this file.
        let (bg, fg, text) = match name {
            "destructive" => (
                "destructive_bg_color",
                "destructive_fg_color",
                "destructive_color",
            ),
            "success" => ("success_bg_color", "success_fg_color", "success_color"),
            "warning" => ("warning_bg_color", "warning_fg_color", "warning_color"),
            _ => ("error_bg_color", "error_fg_color", "error_color"),
        };
        out.push((bg, color));
        out.push((fg, on(color)));
        out.push((text, as_text(color, ui.bg, ui.fg)));
    }
    // The older names, for GTK apps that do not use libadwaita.
    out.extend([
        ("theme_bg_color", ui.bg),
        ("theme_fg_color", ui.fg),
        ("theme_base_color", ui.bg_dim),
        ("theme_text_color", ui.fg),
        ("theme_selected_bg_color", ui.accent),
        ("theme_selected_fg_color", ui.accent_fg),
        ("insensitive_bg_color", ui.bg),
        ("insensitive_fg_color", ui.fg_subtle),
        ("insensitive_base_color", ui.bg_dim),
        ("borders", ui.border),
        ("theme_unfocused_bg_color", ui.bg),
        ("theme_unfocused_fg_color", ui.fg),
        ("theme_unfocused_base_color", ui.bg_dim),
        ("theme_unfocused_text_color", ui.fg),
        ("theme_unfocused_selected_bg_color", ui.accent),
        ("theme_unfocused_selected_fg_color", ui.accent_fg),
        ("unfocused_insensitive_color", ui.fg_subtle),
        ("unfocused_borders", ui.border),
    ]);
    out
}

/// The names [`colors`] defines. The connector uses them to find lines in
/// the user's `gtk.css` that would override the theme.
pub fn names(theme: &Resolved<'_>) -> Vec<&'static str> {
    colors(theme).into_iter().map(|(name, _)| name).collect()
}

pub fn render(theme: &Resolved<'_>) -> String {
    let mut out = String::new();
    out.push_str(
        "/* Generated by Velatura. Edits to this file are replaced on the next apply. */\n",
    );
    let _ = writeln!(
        out,
        "/* theme {} fingerprint {} format {FORMAT} */",
        theme.label(),
        theme.fingerprint()
    );
    for (name, color) in colors(theme) {
        let value = if color.is_opaque() {
            color.hex()
        } else {
            color.css_rgba()
        };
        let _ = writeln!(out, "@define-color {name} {value};");
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
            include_str!("../tests/golden/aetherius.gtk.css"),
            include_str!("../tests/golden/ember.gtk.css"),
        ];
        for (theme, expected) in themes.iter().zip(golden) {
            assert_eq!(render(&Resolved::new(theme, None).unwrap()), expected);
        }
    }

    #[test]
    fn output_is_only_definitions_and_text_is_readable() {
        for theme in bundled::themes() {
            let r = Resolved::new(&theme, None).unwrap();
            let out = render(&r);
            for line in out.lines().skip(2) {
                let rest = line.strip_prefix("@define-color ").unwrap();
                let (name, value) = rest.strip_suffix(';').unwrap().split_once(' ').unwrap();
                assert!(name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
                assert!(
                    value.starts_with('#') || value.starts_with("rgba("),
                    "{line}"
                );
            }
            let all = colors(&r);
            let get = |n: &str| all.iter().find(|(name, _)| *name == n).unwrap().1;
            for text in [
                "accent_color",
                "error_color",
                "success_color",
                "warning_color",
            ] {
                assert!(
                    get(text).contrast(get("window_bg_color")) >= READABLE,
                    "{text}"
                );
            }
            // No name is defined twice.
            let mut seen: Vec<_> = all.iter().map(|(n, _)| *n).collect();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), all.len());
        }
    }
}
