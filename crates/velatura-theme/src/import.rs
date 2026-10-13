//! Themes from other formats: base16 and base24 schemes, and Kitty themes.
//!
//! What is read is untrusted, as a theme file is. Only colours and three
//! short texts (name, author, licence) are taken from it; the result is
//! written out as a theme file and goes through the one validator, so an
//! imported theme can hold nothing a theme file could not.

use std::collections::BTreeMap;

use crate::editor::{new_variant, slug};
use crate::model::{DERIVE, Loaded, Theme, Variant, clean_text, parse_theme, to_toml};
use crate::{Appearance, Color};

/// A format other than Velatura's own that can be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Base16,
    Base24,
    Kitty,
}

/// The file is in a known format but cannot become a theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incomplete {
    pub format: Format,
    /// The colours it has to give and does not, by their names in the file.
    pub missing: Vec<String>,
}

/// A colour as these formats write one: six hex digits, with or without `#`.
fn color(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    (digits.len() == 6)
        .then(|| Color::parse(&format!("#{digits}")).ok())
        .flatten()
}

fn appearance_of(bg: Color, fg: Color) -> Appearance {
    if bg.luminance() > fg.luminance() {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

/// Give `role` a colour of the file's own, so it is no longer worked out.
fn set(v: &mut Variant, role: &str, color: Option<Color>) {
    if let Some(color) = color
        && v.ui.set(role, color)
    {
        v.derived.retain(|r| *r != role);
    }
}

struct Meta {
    name: String,
    author: String,
    license: String,
}

/// The theme as a checked theme file would give it. `stem` is the name of
/// the file without its ending, used where the file names nothing.
fn finish(meta: Meta, stem: &str, variant: Variant) -> Option<Loaded> {
    let name = match clean_text(&meta.name, 64) {
        name if name.trim().is_empty() => clean_text(stem, 64),
        name => name,
    };
    let id = [slug(&name), slug(stem), "imported".to_owned()]
        .into_iter()
        .find(|id| !id.is_empty())?;
    let theme = Theme {
        id,
        name: if name.trim().is_empty() {
            "Imported".to_owned()
        } else {
            name
        },
        author: clean_text(&meta.author, 64),
        license: clean_text(&meta.license, 64),
        description: String::new(),
        derive: DERIVE,
        variants: BTreeMap::from([("default".to_owned(), variant)]),
    };
    parse_theme(&to_toml(&theme)).ok()
}

/// `key: value` pairs of a flat YAML file, nesting ignored: a scheme's keys
/// are all different. Values lose their quotes and a trailing comment.
fn yaml_pairs(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        let value = value.trim();
        let value = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next().unwrap_or(""),
            _ => value.split(" #").next().unwrap_or("").trim(),
        };
        if !key.is_empty() && !key.starts_with('#') {
            out.entry(key.to_owned())
                .or_insert_with(|| value.to_owned());
        }
    }
    out
}

const BASE16: [&str; 16] = [
    "base00", "base01", "base02", "base03", "base04", "base05", "base06", "base07", "base08",
    "base09", "base0A", "base0B", "base0C", "base0D", "base0E", "base0F",
];
const BASE24: [&str; 8] = [
    "base10", "base11", "base12", "base13", "base14", "base15", "base16", "base17",
];

/// A base16 scheme, or a base24 one when it has the eight further colours:
/// the sixteen-colour styling the format's guidelines describe.
fn base16(text: &str, stem: &str) -> Option<Result<(Loaded, Format), Incomplete>> {
    let pairs = yaml_pairs(text);
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| color(value))
    };
    if !BASE16.iter().any(|name| get(name).is_some()) {
        return None;
    }
    let missing: Vec<String> = BASE16
        .iter()
        .filter(|name| get(name).is_none())
        .map(|name| (*name).to_owned())
        .collect();
    if !missing.is_empty() {
        return Some(Err(Incomplete {
            format: Format::Base16,
            missing,
        }));
    }
    let b: Vec<Color> = BASE16.iter().filter_map(|name| get(name)).collect();
    let more: Vec<Option<Color>> = BASE24.iter().map(|name| get(name)).collect();
    let format = if more.iter().all(Option::is_some) {
        Format::Base24
    } else {
        Format::Base16
    };
    let (bg, fg, accent) = (b[0], b[5], b[13]);
    let appearance = match pairs.get("variant").map(String::as_str) {
        Some("light") => Appearance::Light,
        Some("dark") => Appearance::Dark,
        _ => appearance_of(bg, fg),
    };
    let text_of = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| pairs.get(*key))
            .cloned()
            .unwrap_or_default()
    };
    let mut v = new_variant("", appearance, bg, fg, accent);
    // Terminal colours as base16's own terminal templates number them.
    v.ansi = [
        b[0], b[8], b[11], b[10], b[13], b[14], b[12], b[5], b[3], b[8], b[11], b[10], b[13],
        b[14], b[12], b[7],
    ];
    if format == Format::Base24 {
        // Bright red, green, yellow, blue, magenta and cyan of their own.
        for (slot, from) in [(9, 2), (10, 4), (11, 3), (12, 6), (13, 7), (14, 5)] {
            if let Some(color) = more[from] {
                v.ansi[slot] = color;
            }
        }
    }
    v.rederive();
    for (role, color) in [
        ("surface", Some(b[1])),
        ("selection_bg", Some(b[2])),
        ("selection_fg", Some(b[5])),
        ("fg_subtle", Some(b[3])),
        ("fg_muted", Some(b[4])),
        ("bg_dim", more[0].filter(|_| format == Format::Base24)),
    ] {
        set(&mut v, role, color);
    }
    for (role, from) in [
        ("comment", 3),
        ("keyword", 14),
        ("string", 11),
        ("number", 9),
        ("constant", 9),
        ("function", 13),
        ("type", 10),
        ("variable", 8),
        ("operator", 5),
        ("punctuation", 5),
        ("tag", 8),
        ("attribute", 10),
        ("builtin", 12),
        ("escape", 12),
    ] {
        if let Some(known) = crate::model::SYNTAX_ROLES.iter().find(|r| **r == role) {
            v.syntax.insert(known, b[from]);
        }
    }
    // The scheme's own colours under their own names, so none is lost.
    for (name, color) in BASE16.iter().zip(&b) {
        v.palette.insert(name.to_ascii_lowercase(), *color);
    }
    for (name, color) in BASE24.iter().zip(&more) {
        if let Some(color) = color.filter(|_| format == Format::Base24) {
            v.palette.insert((*name).to_owned(), color);
        }
    }
    let meta = Meta {
        name: text_of(&["name", "scheme"]),
        author: text_of(&["author"]),
        license: String::new(),
    };
    v.name = clean_text(&meta.name, 64);
    Some(
        finish(meta, stem, v)
            .map(|loaded| (loaded, format))
            .ok_or(Incomplete {
                format,
                missing: Vec::new(),
            }),
    )
}

/// A Kitty theme: `key value` lines, and what its header comments say of
/// it (`## name: …`). Values that are not colours (`none`, `background`)
/// are passed over.
fn kitty(text: &str, stem: &str) -> Option<Result<(Loaded, Format), Incomplete>> {
    let mut colors: BTreeMap<&str, Color> = BTreeMap::new();
    let mut about: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix('#') {
            if let Some((key, value)) = comment.trim_start_matches('#').split_once(':') {
                about
                    .entry(key.trim().to_ascii_lowercase())
                    .or_insert_with(|| value.trim().to_owned());
            }
            continue;
        }
        let mut words = line.split_whitespace();
        if let (Some(key), Some(value)) = (words.next(), words.next())
            && let Some(color) = color(value).filter(|_| value.starts_with('#'))
        {
            colors.insert(key, color);
        }
    }
    let ansi_names: Vec<String> = (0..16).map(|i| format!("color{i}")).collect();
    let have = ansi_names
        .iter()
        .filter(|n| colors.contains_key(n.as_str()))
        .count();
    if !colors.contains_key("background") && !colors.contains_key("foreground") && have < 8 {
        return None;
    }
    let missing: Vec<String> = ["background", "foreground"]
        .into_iter()
        .map(str::to_owned)
        .chain(ansi_names.iter().cloned())
        .filter(|name| !colors.contains_key(name.as_str()))
        .collect();
    if !missing.is_empty() {
        // Two colours and nothing else is any settings file, not a theme.
        if have == 0 {
            return None;
        }
        return Some(Err(Incomplete {
            format: Format::Kitty,
            missing,
        }));
    }
    let get = |name: &str| colors.get(name).copied();
    let (bg, fg) = (get("background")?, get("foreground")?);
    // Kitty has no accent; the border of the active window is the nearest
    // thing a theme says, and blue is what terminals fall back on.
    let accent = get("active_border_color")
        .filter(|c| *c != bg && *c != fg)
        .or(get("color4"))?;
    let mut v = new_variant("", appearance_of(bg, fg), bg, fg, accent);
    for (slot, name) in v.ansi.iter_mut().zip(&ansi_names) {
        *slot = get(name)?;
    }
    v.rederive();
    for (role, from) in [
        ("selection_bg", "selection_background"),
        ("selection_fg", "selection_foreground"),
        ("cursor", "cursor"),
        ("cursor_fg", "cursor_text_color"),
        ("link", "url_color"),
        ("border", "inactive_border_color"),
    ] {
        set(&mut v, role, get(from));
    }
    let said = |key: &str| about.get(key).cloned().unwrap_or_default();
    let meta = Meta {
        name: said("name"),
        author: said("author"),
        license: said("license"),
    };
    v.name = clean_text(&meta.name, 64);
    Some(
        finish(meta, stem, v)
            .map(|loaded| (loaded, Format::Kitty))
            .ok_or(Incomplete {
                format: Format::Kitty,
                missing: Vec::new(),
            }),
    )
}

/// Read `text` as a theme in another format. `None` when it is in none of
/// the formats known here.
pub fn read(text: &str, stem: &str) -> Option<Result<(Loaded, Format), Incomplete>> {
    base16(text, stem).or_else(|| kitty(text, stem))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Resolved, bundled, kitty as kitty_out};

    const OCEAN: &str = include_str!("../tests/fixtures/base16-ocean.yaml");
    const OCEAN_NEW: &str = include_str!("../tests/fixtures/base16-ocean-palette.yaml");
    const ONE_LIGHT: &str = include_str!("../tests/fixtures/base24-one-light.yaml");
    const DRACULA: &str = include_str!("../tests/fixtures/kitty-dracula.conf");

    fn hex(c: Color) -> String {
        c.hex()
    }

    #[test]
    fn a_base16_scheme_in_either_layout_becomes_the_same_theme() {
        let (old, format) = read(OCEAN, "ocean").unwrap().unwrap();
        assert_eq!(format, Format::Base16);
        let (new, _) = read(OCEAN_NEW, "whatever").unwrap().unwrap();
        assert_eq!(old.theme, new.theme);
        assert!(old.warnings.is_empty(), "{:?}", old.warnings);
        let t = &old.theme;
        assert_eq!((t.id.as_str(), t.name.as_str()), ("ocean", "Ocean"));
        assert_eq!(t.author, "Chris Kempson (http://chriskempson.com)");
        let v = &t.variants["default"];
        assert_eq!(v.appearance, Appearance::Dark);
        assert_eq!(
            [hex(v.ui.bg), hex(v.ui.fg), hex(v.ui.accent)],
            ["#2b303b", "#c0c5ce", "#8fa1b3"]
        );
        assert_eq!(hex(v.ui.surface), "#343d46");
        assert_eq!(hex(v.ui.error), "#bf616a");
        assert_eq!(hex(v.ansi[2]), "#a3be8c");
        assert_eq!(hex(v.ansi[15]), "#eff1f5");
        assert_eq!(hex(v.syntax["keyword"]), "#b48ead");
        assert_eq!(v.palette.len(), 16);
        assert_eq!(hex(v.palette["base0a"]), "#ebcb8b");
        // What the scheme does not give is what the rule works out, so
        // the editor takes it for automatic.
        assert_eq!(v.ui.border, v.automatic().border);
        assert_ne!(v.ui.surface, v.automatic().surface);
    }

    #[test]
    fn a_base24_scheme_brings_its_bright_colours_and_says_light() {
        let (loaded, format) = read(ONE_LIGHT, "one-light").unwrap().unwrap();
        assert_eq!(format, Format::Base24);
        let v = &loaded.theme.variants["default"];
        assert_eq!(v.appearance, Appearance::Light);
        assert_eq!(hex(v.ansi[1]), "#e05661");
        assert_eq!(hex(v.ansi[9]), "#ff616e");
        assert_eq!(hex(v.ui.bg_dim), "#e7e7e9");
        assert_eq!(v.palette.len(), 24);
    }

    #[test]
    fn a_kitty_theme_becomes_a_theme_and_goes_back_out_the_same() {
        let (loaded, format) = read(DRACULA, "dracula").unwrap().unwrap();
        assert_eq!(format, Format::Kitty);
        let t = &loaded.theme;
        assert_eq!((t.id.as_str(), t.name.as_str()), ("dracula", "Dracula"));
        assert_eq!(t.license, "MIT");
        let v = &t.variants["default"];
        assert_eq!(
            [hex(v.ui.bg), hex(v.ui.fg), hex(v.ui.selection_bg)],
            ["#282a36", "#f8f8f2", "#44475a"]
        );
        // `selection_foreground none` is not a colour: worked out.
        assert_eq!(v.ui.selection_fg, v.ui.fg);
        // Written back for Kitty, every colour the file gave is the same.
        let out = kitty_out::render(&Resolved::new(t, None).unwrap());
        for line in DRACULA.lines() {
            let mut words = line.split_whitespace();
            if let (Some(key), Some(value)) = (words.next(), words.next())
                && (key.starts_with("color") || key == "background" || key == "foreground")
            {
                assert!(
                    out.contains(&format!("{key} {}\n", value.to_lowercase())),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn velaturas_own_kitty_file_reads_back_with_the_same_terminal_colours() {
        for theme in bundled::themes() {
            let resolved = Resolved::new(&theme, None).unwrap();
            let (loaded, _) = read(&kitty_out::render(&resolved), "x").unwrap().unwrap();
            let v = &loaded.theme.variants["default"];
            assert_eq!(v.ansi, resolved.colors.ansi);
            assert_eq!(
                (v.ui.bg, v.ui.fg),
                (resolved.colors.ui.bg, resolved.colors.ui.fg)
            );
            assert_eq!(v.ui.cursor, resolved.colors.ui.cursor);
        }
    }

    #[test]
    fn what_is_missing_is_named_and_other_files_are_not_themes() {
        let short = OCEAN
            .replace("base0A", "base0a_gone")
            .replace("base03", "x");
        assert_eq!(
            read(&short, "ocean").unwrap().unwrap_err(),
            Incomplete {
                format: Format::Base16,
                missing: vec!["base03".to_owned(), "base0A".to_owned()],
            }
        );
        let short: String = DRACULA
            .lines()
            .filter(|l| !l.starts_with("color1 "))
            .map(|l| format!("{l}\n"))
            .collect();
        let why = read(&short, "d").unwrap().unwrap_err();
        assert_eq!(
            (why.format, why.missing),
            (Format::Kitty, vec!["color1".to_owned()])
        );
        for other in [
            "",
            "#!/bin/sh\nrm -rf ~\n",
            "font_size 12\nbackground_opacity 0.9\n",
            "background #000000\nforeground #ffffff\n",
            "name: x\nauthor: y\n",
            "schema = 1\n[meta]\nid = \"x\"\n",
        ] {
            assert!(read(other, "x").is_none(), "{other:?}");
        }
    }

    /// Text from the file reaches the theme only as cleaned, capped names,
    /// and whatever it says the theme still passes the validator.
    #[test]
    fn hostile_text_in_a_scheme_stays_text() {
        let hostile = "x\\\"\\n[variants.evil]\u{202e}\u{1b}]52;c;AAAA\u{7}".repeat(20);
        let src = OCEAN.replace("\"Ocean\"", &format!("\"{hostile}\""));
        let (loaded, _) = read(&src, "../../etc/passwd").unwrap().unwrap();
        let t = &loaded.theme;
        assert_eq!(t.variants.len(), 1);
        assert!(t.name.chars().count() <= 64 && !t.name.contains('\u{202e}'));
        assert!(crate::model::is_slug(&t.id), "{}", t.id);
        let src = OCEAN.replace("scheme: \"Ocean\"\n", "");
        let (loaded, _) = read(&src, "../../etc/passwd").unwrap().unwrap();
        assert_eq!(loaded.theme.id, "etc-passwd");
    }
}
