//! The Shell's own stylesheet, restated in a theme's colours.
//!
//! GNOME Shell 47 and later name the accent in their stylesheet as
//! `-st-accent-color`, and work out its shades and those of every other
//! colour while they run, with `st-mix`, `st-lighten` and the like. So
//! every colour the Shell draws is one written in its stylesheet or a
//! function of them. Restating each declaration that holds a colour, with
//! the colour replaced, recolours all of it and keeps every shade in its
//! place, while the Shell's sizes, shapes and spacing stay its own.
//!
//! The greys are moved along the line from the theme's background to its
//! text: a grey a third of the way from the Shell's background to its text
//! becomes the colour a third of the way from the theme's. Red, yellow and
//! green become the theme's error, warning and success colours, and the
//! accent the theme's accent, exactly.
//!
//! The output is text of the Shell's own stylesheet with colours put in
//! place, and nothing a theme file says but colours.

use std::fmt::Write;

use crate::{Appearance, Color, Resolved, sha256_hex};

/// Colours the Shell's stylesheets write by name, as CSS defines them.
const NAMED: [(&str, Color); 6] = [
    ("white", Color::rgb(255, 255, 255)),
    ("black", Color::rgb(0, 0, 0)),
    ("whitesmoke", Color::rgb(245, 245, 245)),
    ("gainsboro", Color::rgb(220, 220, 220)),
    ("darkgray", Color::rgb(169, 169, 169)),
    ("gray", Color::rgb(128, 128, 128)),
];

/// A colour with more chroma than this is a colour of its own (an error
/// red, a warning yellow), not a grey.
const CHROMATIC: f64 = 0.06;

/// The rules of a stylesheet without its comments, each as its selectors
/// and its declarations. `None` for anything this does not expect from
/// a compiled stylesheet: nested blocks, unclosed ones.
fn rules(css: &str) -> Option<Vec<(String, String)>> {
    let mut text = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("/*") {
        text.push_str(&rest[..at]);
        rest = &rest[at + 2..];
        rest = &rest[rest.find("*/")? + 2..];
    }
    text.push_str(rest);
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let close = open + rest[open..].find('}')?;
        let body = &rest[open + 1..close];
        if body.contains('{') {
            return None;
        }
        let selectors = rest[..open]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push((selectors, body.to_owned()));
        rest = &rest[close + 1..];
    }
    rest.trim().is_empty().then_some(out)
}

/// Where the colours of one stylesheet go: the Shell's greys, by their
/// lightness, onto the theme's line from background to text; everything
/// else by what it is.
struct Ramp {
    /// The Shell's lightness and the theme's colour there, lightest last.
    points: Vec<(f64, Color)>,
    accent: Color,
    error: Color,
    warning: Color,
    success: Color,
}

impl Ramp {
    /// From the Shell's own text and background colours. `None` if they do
    /// not lie the way round the theme's do.
    fn new(theme: &Resolved<'_>, text: Color, back: Color) -> Option<Self> {
        let ui = &theme.colors.ui;
        let lightness = |c: Color| c.to_oklab()[0];
        let black = Color::rgb(0, 0, 0);
        let white = Color::rgb(255, 255, 255);
        let mut points = match theme.colors.appearance {
            Appearance::Dark if lightness(back) < lightness(text) => vec![
                (0.0, ui.bg_dim),
                (lightness(back), ui.bg),
                (lightness(text), ui.fg),
            ],
            Appearance::Light if lightness(text) < lightness(back) => vec![
                (0.0, ui.fg.mix(black, 0.5)),
                (lightness(text), ui.fg),
                (lightness(back), ui.bg),
                (1.0, ui.bg.mix(white, 0.5)),
            ],
            _ => return None,
        };
        points.dedup_by(|b, a| b.0 <= a.0);
        Some(Self {
            points,
            accent: ui.accent,
            error: ui.error,
            warning: ui.warning,
            success: ui.success,
        })
    }

    fn map(&self, stock: Color) -> Color {
        let [l, a, b] = stock.to_oklab();
        if a.hypot(b) > CHROMATIC {
            let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
            return match hue {
                h if !(40.0..330.0).contains(&h) => self.error,
                h if h < 120.0 => self.warning,
                h if h < 200.0 => self.success,
                _ => self.accent,
            };
        }
        let (first, last) = (self.points[0], self.points[self.points.len() - 1]);
        if l <= first.0 {
            return first.1;
        }
        if l >= last.0 {
            return last.1;
        }
        let i = self.points.iter().position(|(at, _)| *at > l).unwrap_or(1);
        let ((l0, c0), (l1, c1)) = (self.points[i - 1], self.points[i]);
        let t = (l - l0) / (l1 - l0);
        if t <= 0.0 {
            return c0;
        }
        let (p, q) = (c0.to_oklab(), c1.to_oklab());
        let mixed = [0, 1, 2].map(|k| p[k] + (q[k] - p[k]) * t);
        Color::from_oklab(mixed).unwrap_or_else(|| c0.mix(c1, t))
    }
}

/// A colour as the Shell's stylesheets write one: `#rrggbb`, `#rgb`, or a
/// name. `None` for anything else.
fn parse(token: &str) -> Option<Color> {
    if let Some((_, c)) = NAMED.iter().find(|(name, _)| *name == token) {
        return Some(*c);
    }
    let digits = token.strip_prefix('#')?;
    let long = match digits.len() {
        6 => digits.to_owned(),
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        _ => return None,
    };
    Color::parse(&format!("#{long}")).ok()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// `value` with every colour in it replaced, and whether there was one.
fn recolor(value: &str, ramp: &Ramp, accent_fg: Color) -> (String, bool) {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let rest = &value[i..];
        // `#` and its hex digits.
        if bytes[i] == b'#' {
            let len = 1 + rest[1..].bytes().take_while(u8::is_ascii_hexdigit).count();
            if let Some(c) = parse(&rest[..len]) {
                out.push_str(&ramp.map(c).hex());
                changed = true;
            } else {
                out.push_str(&rest[..len]);
            }
            i += len;
            continue;
        }
        // `rgb(r, g, b)` and `rgba(r, g, b, a)`, keeping the alpha.
        if (rest.starts_with("rgba(") || rest.starts_with("rgb("))
            && (i == 0 || !is_word(bytes[i - 1]))
            && let Some(close) = rest.find(')')
        {
            let open = rest.find('(').unwrap_or(0);
            let parts: Vec<&str> = rest[open + 1..close].split(',').map(str::trim).collect();
            let channel = |p: &str| p.parse::<u8>().ok();
            let alpha_ok =
                |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit() || b == b'.');
            if let (Some(r), Some(g), Some(b)) = (
                parts.first().and_then(|p| channel(p)),
                parts.get(1).and_then(|p| channel(p)),
                parts.get(2).and_then(|p| channel(p)),
            ) && (parts.len() == 3 || (parts.len() == 4 && alpha_ok(parts[3])))
            {
                let c = ramp.map(Color::rgb(r, g, b));
                let alpha = parts.get(3).copied().unwrap_or("1");
                let _ = write!(out, "rgba({}, {}, {}, {alpha})", c.r, c.g, c.b);
                changed = true;
                i += close + 1;
                continue;
            }
        }
        // A word: the accent's names, or a colour's.
        if is_word(bytes[i]) && (i == 0 || !is_word(bytes[i - 1])) {
            let len = rest.bytes().take_while(|b| is_word(*b)).count();
            let word = &rest[..len];
            let color = match word {
                "-st-accent-color" => Some(ramp.accent),
                "-st-accent-fg-color" => Some(accent_fg),
                _ => parse(word).map(|c| ramp.map(c)),
            };
            match color {
                Some(c) => {
                    out.push_str(&c.hex());
                    changed = true;
                }
                None => out.push_str(word),
            }
            i += len;
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, changed)
}

/// The colour of `property` in the rule for exactly `selector`.
fn stock_color(rules: &[(String, String)], selector: &str, property: &str) -> Option<Color> {
    rules
        .iter()
        .filter(|(s, _)| s == selector)
        .flat_map(|(_, body)| declarations(body))
        .rfind(|(p, _)| *p == property)
        .and_then(|(_, v)| parse(v))
}

/// The declarations of one rule as CSS reads them: where a property is
/// declared more than once, only the last counts, in its place. The
/// Shell's stylesheet does that on purpose, as in `background-color:
/// #222226; background-color: transparent;`.
fn declarations(body: &str) -> Vec<(&str, &str)> {
    let all: Vec<(&str, &str)> = body
        .split(';')
        .filter_map(|d| d.split_once(':'))
        .map(|(p, v)| (p.trim(), v.trim()))
        .collect();
    all.iter()
        .enumerate()
        .filter(|(i, (p, _))| !all[i + 1..].iter().any(|(q, _)| q == p))
        .map(|(_, d)| *d)
        .collect()
}

/// `.a:hover .b` as `.a:hover.a:hover .b`: one step heavier, for the
/// stylesheet of Velatura's own extension. A selector that starts with an
/// element's type cannot be doubled and is left as it is.
fn heavier(selector: &str) -> String {
    let (first, rest) = selector.split_once(' ').unwrap_or((selector, ""));
    let sep = if rest.is_empty() { "" } else { " " };
    if first.starts_with(['.', '#']) {
        format!("{first}{first}{sep}{rest}")
    } else {
        selector.to_owned()
    }
}

/// The colour declarations of `stock`, the Shell's own stylesheet for the
/// theme's appearance, restated in the theme's colours. With `heavy`, each
/// selector is made one step heavier where it can be. `None` when `stock`
/// is not a stylesheet this knows how to read.
pub fn restate(theme: &Resolved<'_>, stock: &str, heavy: bool) -> Option<String> {
    let rules = rules(stock)?;
    let back = match theme.colors.appearance {
        Appearance::Dark => "#overviewGroup",
        Appearance::Light => "#panel",
    };
    let ramp = Ramp::new(
        theme,
        stock_color(&rules, "stage", "color")?,
        stock_color(&rules, back, "background-color")?,
    )?;
    let accent_fg = theme.colors.ui.accent_fg;
    let mut out = String::new();
    out.push_str(
        "/* Generated by Velatura. Edits to this file are replaced on the next apply. */\n",
    );
    let _ = writeln!(
        out,
        "/* theme {} fingerprint {} format {}, from the Shell's stylesheet {} */",
        theme.label(),
        theme.fingerprint(),
        super::FORMAT,
        &sha256_hex(stock.as_bytes())[..16]
    );
    let mut kept = 0;
    for (selectors, body) in &rules {
        if selectors.starts_with('@') || selectors.is_empty() {
            continue;
        }
        let mut kept_here = Vec::new();
        for (property, value) in declarations(body) {
            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
            let (value, changed) = recolor(&value, &ramp, accent_fg);
            if changed {
                kept_here.push(format!("{property}: {value}"));
            }
        }
        if kept_here.is_empty() {
            continue;
        }
        let selectors = if heavy {
            selectors
                .split(',')
                .map(|s| heavier(s.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            selectors.clone()
        };
        let _ = writeln!(out, "{selectors} {{ {}; }}", kept_here.join("; "));
        kept += 1;
    }
    if kept == 0 {
        return None;
    }
    // The panel's text comes down from the stage in the Shell's own
    // stylesheet, so an extension that colours the panel's text would
    // win over it. Said outright, it is the panel's own.
    let panel = if heavy {
        heavier("#panel")
    } else {
        "#panel".to_owned()
    };
    let text = ramp.map(stock_color(&rules, "stage", "color")?);
    let _ = writeln!(out, "{panel} {{ color: {}; }}", text.hex());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    /// Rules as GNOME Shell 50's dark stylesheet writes them, and one as
    /// 47's light one does.
    const EXCERPT: &str = "/* stock */\nstage {\n  font-size: 1em;\n  color: #ffffff; }\n\n#overviewGroup {\n  background-color: #222226; }\n\n#panel {\n  background-color: #000000;\n  font-weight: bold;\n  height: 2.2em; }\n\n.quick-toggle:checked, .calendar .calendar-day.calendar-today {\n  color: -st-accent-fg-color;\n  background-color: -st-accent-color; }\n\n.popup-menu-item:hover {\n  background-color: st-lighten(#424247, 7%); }\n\nStScrollBar > .vhandle {\n  background-color: rgba(255, 255, 255, 0.5);\n  border-radius: 99px; }\n\n.icon-button:focus {\n  box-shadow: inset 0 0 0 2px st-transparentize(st-mix(-st-accent-color, #ffffff, 60%), 0.2) !important; }\n\n.message .message-header { color: whitesmoke; spacing: 6px; }\n\n.button.destructive-action { background-color: #c01c28; }\n\n.osd-window { padding: 12px; }\n";

    fn ember() -> crate::Theme {
        bundled::themes().remove(1)
    }

    #[test]
    fn restates_only_colours_and_keeps_the_shells_own_functions() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let ui = &r.colors.ui;
        let out = restate(&r, EXCERPT, false).unwrap();
        let lines: Vec<&str> = out.lines().skip(2).collect();
        // Every rule with a colour, and none without, then the panel's text.
        assert_eq!(lines.len(), 10, "{out}");
        assert_eq!(
            lines.last(),
            Some(&format!("#panel {{ color: {}; }}", ui.fg.hex()).as_str())
        );
        assert!(!out.contains("osd-window") && !out.contains("height") && !out.contains("1em"));
        assert!(lines.contains(&format!("stage {{ color: {}; }}", ui.fg.hex()).as_str()));
        assert!(lines.contains(
            &format!("#overviewGroup {{ background-color: {}; }}", ui.bg.hex()).as_str()
        ));
        assert!(
            lines
                .contains(&format!("#panel {{ background-color: {}; }}", ui.bg_dim.hex()).as_str())
        );
        assert!(lines.contains(
            &format!(
                ".quick-toggle:checked, .calendar .calendar-day.calendar-today {{ color: {}; background-color: {}; }}",
                ui.accent_fg.hex(),
                ui.accent.hex()
            )
            .as_str()
        ));
        assert!(out.contains(&format!(
            "box-shadow: inset 0 0 0 2px st-transparentize(st-mix({}, {}, 60%), 0.2) !important;",
            ui.accent.hex(),
            ui.fg.hex()
        )));
        assert!(out.contains(".popup-menu-item:hover { background-color: st-lighten(#"));
        assert!(out.contains(&format!(
            "StScrollBar > .vhandle {{ background-color: rgba({}, {}, {}, 0.5); }}",
            ui.fg.r, ui.fg.g, ui.fg.b
        )));
        assert!(out.contains(&format!("background-color: {}; }}", ui.error.hex())));
        // The Shell's own greys and accent are gone.
        for stock in ["#222226", "#424247", "#ffffff", "-st-accent", "whitesmoke"] {
            assert!(!out.contains(stock), "{stock} is still there");
        }
    }

    #[test]
    fn a_grey_between_the_shells_background_and_text_lands_between_the_themes() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let ui = &r.colors.ui;
        let ramp = Ramp::new(&r, Color::rgb(255, 255, 255), Color::rgb(0x22, 0x22, 0x26)).unwrap();
        let mid = ramp.map(Color::rgb(0x42, 0x42, 0x47));
        let l = |c: Color| c.to_oklab()[0];
        assert!(l(ui.bg) < l(mid) && l(mid) < l(ui.fg), "{mid:?}");
        // Lighter and darker stay in order.
        assert!(l(ramp.map(Color::rgb(0x36, 0x36, 0x3a))) < l(mid));
        assert_eq!(ramp.map(Color::rgb(0x22, 0x22, 0x26)), ui.bg);
        assert_eq!(ramp.map(Color::rgb(0, 0, 0)), ui.bg_dim);
        assert_eq!(ramp.map(Color::rgb(255, 255, 255)), ui.fg);
        // Colours of their own go to the theme's colours for the same job.
        assert_eq!(ramp.map(Color::rgb(0xc0, 0x1c, 0x28)), ui.error);
        assert_eq!(ramp.map(Color::rgb(0xcd, 0x93, 0x09)), ui.warning);
        assert_eq!(ramp.map(Color::rgb(0x2e, 0xc2, 0x7e)), ui.success);
        assert_eq!(ramp.map(Color::rgb(0x35, 0x84, 0xe4)), ui.accent);
    }

    #[test]
    fn a_light_theme_goes_on_the_light_stylesheet() {
        let mut theme = ember();
        let variant = theme.variants.values_mut().next().unwrap();
        variant.appearance = Appearance::Light;
        variant.ui.bg = Color::rgb(0xf4, 0xef, 0xe6);
        variant.ui.fg = Color::rgb(0x2b, 0x24, 0x20);
        let r = Resolved::new(&theme, None).unwrap();
        let light = "stage { color: #222226; }\n#panel { background-color: #fafafb; }\n.popup-menu-content { background-color: #ffffff; }\n#overviewGroup { background-color: #222226; }\n";
        let out = restate(&r, light, false).unwrap();
        assert!(
            out.contains("#panel { background-color: #f4efe6; }"),
            "{out}"
        );
        assert!(
            out.contains("#overviewGroup { background-color: #2b2420; }"),
            "{out}"
        );
        // Cards lighter than the Shell's background, lighter than the theme's.
        let card = out
            .lines()
            .find(|l| l.starts_with(".popup-menu-content"))
            .and_then(|l| parse(&l[l.find('#').unwrap()..l.find(';').unwrap()]))
            .unwrap();
        assert!(card.to_oklab()[0] > Color::rgb(0xf4, 0xef, 0xe6).to_oklab()[0]);
        // A dark theme on a light stylesheet is refused, not inverted.
        let dark = ember();
        assert!(restate(&Resolved::new(&dark, None).unwrap(), light, false).is_none());
    }

    #[test]
    fn what_is_not_a_compiled_stylesheet_is_refused() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        for bad in [
            "",
            "stage { color: #ffffff; }",
            "@media x { stage { color: #fff; } }\n#overviewGroup { background-color: #222; }",
            "stage { color: #ffffff; }\n#overviewGroup { background-color: #222226; ",
            "/* open",
        ] {
            assert!(restate(&r, bad, false).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn heavy_doubles_what_can_be_doubled() {
        assert_eq!(heavier("#panel"), "#panel#panel");
        assert_eq!(heavier(".a:hover .b"), ".a:hover.a:hover .b");
        assert_eq!(heavier("StScrollBar > .vhandle"), "StScrollBar > .vhandle");
        assert_eq!(heavier("stage"), "stage");
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let out = restate(&r, EXCERPT, true).unwrap();
        assert!(out.contains("\n#panel#panel { "));
        assert!(out.ends_with(&format!(
            "\n#panel#panel {{ color: {}; }}\n",
            r.colors.ui.fg.hex()
        )));
        assert!(out.contains(
            "\n.quick-toggle:checked.quick-toggle:checked, .calendar.calendar .calendar-day"
        ));
    }

    #[test]
    fn named_and_short_colours_are_read() {
        assert_eq!(parse("whitesmoke"), Some(Color::rgb(245, 245, 245)));
        assert_eq!(parse("#fff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(parse("#ffff"), None);
        assert_eq!(parse("transparent"), None);
        assert_eq!(parse("grayish"), None);
    }

    /// A property declared twice counts by its last declaration, as in the
    /// Shell's own tiles of the dash and app grid, which are transparent.
    #[test]
    fn a_property_declared_twice_counts_by_the_last() {
        let theme = ember();
        let r = Resolved::new(&theme, None).unwrap();
        let stock = "stage { color: #ffffff; }\n#overviewGroup { background-color: #222226; }\n\
            .overview-tile, .grid-search-result {\n  color: #fafafb;\n  background-color: #222226;\n  background-color: transparent;\n  border-radius: 24px; }\n\
            .later { background-color: transparent; background-color: #222226; }\n";
        let out = restate(&r, stock, false).unwrap();
        let tile = out
            .lines()
            .find(|l| l.starts_with(".overview-tile"))
            .unwrap();
        assert!(!tile.contains("background-color"), "{tile}");
        assert!(tile.contains("color: #"), "{tile}");
        assert!(
            out.contains(&format!(
                ".later {{ background-color: {}; }}",
                r.colors.ui.bg.hex()
            )),
            "{out}"
        );
    }
}
