//! Templates of the user's: any text, with a theme's colours put in where
//! it names them.
//!
//! A placeholder is `{{name}}` or `{{name|format}}`. The names are the
//! theme's interface roles (`bg`, `accent`, …), its sixteen terminal
//! colours (`black` … `bright_white`), `syntax.<role>`, `diff.<role>` and
//! `diagnostic.<role>`, and `appearance`, which is the word `dark` or
//! `light`. The formats are `hex` (`#rrggbb`, the default), `bare`
//! (`rrggbb`), `rgb` (`224, 145, 79`) and `float` (`0.878, 0.569, 0.310`).
//! There are no conditions, loops or other templates: what is not a
//! placeholder is written as it is.
//!
//! A `{{` that does not have a placeholder's shape is text, so code that
//! happens to hold one (`{{1, 2}}` in Lua) is left alone. One that has the
//! shape and names something unknown is an error, with its line, so a
//! misspelt name never goes out as text.

use std::fmt::{self, Write};

use crate::model::{ANSI_NAMES, DIAGNOSTIC_ROLES, DIFF_ROLES, SYNTAX_ROLES};
use crate::{Appearance, Color, Resolved};

/// The most text a template may have.
pub const MAX_BYTES: usize = 256 * 1024;

/// The interface roles, in the order [`crate::Ui::roles`] gives them.
const UI_ROLES: [&str; 17] = [
    "bg",
    "fg",
    "accent",
    "bg_dim",
    "surface",
    "border",
    "fg_muted",
    "fg_subtle",
    "accent_fg",
    "selection_bg",
    "selection_fg",
    "cursor",
    "cursor_fg",
    "link",
    "success",
    "warning",
    "error",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Hex,
    Bare,
    Rgb,
    Float,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Name {
    Ui(usize),
    Ansi(usize),
    Syntax(&'static str),
    Diff(&'static str),
    Diagnostic(&'static str),
    Appearance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Text(String),
    Color(Name, Format),
    Appearance,
}

/// A template that has been read and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

/// Why a template cannot be used. Names and formats are only ever made of
/// lower-case letters, digits, `_` and `.`, so they can be shown as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    TooLarge,
    UnknownName {
        line: usize,
        name: String,
    },
    UnknownFormat {
        line: usize,
        format: String,
    },
    /// `appearance` is a word, not a colour, and takes no format.
    FormatOnWord {
        line: usize,
    },
    /// No placeholder at all: the template would write the same text for
    /// every theme.
    NoPlaceholder,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(f, "larger than {} KB", MAX_BYTES / 1024),
            Self::UnknownName { line, name } => write!(f, "line {line}: no color is called {name}"),
            Self::UnknownFormat { line, format } => write!(
                f,
                "line {line}: no format is called {format} (hex, bare, rgb or float)"
            ),
            Self::FormatOnWord { line } => {
                write!(f, "line {line}: appearance is a word and takes no format")
            }
            Self::NoPlaceholder => f.write_str("it names no color"),
        }
    }
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.'
}

/// `name` and `format` of the placeholder at the start of `text` (which
/// starts with `{{`), and its length. `None` if it does not have the shape.
fn shape(text: &str) -> Option<(&str, Option<&str>, usize)> {
    let inner_end = text.find("}}")?;
    let inner = &text[2..inner_end];
    if inner.contains('\n') {
        return None;
    }
    let (name, format) = match inner.split_once('|') {
        Some((n, f)) => (n.trim(), Some(f.trim())),
        None => (inner.trim(), None),
    };
    let word = |w: &str| !w.is_empty() && w.bytes().all(is_name_byte);
    (word(name) && format.is_none_or(word)).then_some((name, format, inner_end + 2))
}

fn name(text: &str) -> Option<Name> {
    if text == "appearance" {
        return Some(Name::Appearance);
    }
    if let Some(i) = UI_ROLES.iter().position(|r| *r == text) {
        return Some(Name::Ui(i));
    }
    if let Some(i) = ANSI_NAMES.iter().position(|r| *r == text) {
        return Some(Name::Ansi(i));
    }
    let (group, role) = text.split_once('.')?;
    let find = |roles: &[&'static str]| roles.iter().copied().find(|r| *r == role);
    match group {
        "syntax" => find(&SYNTAX_ROLES).map(Name::Syntax),
        "diff" => find(&DIFF_ROLES).map(Name::Diff),
        "diagnostic" => find(&DIAGNOSTIC_ROLES).map(Name::Diagnostic),
        _ => None,
    }
}

/// Read and check a template.
pub fn parse(text: &str) -> Result<Template, Problem> {
    if text.len() > MAX_BYTES {
        return Err(Problem::TooLarge);
    }
    let mut pieces = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let mut placeholders = 0;
    while let Some(at) = rest.find("{{") {
        let line = text[..text.len() - rest.len() + at].matches('\n').count() + 1;
        let Some((found, format, len)) = shape(&rest[at..]) else {
            plain.push_str(&rest[..at + 2]);
            rest = &rest[at + 2..];
            continue;
        };
        plain.push_str(&rest[..at]);
        let found = name(found).ok_or_else(|| Problem::UnknownName {
            line,
            name: found.to_owned(),
        })?;
        let format = match format {
            None | Some("hex") => Format::Hex,
            Some(_) if found == Name::Appearance => return Err(Problem::FormatOnWord { line }),
            Some("bare") => Format::Bare,
            Some("rgb") => Format::Rgb,
            Some("float") => Format::Float,
            Some(other) => {
                return Err(Problem::UnknownFormat {
                    line,
                    format: other.to_owned(),
                });
            }
        };
        if !plain.is_empty() {
            pieces.push(Piece::Text(std::mem::take(&mut plain)));
        }
        pieces.push(match found {
            Name::Appearance => Piece::Appearance,
            other => Piece::Color(other, format),
        });
        placeholders += 1;
        rest = &rest[at + len..];
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        pieces.push(Piece::Text(plain));
    }
    if placeholders == 0 {
        return Err(Problem::NoPlaceholder);
    }
    Ok(Template { pieces })
}

fn color(theme: &Resolved<'_>, name: &Name) -> Color {
    let v = theme.colors;
    match name {
        Name::Ui(i) => v.ui.roles()[*i].1,
        Name::Ansi(i) => v.ansi[*i],
        Name::Syntax(role) => v.syntax_color(role),
        Name::Diff(role) => v.diff_color(role),
        Name::Diagnostic(role) => v.diagnostic_color(role),
        Name::Appearance => v.ui.fg,
    }
}

impl Template {
    /// The template's text with the theme's colours in it.
    pub fn render(&self, theme: &Resolved<'_>) -> String {
        let mut out = String::new();
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Appearance => out.push_str(match theme.colors.appearance {
                    Appearance::Dark => "dark",
                    Appearance::Light => "light",
                }),
                Piece::Color(name, format) => {
                    let c = color(theme, name);
                    let _ = match format {
                        Format::Hex => write!(out, "{}", c.hex()),
                        Format::Bare => write!(out, "{}", c.hex_noprefix()),
                        Format::Rgb => write!(out, "{}, {}, {}", c.r, c.g, c.b),
                        Format::Float => write!(
                            out,
                            "{:.3}, {:.3}, {:.3}",
                            f64::from(c.r) / 255.0,
                            f64::from(c.g) / 255.0,
                            f64::from(c.b) / 255.0
                        ),
                    };
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    fn ember() -> crate::Theme {
        bundled::themes().remove(1)
    }

    #[test]
    fn placeholders_take_the_themes_colours_in_each_format() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let t = parse(
            "bg={{bg}} accent={{ accent | bare }}\nred={{red|rgb}} k={{syntax.keyword|float}}\nmode={{appearance}} add={{diff.add}} hint={{diagnostic.hint|hex}}\n",
        )
        .unwrap();
        let v = r.colors;
        let k = v.syntax_color("keyword");
        let float = |c: u8| format!("{:.3}", f64::from(c) / 255.0);
        assert_eq!(
            t.render(&r),
            format!(
                "bg={} accent={}\nred={}, {}, {} k={}, {}, {}\nmode=dark add={} hint={}\n",
                v.ui.bg.hex(),
                v.ui.accent.hex_noprefix(),
                v.ansi[1].r,
                v.ansi[1].g,
                v.ansi[1].b,
                float(k.r),
                float(k.g),
                float(k.b),
                v.diff_color("add").hex(),
                v.diagnostic_color("hint").hex()
            )
        );
    }

    #[test]
    fn every_name_can_be_used() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let mut all: Vec<String> = UI_ROLES.iter().map(|s| (*s).to_owned()).collect();
        all.extend(ANSI_NAMES.iter().map(|s| (*s).to_owned()));
        all.extend(SYNTAX_ROLES.iter().map(|s| format!("syntax.{s}")));
        all.extend(DIFF_ROLES.iter().map(|s| format!("diff.{s}")));
        all.extend(DIAGNOSTIC_ROLES.iter().map(|s| format!("diagnostic.{s}")));
        let text: String = all.iter().map(|n| format!("{{{{{n}}}}}\n")).collect();
        let out = parse(&text).unwrap().render(&r);
        assert_eq!(out.lines().count(), all.len());
        assert!(out.lines().all(|l| Color::parse(l).is_ok()), "{out}");
        // The interface roles are in the model's own order.
        let roles = r.colors.ui.roles();
        for (i, role) in UI_ROLES.iter().enumerate() {
            assert_eq!(roles[i].0, *role);
        }
    }

    #[test]
    fn what_is_not_a_placeholder_is_text() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let text = "local t = {{1, 2}}\n{{ }} {{Bg}} {{bg\n}} }} {\n{{fg}}";
        let out = parse(text).unwrap().render(&r);
        assert_eq!(
            out,
            format!(
                "local t = {{{{1, 2}}}}\n{{{{ }}}} {{{{Bg}}}} {{{{bg\n}}}} }}}} {{\n{}",
                r.colors.ui.fg.hex()
            )
        );
    }

    #[test]
    fn mistakes_are_refused_with_their_line() {
        assert_eq!(
            parse("a\nb {{acent}}"),
            Err(Problem::UnknownName {
                line: 2,
                name: "acent".to_owned()
            })
        );
        assert_eq!(
            parse("{{bg|HEX}}").err().map(|_| ()),
            Some(()),
            "upper case is not a format's shape, so it stays text, and then nothing is named"
        );
        assert_eq!(
            parse("{{bg|hsl}}"),
            Err(Problem::UnknownFormat {
                line: 1,
                format: "hsl".to_owned()
            })
        );
        assert_eq!(
            parse("x\n\n{{appearance|bare}}"),
            Err(Problem::FormatOnWord { line: 3 })
        );
        assert_eq!(parse("plain text"), Err(Problem::NoPlaceholder));
        assert_eq!(parse("{{syntax.nothing}}").err().map(|_| ()), Some(()));
        assert_eq!(parse(&"x".repeat(MAX_BYTES + 1)), Err(Problem::TooLarge));
    }

    /// Nothing but the template's own text and colours goes out: whatever
    /// a theme file says in its names never reaches the output.
    #[test]
    fn only_colours_come_from_the_theme() {
        let mut theme = ember();
        theme.name = "x\n}} {{ rm -rf".to_owned();
        let r = Resolved::new(&theme, None).unwrap();
        let out = parse("[{{bg}}]").unwrap().render(&r);
        assert_eq!(out, format!("[{}]", r.colors.ui.bg.hex()));
    }
}
