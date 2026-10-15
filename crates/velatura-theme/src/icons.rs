//! Recolouring of vector icon themes: which colours an icon theme uses
//! for what stands out, and the same icons with the theme's accent in
//! their place.
//!
//! An icon theme that draws its folders in a colour of its own (the
//! purple of Tela purple, the blue of Adwaita) uses that colour, and a few
//! lighter and darker shades of it, wherever it marks something. Those
//! shades are found in its folder icons and each is moved to the theme's
//! accent, keeping how much lighter or darker than the main one it was, so
//! the shading of every icon stays. Nothing else in an icon is touched.
//!
//! This module performs no I/O: the engine reads the icon theme and hands
//! its files here.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::Color;

/// Bumped whenever the colours chosen or the way they are moved changes.
pub const FORMAT: u32 = 1;

/// Below this chroma (OKLCH) a colour counts as grey and is never moved.
const GREY: f64 = 0.06;
/// How far in hue from the main colour a shade may be, in degrees.
const SAME_HUE: f64 = 25.0;

fn lch(c: Color) -> (f64, f64, f64) {
    let [l, a, b] = c.to_oklab();
    (l, a.hypot(b), b.atan2(a).to_degrees())
}

fn hue_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Every `#rrggbb` and `#rgb` colour in `text`, with where it stands.
fn colors(text: &[u8]) -> Vec<(usize, usize, Color)> {
    let hex = |b: &u8| b.is_ascii_hexdigit();
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'#' {
            let run = text[i + 1..].iter().take_while(|b| hex(b)).count();
            let digits = &text[i + 1..i + 1 + run];
            let full = match run {
                6 => Some(digits.to_vec()),
                3 => Some(digits.iter().flat_map(|d| [*d, *d]).collect()),
                _ => None,
            };
            if let Some(full) = full
                && let Ok(text) = std::str::from_utf8(&full)
                && let Ok(color) = Color::parse(&format!("#{text}"))
            {
                out.push((i, i + 1 + run, color));
            }
            i += 1 + run;
        } else {
            i += 1;
        }
    }
    out
}

/// The colours an icon theme marks things with, and where each goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recolor {
    /// The main colour of the family, as found.
    pub main: Color,
    /// Every shade of the family, and the colour it becomes.
    pub moves: BTreeMap<Color, Color>,
}

impl Recolor {
    /// The family of `folders`, the icon theme's folder icons, moved to
    /// `accent`. `None` when they have no colour of their own: a theme that
    /// is all grey, or one whose folders are pictures, not drawings.
    pub fn new(folders: &[&[u8]], accent: Color) -> Option<Self> {
        let mut count: BTreeMap<Color, usize> = BTreeMap::new();
        for svg in folders {
            for (_, _, c) in colors(svg) {
                if lch(c).1 >= GREY {
                    *count.entry(c).or_default() += 1;
                }
            }
        }
        // The most used; between two used as often, the more colourful.
        let main = *count
            .iter()
            .max_by(|a, b| {
                a.1.cmp(b.1)
                    .then(lch(*a.0).1.total_cmp(&lch(*b.0).1))
                    .then(b.0.cmp(a.0))
            })?
            .0;
        let (ml, mc, mh) = lch(main);
        let (al, ac, ah) = lch(accent);
        let moves = count
            .keys()
            .filter(|c| hue_gap(lch(**c).2, mh) <= SAME_HUE)
            .map(|c| {
                let (l, ch, h) = lch(*c);
                let to = Color::from_oklch(
                    (al + (l - ml)).clamp(0.05, 0.98),
                    (ch * ac / mc).min(0.37),
                    ah + (h - mh),
                );
                (*c, to)
            })
            .collect();
        Some(Self { main, moves })
    }

    /// `svg` with the family's shades moved, or `None` if it has none of
    /// them. Every other byte stays as it was.
    pub fn apply(&self, svg: &[u8]) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(svg.len());
        let mut at = 0;
        let mut moved = false;
        for (start, end, c) in colors(svg) {
            if let Some(to) = self.moves.get(&c) {
                out.extend_from_slice(&svg[at..start]);
                out.extend_from_slice(to.hex().as_bytes());
                at = end;
                moved = true;
            }
        }
        if !moved {
            return None;
        }
        out.extend_from_slice(&svg[at..]);
        Some(out)
    }

    /// What decides the result, as text: the format and every move.
    pub fn recipe(&self) -> String {
        let mut out = format!("format {FORMAT}\n");
        for (from, to) in &self.moves {
            let _ = writeln!(out, "{} {}", from.hex(), to.hex());
        }
        out
    }
}

/// One folder of an icon theme, as its `index.theme` describes it. Only
/// keys whose values are numbers or known words are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub size: u32,
    pub scale: Option<u32>,
    pub context: String,
    pub kind: Option<&'static str>,
    pub min: Option<u32>,
    pub max: Option<u32>,
    pub threshold: Option<u32>,
}

impl Folder {
    /// Folders whose icons are not recoloured: those of apps, whose colours
    /// are their makers', and symbolic ones, which follow the text colour.
    pub fn recolored(&self) -> bool {
        !matches!(
            self.context.as_str(),
            "Applications" | "Apps" | "Animations"
        ) && !self.name.contains("symbolic")
    }
}

/// What an icon theme's `index.theme` says that recolouring needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub folders: Vec<Folder>,
}

/// A folder name as icon themes use them: `scalable/places`, `16@2x/apps`.
fn plain_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
        })
}

/// A name an icon theme can have and Velatura will write into a file: no
/// separators, no list commas, nothing that is not printable.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " ._+-".contains(c))
}

/// Read the parts of an `index.theme` that recolouring needs.
pub fn parse_index(text: &str) -> Index {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.to_owned());
            sections.entry(name.to_owned()).or_default();
        } else if let (Some(section), Some((key, value))) = (&current, line.split_once('=')) {
            sections
                .entry(section.clone())
                .or_default()
                .entry(key.trim().to_owned())
                .or_insert_with(|| value.trim().to_owned());
        }
    }
    let head = sections.get("Icon Theme").cloned().unwrap_or_default();
    let listed = ["Directories", "ScaledDirectories"]
        .iter()
        .filter_map(|key| head.get(*key))
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|name| plain_folder(name))
        .map(str::to_owned);
    let mut seen = std::collections::BTreeSet::new();
    let mut folders = Vec::new();
    for name in listed {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(keys) = sections.get(&name) else {
            continue;
        };
        let number = |key: &str| keys.get(key).and_then(|v| v.parse::<u32>().ok());
        let Some(size) = number("Size") else {
            continue;
        };
        let context = keys
            .get("Context")
            .filter(|c| c.len() <= 32 && c.bytes().all(|b| b.is_ascii_alphabetic()))
            .cloned()
            .unwrap_or_default();
        folders.push(Folder {
            name,
            size,
            scale: number("Scale"),
            kind: match keys.get("Type").map(String::as_str) {
                Some("Fixed") => Some("Fixed"),
                Some("Scalable") => Some("Scalable"),
                Some("Threshold") => Some("Threshold"),
                _ => None,
            },
            min: number("MinSize"),
            max: number("MaxSize"),
            threshold: number("Threshold"),
            context,
        });
    }
    Index { folders }
}

/// The `index.theme` of a recoloured set: the folders it has, and the icon
/// theme it was made from as the one to look in for everything else.
/// Hidden, so lists of icon themes to choose from do not show it.
pub fn index_theme(base: &str, folders: &[&Folder]) -> String {
    let mut out = String::from(
        "# Generated by Velatura from another icon theme. Edits are lost on the next apply.\n",
    );
    out.push_str("[Icon Theme]\n");
    let _ = writeln!(out, "Name=Velatura ({base})");
    let _ = writeln!(
        out,
        "Comment=The icons of {base} in the colors of the theme in use"
    );
    let _ = writeln!(out, "Inherits={base}");
    out.push_str("Hidden=true\n");
    let _ = writeln!(out, "X-Velatura-Base={base}");
    let _ = writeln!(out, "X-Velatura-Format={FORMAT}");
    let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
    let _ = writeln!(out, "Directories={}", names.join(","));
    for f in folders {
        let _ = write!(out, "\n[{}]\nSize={}\n", f.name, f.size);
        for (key, value) in [
            ("Scale", f.scale),
            ("MinSize", f.min),
            ("MaxSize", f.max),
            ("Threshold", f.threshold),
        ] {
            if let Some(v) = value {
                let _ = writeln!(out, "{key}={v}");
            }
        }
        if !f.context.is_empty() {
            let _ = writeln!(out, "Context={}", f.context);
        }
        if let Some(kind) = f.kind {
            let _ = writeln!(out, "Type={kind}");
        }
    }
    out
}

/// The same for icons that are pictures. A picture's colours are not a
/// few written values but every pixel, smoothed at the edges, so the
/// family is a hue: the one most of the folders' coloured pixels share.
/// Each pixel near that hue moves to the accent the way a shade does in a
/// drawing; one near the edge of the family moves part of the way, so
/// edges stay smooth.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelRecolor {
    /// Lightness, chroma and hue of the family's main colour.
    main: (f64, f64, f64),
    accent: (f64, f64, f64),
}

/// How much of the way a value between `from` and `to` is, eased.
fn smoothstep(from: f64, to: f64, x: f64) -> f64 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl PixelRecolor {
    /// The family of `folders`, the icon theme's folder pictures, moved to
    /// `accent`. `None` when too few of their pixels are coloured to say.
    pub fn new(folders: &[&crate::png::Image], accent: Color) -> Option<Self> {
        // Coloured pixels, by hue in steps of ten degrees.
        let mut bins = [0usize; 36];
        let mut colored = Vec::new();
        let mut opaque = 0usize;
        for image in folders {
            for px in image.rgba.as_chunks::<4>().0 {
                if px[3] < 128 {
                    continue;
                }
                opaque += 1;
                let (l, ch, h) = lch(Color::rgb(px[0], px[1], px[2]));
                if ch >= GREY {
                    bins[(h.rem_euclid(360.0) / 10.0) as usize % 36] += 1;
                    colored.push((l, ch, h));
                }
            }
        }
        let (peak, count) = bins.iter().enumerate().max_by_key(|(_, n)| **n)?;
        if *count == 0 || colored.len() * 20 < opaque {
            return None;
        }
        let centre = peak as f64 * 10.0 + 5.0;
        let family: Vec<&(f64, f64, f64)> = colored
            .iter()
            .filter(|(_, _, h)| hue_gap(*h, centre) <= SAME_HUE)
            .collect();
        // The colour the folders are filled with: the most used one, as in
        // a drawing, where one is used for a good part of them; otherwise,
        // in a picture that is all gradient, the family's average.
        let mut uses: BTreeMap<Color, usize> = BTreeMap::new();
        for image in folders {
            for px in image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] >= 128)
            {
                let c = Color::rgb(px[0], px[1], px[2]);
                let (_, ch, h) = lch(c);
                if ch >= GREY && hue_gap(h, centre) <= SAME_HUE {
                    *uses.entry(c).or_default() += 1;
                }
            }
        }
        if let Some((fill, n)) = uses.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            && *n * 10 >= family.len()
        {
            return Some(Self {
                main: lch(*fill),
                accent: lch(accent),
            });
        }
        let n = family.len() as f64;
        let l = family.iter().map(|p| p.0).sum::<f64>() / n;
        let ch = family.iter().map(|p| p.1).sum::<f64>() / n;
        let (sin, cos) = family.iter().fold((0.0, 0.0), |(s, c), p| {
            let (ps, pc) = p.2.to_radians().sin_cos();
            (s + ps, c + pc)
        });
        let h = sin.atan2(cos).to_degrees();
        Some(Self {
            main: (l, ch, h),
            accent: lch(accent),
        })
    }

    /// Where one pixel's colour goes: `None` if it is not of the family.
    fn moved(&self, c: Color) -> Option<Color> {
        let (l, ch, h) = lch(c);
        let (ml, mc, mh) = self.main;
        let (al, ac, ah) = self.accent;
        let gap = hue_gap(h, mh);
        let weight =
            smoothstep(GREY * 0.5, GREY, ch) * (1.0 - smoothstep(SAME_HUE, SAME_HUE + 10.0, gap));
        if weight <= 0.0 {
            return None;
        }
        let to = Color::from_oklch(
            (al + (l - ml)).clamp(0.05, 0.98),
            (ch * ac / mc).min(0.37),
            ah + (h - mh),
        );
        // Part of the family even where it lands on itself, as when the
        // accent is the icon theme's own colour.
        Some(c.mix(to, weight))
    }

    /// Move the family's pixels in `image`. False if it has none.
    /// Transparency stays as it was.
    pub fn apply(&self, image: &mut crate::png::Image) -> bool {
        let mut seen: BTreeMap<Color, Option<Color>> = BTreeMap::new();
        let mut moved = false;
        for px in image.rgba.as_chunks_mut::<4>().0 {
            if px[3] == 0 {
                continue;
            }
            let c = Color::rgb(px[0], px[1], px[2]);
            let to = *seen.entry(c).or_insert_with(|| self.moved(c));
            if let Some(to) = to {
                px[..3].copy_from_slice(&[to.r, to.g, to.b]);
                moved = true;
            }
        }
        moved
    }

    /// What decides the result, as text.
    pub fn recipe(&self) -> String {
        let r = |v: f64| (v * 1000.0).round() / 1000.0;
        format!(
            "format {FORMAT} pictures\nmain {} {} {}\naccent {} {} {}\n",
            r(self.main.0),
            r(self.main.1),
            r(self.main.2),
            r(self.accent.0),
            r(self.accent.1),
            r(self.accent.2)
        )
    }
}

/// The icon theme a set was made from, from the set's `index.theme`.
pub fn base_of(index: &str) -> Option<String> {
    index
        .lines()
        .find_map(|l| l.trim().strip_prefix("X-Velatura-Base="))
        .map(str::trim)
        .filter(|name| plain_name(name))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TELA_FOLDER: &[u8] = br##"<svg><style>.ColorScheme-Highlight { color:#7e57c2; }</style>
<path class="ColorScheme-Highlight" fill="#7e57c2"/><path fill="#000000" opacity=".2"/><path fill="#ffffff"/>
<rect fill="#7E57C2"/><path fill="#5294e2"/><path fill="#727272"/></svg>"##;
    const ADWAITA_FOLDER: &[u8] =
        br##"<svg><stop stop-color="#62a0ea"/><stop stop-color="#afd4ff"/>
<stop stop-color="#62a0ea"/><stop stop-color="#c0d5ea"/><path fill="#438de6"/><path fill="#62a0ea"/>
<path fill="#a4caee"/><path fill="#62a0ea"/></svg>"##;

    fn c(hex: &str) -> Color {
        Color::parse(hex).unwrap()
    }

    #[test]
    fn the_main_colour_and_its_shades_are_found_in_the_folders() {
        let tela = Recolor::new(&[TELA_FOLDER], c("#e0914f")).unwrap();
        assert_eq!(tela.main, c("#7e57c2"));
        // The blue of Tela's other folders is another hue; greys are greys.
        assert_eq!(
            tela.moves.keys().copied().collect::<Vec<_>>(),
            [c("#7e57c2")]
        );
        let adwaita = Recolor::new(&[ADWAITA_FOLDER], c("#e0914f")).unwrap();
        assert_eq!(adwaita.main, c("#62a0ea"));
        let shades: Vec<Color> = adwaita.moves.keys().copied().collect();
        assert!(shades.contains(&c("#438de6")) && shades.contains(&c("#afd4ff")));
        assert!(!shades.contains(&c("#c0d5ea")), "too grey to be part of it");
        assert!(Recolor::new(&[b"<svg fill=\"#333333\"/>"], c("#e0914f")).is_none());
        assert!(Recolor::new(&[], c("#e0914f")).is_none());
    }

    #[test]
    fn the_main_colour_becomes_the_accent_and_shades_keep_their_place() {
        let accent = c("#e0914f");
        let r = Recolor::new(&[ADWAITA_FOLDER], accent).unwrap();
        assert_eq!(r.moves[&c("#62a0ea")], accent);
        let lighter = r.moves[&c("#afd4ff")];
        let darker = r.moves[&c("#438de6")];
        assert!(lch(lighter).0 > lch(accent).0 && lch(darker).0 < lch(accent).0);
        for to in [lighter, darker] {
            assert!(hue_gap(lch(to).2, lch(accent).2) < 25.0, "{}", to.hex());
        }
    }

    #[test]
    fn only_the_familys_colours_are_replaced_byte_for_byte() {
        let r = Recolor::new(&[TELA_FOLDER], c("#e0914f")).unwrap();
        let out = String::from_utf8(r.apply(TELA_FOLDER).unwrap()).unwrap();
        let before = String::from_utf8_lossy(TELA_FOLDER);
        assert_eq!(
            out.matches("#e0914f").count(),
            3,
            "upper and lower case alike"
        );
        assert_eq!(
            out.replace("#e0914f", "#7e57c2"),
            before.replace("#7E57C2", "#7e57c2")
        );
        assert!(r.apply(b"<svg fill=\"#5294e2\"/>").is_none());
        // Short forms, and runs that are not colours, are read rightly.
        let short = Recolor::new(&[b"<svg fill=\"#f00\" stroke=\"#f00\"/>"], c("#00ff00")).unwrap();
        assert_eq!(
            short
                .apply(b"<a fill=\"#f00\"/><b id=\"#f00ba\"/>")
                .unwrap(),
            b"<a fill=\"#00ff00\"/><b id=\"#f00ba\"/>"
        );
    }

    /// A folder picture: a body in one colour, darker and lighter bands
    /// of it, a grey label, and soft edges.
    fn folder_picture(main: Color) -> crate::png::Image {
        let mut rgba = Vec::new();
        for y in 0..32u32 {
            for x in 0..32u32 {
                let c = match (x, y) {
                    (0 | 31, _) | (_, 0 | 31) => Color { a: 0, ..main },
                    (_, 1..=4) => main.mix(c("#000000"), 0.3),
                    (_, 27..=30) => main.mix(c("#ffffff"), 0.4),
                    (10..=20, 12..=18) => c("#e0e0e0"),
                    _ => main,
                };
                rgba.extend_from_slice(&[c.r, c.g, c.b, c.a]);
            }
        }
        crate::png::Image {
            width: 32,
            height: 32,
            rgba,
        }
    }

    #[test]
    fn pictures_move_their_family_to_the_accent_and_keep_the_rest() {
        let periwinkle = c("#8493d8");
        let accent = c("#e0914f");
        let folder = folder_picture(periwinkle);
        let r = PixelRecolor::new(&[&folder], accent).unwrap();
        let mut moved = folder.clone();
        assert!(r.apply(&mut moved));
        let at = |img: &crate::png::Image, x: usize, y: usize| {
            let i = (y * 32 + x) * 4;
            Color::rgb(img.rgba[i], img.rgba[i + 1], img.rgba[i + 2])
        };
        // The body is the accent, give or take rounding.
        let body = at(&moved, 5, 8);
        assert!(body.contrast(accent) < 1.05, "{}", body.hex());
        // The darker band is darker than the body, the lighter one lighter.
        assert!(lch(at(&moved, 5, 2)).0 < lch(body).0);
        assert!(lch(at(&moved, 5, 28)).0 > lch(body).0);
        // The grey label and the transparent edge are as they were.
        assert_eq!(at(&moved, 15, 15), c("#e0e0e0"));
        assert_eq!(moved.rgba[3], 0);
        assert_eq!(moved.rgba.len(), folder.rgba.len());
        // A picture with nothing of the family is left alone.
        let mut grey = folder_picture(c("#808080"));
        assert!(!r.apply(&mut grey));
        // Folders that are all grey have no family to find.
        assert!(PixelRecolor::new(&[&folder_picture(c("#808080"))], accent).is_none());
    }

    #[test]
    fn an_index_is_read_only_for_what_is_plain_and_written_back() {
        let index = parse_index(
            "[Icon Theme]\nName=Tela\nInherits=hicolor\nDirectories=16/places,scalable/apps,../../etc,16/places,symbolic/places\n\
             ScaledDirectories=16@2x/places\n\n[16/places]\nSize=16\nContext=Places\nType=Fixed\n\
             [scalable/apps]\nSize=64\nMinSize=16\nMaxSize=512\nContext=Applications\nType=Scalable\n\
             [symbolic/places]\nSize=16\nContext=Places\nType=Scalable\n[16@2x/places]\nSize=16\nScale=2\nContext=Places\nType=Odd\n[../../etc]\nSize=1\n",
        );
        let names: Vec<&str> = index.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "16/places",
                "scalable/apps",
                "symbolic/places",
                "16@2x/places"
            ]
        );
        let kept: Vec<&Folder> = index.folders.iter().filter(|f| f.recolored()).collect();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].kind, None, "an unknown type is not written");
        let text = index_theme("Tela-purple", &kept);
        assert!(text.contains("Inherits=Tela-purple\nHidden=true\n"));
        assert!(text.contains("Directories=16/places,16@2x/places\n"));
        assert!(text.contains("[16@2x/places]\nSize=16\nScale=2\nContext=Places\n"));
        assert_eq!(base_of(&text).as_deref(), Some("Tela-purple"));
        assert_eq!(parse_index(&text).folders.len(), 2);
        assert!(plain_name("Tela purple") && !plain_name("a,b") && !plain_name("../x"));
    }
}
