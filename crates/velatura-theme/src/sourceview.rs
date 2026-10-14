//! Renderer for GtkSourceView: colours in, the text of the style scheme
//! `velatura.xml` out. GNOME Text Editor, and other apps that show code
//! with GtkSourceView 5, read it.
//!
//! Every attribute value is a fixed word or a colour, so nothing from a
//! theme file other than its colours can end up in the XML.

use std::fmt::Write;

use crate::{Appearance, Color, Resolved};

/// Bumped whenever the set of styles or their mapping changes.
pub const FORMAT: u32 = 1;

/// The scheme's id: what an app's style setting is given.
pub const ID: &str = "velatura";

#[derive(Clone, Copy, Default)]
struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    italic: bool,
    /// `single`, `low` or `error`, with the colour of the line.
    underline: Option<(&'static str, Option<Color>)>,
    strike: bool,
}

fn fg(c: Color) -> Style {
    Style {
        fg: Some(c),
        ..Style::default()
    }
}

fn bg(c: Color) -> Style {
    Style {
        bg: Some(c),
        ..Style::default()
    }
}

fn on(fg: Color, bg: Color) -> Style {
    Style {
        fg: Some(fg),
        bg: Some(bg),
        ..Style::default()
    }
}

fn wavy(c: Color) -> Style {
    Style {
        underline: Some(("error", Some(c))),
        ..Style::default()
    }
}

impl Style {
    fn bold(self) -> Self {
        Self { bold: true, ..self }
    }
    fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }
    fn underlined(self, kind: &'static str) -> Self {
        Self {
            underline: Some((kind, None)),
            ..self
        }
    }

    fn xml(&self, name: &str) -> String {
        let mut out = format!("  <style name=\"{name}\"");
        for (key, color) in [("foreground", self.fg), ("background", self.bg)] {
            if let Some(color) = color {
                let _ = write!(out, " {key}=\"{}\"", color.hex());
            }
        }
        for (key, set) in [("bold", self.bold), ("italic", self.italic)] {
            if set {
                let _ = write!(out, " {key}=\"true\"");
            }
        }
        if let Some((kind, color)) = self.underline {
            let _ = write!(out, " underline=\"{kind}\"");
            if let Some(color) = color {
                let _ = write!(out, " underline-color=\"{}\"", color.hex());
            }
        }
        if self.strike {
            out.push_str(" strikethrough=\"true\"");
        }
        out.push_str("/>\n");
        out
    }
}

/// Every style the scheme sets, in output order.
fn styles(theme: &Resolved<'_>) -> Vec<(&'static str, Style)> {
    let v = theme.colors;
    let ui = &v.ui;
    let s = |role| v.syntax_color(role);
    let d = |role| v.diagnostic_color(role);
    vec![
        // The view itself.
        ("text", on(ui.fg, ui.bg)),
        ("selection", on(ui.selection_fg, ui.selection_bg)),
        ("cursor", fg(ui.cursor)),
        ("current-line", bg(ui.surface)),
        ("current-line-number", on(ui.accent, ui.surface).bold()),
        ("line-numbers", on(ui.fg_subtle, ui.bg)),
        ("right-margin", on(ui.border, ui.border)),
        ("draw-spaces", fg(ui.border)),
        ("background-pattern", bg(ui.bg_dim)),
        ("map-overlay", bg(ui.fg_subtle)),
        ("bracket-match", fg(ui.accent).bold()),
        ("bracket-mismatch", fg(ui.error).bold()),
        ("search-match", on(ui.bg, ui.warning)),
        // What languages build on.
        ("def:comment", fg(s("comment")).italic()),
        ("def:doc-comment-element", fg(ui.fg_muted).italic()),
        ("def:shebang", fg(s("comment")).bold()),
        ("def:string", fg(s("string"))),
        ("def:special-char", fg(s("escape"))),
        ("def:number", fg(s("number"))),
        ("def:decimal", fg(s("number"))),
        ("def:base-n-integer", fg(s("number"))),
        ("def:floating-point", fg(s("number"))),
        ("def:constant", fg(s("constant"))),
        ("def:boolean", fg(s("constant")).bold()),
        ("def:special-constant", fg(s("constant")).bold()),
        ("def:statement", fg(s("keyword"))),
        ("def:keyword", fg(s("keyword"))),
        ("def:operator", fg(s("operator"))),
        ("def:preprocessor", fg(s("attribute"))),
        ("def:type", fg(s("type"))),
        ("def:function", fg(s("function"))),
        ("def:builtin", fg(s("builtin"))),
        ("def:identifier", fg(s("variable"))),
        ("def:error", wavy(d("error"))),
        ("def:warning", wavy(d("warning"))),
        ("def:note", on(ui.bg, d("info")).bold()),
        ("def:underlined", Style::default().underlined("single")),
        ("def:net-address", fg(ui.link).underlined("low")),
        // Text with markup.
        ("def:heading", fg(ui.accent).bold()),
        ("def:emphasis", Style::default().italic()),
        ("def:strong-emphasis", Style::default().bold()),
        ("def:inline-code", fg(s("string"))),
        ("def:preformatted-section", fg(s("string"))),
        ("def:link-text", fg(ui.link)),
        (
            "def:link-destination",
            fg(ui.link).italic().underlined("low"),
        ),
        ("def:list-marker", fg(s("keyword")).bold()),
        ("def:insertion", fg(ui.success)),
        (
            "def:deletion",
            Style {
                strike: true,
                ..fg(ui.error)
            },
        ),
        // A few languages whose own names read better with a role of theirs.
        ("xml:element-name", fg(s("tag"))),
        ("xml:attribute-name", fg(s("attribute"))),
        ("xml:attribute-value", fg(s("string"))),
        ("css:property-name", fg(s("property"))),
        ("rust:attribute", fg(s("attribute"))),
        ("rust:macro", fg(s("builtin"))),
        ("python:builtin-function", fg(s("builtin"))),
        ("diff:added-line", fg(ui.success)),
        ("diff:removed-line", fg(ui.error)),
        ("diff:changed-line", fg(ui.warning)),
        ("diff:location", fg(d("info"))),
        ("diff:diff-file", fg(ui.accent).bold()),
    ]
}

pub fn render(theme: &Resolved<'_>) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<!-- Generated by Velatura. Edits to this file are replaced on the next apply. -->\n",
    );
    let _ = writeln!(
        out,
        "<!-- theme {} fingerprint {} format {FORMAT} -->",
        theme.label(),
        theme.fingerprint()
    );
    let _ = writeln!(
        out,
        "<style-scheme id=\"{ID}\" name=\"Velatura\" version=\"1.0\">"
    );
    out.push_str("  <author>Velatura</author>\n");
    out.push_str("  <description>The theme applied with Velatura</description>\n");
    let _ = writeln!(
        out,
        "  <metadata><property name=\"variant\">{}</property></metadata>",
        match theme.colors.appearance {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        }
    );
    for (name, style) in styles(theme) {
        out.push_str(&style.xml(name));
    }
    out.push_str("</style-scheme>\n");
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
            include_str!("../tests/golden/aetherius.sourceview.xml"),
            include_str!("../tests/golden/ember.sourceview.xml"),
        ];
        for (theme, expected) in themes.iter().zip(golden) {
            assert_eq!(render(&Resolved::new(theme, None).unwrap()), expected);
        }
    }

    #[test]
    fn styles_are_set_once() {
        let themes = bundled::themes();
        let theme = Resolved::new(&themes[0], None).unwrap();
        let mut names: Vec<&str> = styles(&theme).iter().map(|(n, _)| *n).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
    }
}
