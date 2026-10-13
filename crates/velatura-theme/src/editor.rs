//! What the theme editor does to a theme, as plain functions: start one
//! from three colours, keep the worked-out colours in step, name what is
//! hard to read, and fix it.
//!
//! No I/O and no widgets, so all of it is tested here.

use std::collections::BTreeMap;

use crate::model::{Appearance, CONTRAST_FLOOR, DERIVE, UI_OPTIONAL, is_slug};
use crate::{Color, Theme, Variant};

/// Hues of the six terminal colours in OKLCH, in the order terminals
/// number them: red, green, yellow, blue, magenta, cyan.
const HUES: [f64; 6] = [25.0, 145.0, 95.0, 255.0, 325.0, 200.0];

/// Sixteen terminal colours to go with a background and a text colour:
/// six hues at a lightness that reads on the background, and neutrals
/// taken from the two.
pub fn terminal_colors(appearance: Appearance, bg: Color, fg: Color) -> [Color; 16] {
    let black = Color::rgb(0, 0, 0);
    let (normal, bright, neutrals) = match appearance {
        Appearance::Dark => (
            0.72,
            0.82,
            [bg.mix(black, 0.3), fg.mix(bg, 0.15), bg.mix(fg, 0.4), fg],
        ),
        Appearance::Light => (
            0.50,
            0.42,
            [fg, bg.mix(fg, 0.3), fg.mix(bg, 0.35), bg.mix(fg, 0.12)],
        ),
    };
    let mut out = [black; 16];
    out[0] = neutrals[0];
    out[7] = neutrals[1];
    out[8] = neutrals[2];
    out[15] = neutrals[3];
    for (i, hue) in HUES.iter().enumerate() {
        out[1 + i] = readable(Color::from_oklch(normal, 0.13, *hue), bg, 3.0);
        out[9 + i] = readable(Color::from_oklch(bright, 0.13, *hue), bg, 3.0);
    }
    out
}

/// `color`, moved away from `on` in lightness until the two have a
/// contrast of `need`, or as far as it goes.
pub fn readable(color: Color, on: Color, need: f64) -> Color {
    if color.contrast(on) >= need {
        return color;
    }
    let lighter = on.luminance() < 0.18;
    let mut l = color.to_oklab()[0];
    let mut best = color;
    for _ in 0..100 {
        l += if lighter { 0.01 } else { -0.01 };
        if !(0.0..=1.0).contains(&l) {
            break;
        }
        best = color.with_lightness(l);
        if best.contrast(on) >= need {
            return best;
        }
    }
    // Nothing with this hue gets there: the plain end of the scale does.
    let end = if lighter {
        Color::rgb(255, 255, 255)
    } else {
        Color::rgb(0, 0, 0)
    };
    if end.contrast(on) > best.contrast(on) {
        end
    } else {
        best
    }
}

/// A variant from the three colours a person picks first. Everything
/// else is worked out and marked as such.
pub fn new_variant(
    name: &str,
    appearance: Appearance,
    bg: Color,
    fg: Color,
    accent: Color,
) -> Variant {
    let ansi = terminal_colors(appearance, bg, fg);
    let mut v = Variant {
        name: name.to_owned(),
        appearance,
        palette: BTreeMap::new(),
        ui: blank(bg),
        derived: UI_OPTIONAL.to_vec(),
        ansi,
        syntax: BTreeMap::new(),
        diff: BTreeMap::new(),
        diagnostic: BTreeMap::new(),
        slots: BTreeMap::new(),
        suggests: BTreeMap::new(),
    };
    v.ui.bg = bg;
    v.ui.fg = fg;
    v.ui.accent = accent;
    v.rederive();
    v
}

fn blank(c: Color) -> crate::Ui {
    crate::Ui {
        bg: c,
        fg: c,
        accent: c,
        bg_dim: c,
        surface: c,
        border: c,
        fg_muted: c,
        fg_subtle: c,
        accent_fg: c,
        selection_bg: c,
        selection_fg: c,
        cursor: c,
        cursor_fg: c,
        link: c,
        success: c,
        warning: c,
        error: c,
    }
}

/// A theme being edited: one variant of it, and which of its colours
/// follow the others.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub theme: Theme,
    /// The variant being edited.
    pub variant: String,
    /// True while the terminal colours follow background and text.
    pub auto_terminal: bool,
}

impl Draft {
    /// A new theme from three colours.
    pub fn new(id: &str, name: &str, appearance: Appearance, colors: [Color; 3]) -> Self {
        let [bg, fg, accent] = colors;
        let mut variants = BTreeMap::new();
        variants.insert(
            "default".to_owned(),
            new_variant(name, appearance, bg, fg, accent),
        );
        Self {
            theme: Theme {
                id: id.to_owned(),
                name: name.to_owned(),
                author: String::new(),
                license: String::new(),
                description: String::new(),
                derive: DERIVE,
                variants,
            },
            variant: "default".to_owned(),
            auto_terminal: true,
        }
    }

    /// Open a variant of an existing theme. A colour that is exactly what
    /// would be worked out counts as automatic, so a file with everything
    /// written out still follows its three main colours when edited.
    pub fn open(theme: &Theme, variant: &str) -> Option<Self> {
        let mut theme = theme.clone();
        let v = theme.variants.get_mut(variant)?;
        let auto_terminal = v.ansi == terminal_colors(v.appearance, v.ui.bg, v.ui.fg);
        let auto = v.automatic();
        v.derived = UI_OPTIONAL
            .iter()
            .copied()
            .filter(|role| v.ui.get(role) == auto.get(role))
            .collect();
        Some(Self {
            theme,
            variant: variant.to_owned(),
            auto_terminal,
        })
    }

    pub fn colors(&self) -> &Variant {
        &self.theme.variants[&self.variant]
    }

    fn colors_mut(&mut self) -> &mut Variant {
        self.theme
            .variants
            .get_mut(&self.variant)
            .expect("a draft always holds its variant")
    }

    /// After any change: bring the colours that follow others up to date.
    fn settle(&mut self) {
        let auto = self.auto_terminal;
        let v = self.colors_mut();
        if auto {
            v.ansi = terminal_colors(v.appearance, v.ui.bg, v.ui.fg);
        }
        v.rederive();
    }

    pub fn set_name(&mut self, name: &str) {
        let name = crate::model::clean_text(name, 64);
        if self.theme.variants.len() == 1 {
            self.theme.name.clone_from(&name);
        }
        self.colors_mut().name = name;
    }

    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.colors_mut().appearance = appearance;
        self.settle();
    }

    /// Give an interface role a colour of its own. For background, text
    /// and accent this is the only kind of colour there is.
    pub fn set_role(&mut self, role: &str, color: Color) {
        let v = self.colors_mut();
        if v.ui.set(role, color) {
            v.derived.retain(|r| *r != role);
        }
        self.settle();
    }

    /// Let an interface role follow the others again.
    pub fn reset_role(&mut self, role: &str) {
        let v = self.colors_mut();
        if let Some(known) = UI_OPTIONAL.iter().find(|r| **r == role)
            && !v.derived.contains(known)
        {
            v.derived.push(known);
        }
        self.settle();
    }

    pub fn is_automatic(&self, role: &str) -> bool {
        self.colors().derived.contains(&role)
    }

    pub fn set_terminal(&mut self, index: usize, color: Color) {
        if index < 16 {
            self.auto_terminal = false;
            self.colors_mut().ansi[index] = color;
            self.settle();
        }
    }

    pub fn reset_terminal(&mut self) {
        self.auto_terminal = true;
        self.settle();
    }

    /// A code colour, or `None` to leave it unset.
    pub fn set_syntax(&mut self, role: &str, color: Option<Color>) {
        let Some(known) = crate::model::SYNTAX_ROLES.iter().find(|r| **r == role) else {
            return;
        };
        match color {
            Some(color) => self.colors_mut().syntax.insert(known, color),
            None => self.colors_mut().syntax.remove(known),
        };
    }

    /// Change the colour a problem is about so that it reads.
    pub fn fix(&mut self, problem: &Problem) {
        let v = self.colors();
        let (Some(color), Some(on)) = (v.ui.get(problem.role), v.ui.get(problem.on)) else {
            return;
        };
        self.set_role(problem.role, readable(color, on, problem.need + 0.05));
    }
}

/// Two colours that are hard to tell apart where one is drawn on the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Problem {
    /// The role that is drawn.
    pub role: &'static str,
    /// The role it is drawn on.
    pub on: &'static str,
    pub ratio: f64,
    pub need: f64,
}

/// The pairs that have to read, and how well.
const PAIRS: [(&str, &str, f64); 4] = [
    ("fg", "bg", CONTRAST_FLOOR),
    ("fg_muted", "bg", 3.0),
    ("accent_fg", "accent", 3.0),
    ("selection_fg", "selection_bg", 3.0),
];

pub fn problems(v: &Variant) -> Vec<Problem> {
    PAIRS
        .iter()
        .filter_map(|(role, on, need)| {
            let ratio = v.ui.get(role)?.contrast(v.ui.get(on)?);
            (ratio < *need).then_some(Problem {
                role,
                on,
                ratio,
                need: *need,
            })
        })
        .collect()
}

/// A theme id made from a name: lower case, digits and hyphens. Empty if
/// the name has nothing that can go into one.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(48).collect();
    let out = out.trim_end_matches('-').to_owned();
    if is_slug(&out) { out } else { String::new() }
}

/// `wanted`, or `wanted-2`, `wanted-3` and so on: the first that `taken`
/// does not hold. An empty `wanted` becomes `theme`.
pub fn free_id(wanted: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = if wanted.is_empty() { "theme" } else { wanted };
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{parse_theme, to_toml};
    use crate::{Resolved, bundled};

    fn c(hex: &str) -> Color {
        Color::parse(hex).unwrap()
    }

    #[test]
    fn oklab_goes_there_and_back() {
        for hex in [
            "#000000", "#ffffff", "#8493d8", "#191316", "#e66100", "#26a269",
        ] {
            let color = c(hex);
            assert_eq!(Color::from_oklab(color.to_oklab()), Some(color), "{hex}");
        }
        // More chroma than the screen has is cut down, not wrapped round.
        let vivid = Color::from_oklch(0.7, 0.9, 145.0);
        assert!(vivid.g > vivid.r && vivid.g > vivid.b, "{vivid:?}");
        assert_eq!(Color::from_oklch(1.0, 0.0, 0.0), c("#ffffff"));
    }

    #[test]
    fn a_theme_from_three_colors_is_complete_and_readable() {
        for (appearance, bg, fg, accent) in [
            (Appearance::Dark, "#191316", "#dbc7b0", "#e0864a"),
            (Appearance::Light, "#fafafa", "#202020", "#3584e4"),
            (Appearance::Dark, "#000000", "#ffffff", "#ff00ff"),
            (Appearance::Light, "#fdf6e3", "#586e75", "#268bd2"),
        ] {
            let draft = Draft::new("mine", "Mine", appearance, [c(bg), c(fg), c(accent)]);
            let v = draft.colors();
            assert_eq!(v.derived.len(), UI_OPTIONAL.len());
            for (i, color) in v
                .ansi
                .iter()
                .enumerate()
                .filter(|(i, _)| ![0, 7, 8, 15].contains(i))
            {
                assert!(
                    color.contrast(c(bg)) >= 3.0,
                    "{bg} terminal color {i}: {color:?}"
                );
            }
            // The six hues are six different colours.
            let mut hues: Vec<String> = v.ansi[1..7].iter().map(|c| c.hex()).collect();
            hues.dedup();
            assert_eq!(hues.len(), 6);
            // What is written is a valid theme that reads back the same.
            let text = to_toml(&draft.theme);
            let loaded = parse_theme(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
            assert!(
                loaded.warnings.is_empty() || fg == "#586e75",
                "{:?}",
                loaded.warnings
            );
            assert_eq!(loaded.theme.variants["default"].ui, v.ui);
            assert_eq!(loaded.theme.variants["default"].ansi, v.ansi);
            assert!(Resolved::new(&loaded.theme, None).is_some());
        }
    }

    #[test]
    fn every_bundled_theme_survives_being_written_out() {
        for theme in bundled::themes() {
            let text = to_toml(&theme);
            let mut back = parse_theme(&text).unwrap().theme;
            let mut want = theme.clone();
            // Written out in full, nothing is left to derivation.
            for v in want.variants.values_mut().chain(back.variants.values_mut()) {
                v.derived.clear();
            }
            assert_eq!(back, want);
            assert!(!text.contains('@'), "no references in a written theme");
            // And writing that again changes nothing.
            assert_eq!(to_toml(&back), text);
        }
    }

    #[test]
    fn text_in_a_written_theme_cannot_break_out_of_its_string() {
        let mut draft = Draft::new(
            "mine",
            "x",
            Appearance::Dark,
            [c("#101010"), c("#f0f0f0"), c("#8080ff")],
        );
        let hostile = "a\"\nid = \"evil\"\n[variants.x]\n# \\";
        draft.theme.author = hostile.to_owned();
        draft.theme.description = "back\\slash \"quoted\" ünïcode".to_owned();
        draft.set_name(hostile);
        let loaded = parse_theme(&to_toml(&draft.theme)).unwrap();
        assert_eq!(loaded.theme.id, "mine");
        assert_eq!(loaded.theme.variants.len(), 1);
        assert_eq!(loaded.theme.description, "back\\slash \"quoted\" ünïcode");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    }

    #[test]
    fn worked_out_colors_follow_until_one_is_made_custom() {
        let mut d = Draft::new(
            "mine",
            "Mine",
            Appearance::Dark,
            [c("#101010"), c("#f0f0f0"), c("#8080ff")],
        );
        let surface = d.colors().ui.surface;
        let red = d.colors().ansi[1];
        d.set_role("bg", c("#202830"));
        assert_ne!(d.colors().ui.surface, surface, "follows the background");
        assert_eq!(d.colors().ui.cursor, c("#f0f0f0"));

        d.set_role("surface", c("#334455"));
        assert!(!d.is_automatic("surface") && d.is_automatic("border"));
        d.set_role("bg", c("#000000"));
        assert_eq!(d.colors().ui.surface, c("#334455"), "a custom color stays");
        d.reset_role("surface");
        assert_eq!(d.colors().ui.surface, c("#000000").mix(c("#f0f0f0"), 0.08));
        d.reset_role("bg");
        d.reset_role("no-such-role");
        assert!(!d.is_automatic("bg"));

        // Terminal colours follow too, and error follows terminal red.
        d.set_appearance(Appearance::Light);
        d.set_role("bg", c("#ffffff"));
        d.set_role("fg", c("#111111"));
        assert_ne!(d.colors().ansi[1], red);
        assert_eq!(d.colors().ui.error, d.colors().ansi[1]);
        d.set_terminal(1, c("#aa0000"));
        assert!(!d.auto_terminal);
        d.set_role("bg", c("#eeeeee"));
        assert_eq!(d.colors().ansi[1], c("#aa0000"));
        assert_eq!(d.colors().ui.error, c("#aa0000"));
        d.reset_terminal();
        assert_ne!(d.colors().ansi[1], c("#aa0000"));

        d.set_syntax("keyword", Some(c("#a08cd0")));
        d.set_syntax("nonsense", Some(c("#a08cd0")));
        assert_eq!(d.colors().syntax.len(), 1);
        d.set_syntax("keyword", None);
        assert!(d.colors().syntax.is_empty());
    }

    #[test]
    fn opening_a_saved_theme_knows_which_colors_were_automatic() {
        let mut d = Draft::new(
            "mine",
            "Mine",
            Appearance::Dark,
            [c("#101010"), c("#f0f0f0"), c("#8080ff")],
        );
        d.set_role("border", c("#445566"));
        let saved = parse_theme(&to_toml(&d.theme)).unwrap().theme;
        assert!(
            saved.variants["default"].derived.is_empty(),
            "all written out"
        );
        let again = Draft::open(&saved, "default").unwrap();
        assert!(again.auto_terminal);
        assert!(again.is_automatic("surface") && !again.is_automatic("border"));
        assert_eq!(again.colors().ui, d.colors().ui);
        assert!(Draft::open(&saved, "missing").is_none());
        // A bundled theme has its own terminal colours.
        let ember = &bundled::themes()[1];
        assert!(!Draft::open(ember, "default").unwrap().auto_terminal);
    }

    #[test]
    fn hard_to_read_pairs_are_named_and_fixed() {
        let mut d = Draft::new(
            "mine",
            "Mine",
            Appearance::Dark,
            [c("#202020"), c("#606060"), c("#303030")],
        );
        let found = problems(d.colors());
        assert_eq!(found[0].role, "fg");
        assert!(found[0].ratio < 4.5 && found.len() >= 2, "{found:?}");
        for _ in 0..6 {
            let Some(p) = problems(d.colors()).first().copied() else {
                break;
            };
            d.fix(&p);
        }
        assert!(
            problems(d.colors()).is_empty(),
            "{:?}",
            problems(d.colors())
        );
        assert!(d.colors().ui.fg.contrast(d.colors().ui.bg) >= 4.5);
        // A light theme is fixed by going darker, and keeps its hue.
        let mut l = Draft::new(
            "l",
            "L",
            Appearance::Light,
            [c("#ffffff"), c("#9ab0ff"), c("#3584e4")],
        );
        let p = problems(l.colors())[0];
        l.fix(&p);
        let fg = l.colors().ui.fg;
        assert!(fg.contrast(c("#ffffff")) >= 4.5 && fg.b > fg.r, "{fg:?}");
        for theme in bundled::themes() {
            for v in theme.variants.values() {
                assert!(problems(v).is_empty(), "{}: {:?}", theme.id, problems(v));
            }
        }
    }

    #[test]
    fn ids_are_made_from_names() {
        assert_eq!(slug("My Theme"), "my-theme");
        assert_eq!(slug("  Été — 2026! "), "t-2026");
        assert_eq!(slug("日本語"), "");
        assert_eq!(slug("-a--b-"), "a-b");
        assert!(slug(&"x".repeat(200)).len() <= 48);
        let taken = |id: &str| ["ember", "ember-2", "theme"].contains(&id);
        assert_eq!(free_id("ember", &taken), "ember-3");
        assert_eq!(free_id("mine", &taken), "mine");
        assert_eq!(free_id("", &taken), "theme-2");
    }
}
