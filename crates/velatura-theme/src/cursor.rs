//! Recolouring of cursor themes: the pictures in Xcursor files, moved to
//! a theme's colours.
//!
//! The greys of a cursor go along the line from the theme's darker colour
//! to its lighter one, of its background and its text: a black body
//! becomes the darker, a white outline the lighter, and the shading
//! between keeps its place. Body and outline then differ as much as the
//! theme's text and background do. Coloured parts, such as a busy
//! spinner, become shades of the accent, each keeping its lightness.
//!
//! An Xcursor file is a table of chunks, each picture one of them, its
//! pixels four bytes each with the colour multiplied by the alpha. Only
//! the pixels are changed, in place: every other byte of the file stays.
//! This module performs no I/O.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::{Color, Resolved};

/// Bumped whenever the colours chosen or the way they are moved changes.
pub const FORMAT: u32 = 1;

/// The largest side of one picture: cursors are at most a few hundred
/// pixels.
const MAX_SIDE: u32 = 1024;
const IMAGE: u32 = 0xfffd_0002;
/// Below this chroma (OKLCH) a colour counts as grey.
const GREY: f64 = 0.04;

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Where the pixels of each picture are in an Xcursor file. `None` if it
/// is not one, or is damaged.
fn pictures(bytes: &[u8]) -> Option<Vec<std::ops::Range<usize>>> {
    if bytes.get(..4)? != b"Xcur" {
        return None;
    }
    let header = u32_at(bytes, 4)? as usize;
    let count = u32_at(bytes, 12)? as usize;
    if count == 0 || count > 4096 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..count {
        let entry = header.checked_add(i * 12)?;
        if u32_at(bytes, entry)? != IMAGE {
            continue;
        }
        let at = u32_at(bytes, entry + 8)? as usize;
        if u32_at(bytes, at)? != 36 || u32_at(bytes, at + 4)? != IMAGE {
            return None;
        }
        let (w, h) = (u32_at(bytes, at + 16)?, u32_at(bytes, at + 20)?);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return None;
        }
        let start = at + 36;
        let end = start.checked_add(w as usize * h as usize * 4)?;
        bytes.get(start..end)?;
        out.push(start..end);
    }
    (!out.is_empty()).then_some(out)
}

/// A theme's colours for cursors.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorRecolor {
    dark: Color,
    light: Color,
    accent: Color,
}

impl CursorRecolor {
    pub fn new(theme: &Resolved<'_>) -> Self {
        let ui = &theme.colors.ui;
        let (dark, light) = if ui.bg.to_oklab()[0] <= ui.fg.to_oklab()[0] {
            (ui.bg, ui.fg)
        } else {
            (ui.fg, ui.bg)
        };
        Self {
            dark,
            light,
            accent: ui.accent,
        }
    }

    fn moved(&self, c: Color) -> Color {
        let [l, a, b] = c.to_oklab();
        let chroma = a.hypot(b);
        if chroma >= GREY {
            // The accent's hue at this colour's lightness, about as vivid
            // as the accent.
            let [_, aa, ab] = self.accent.to_oklab();
            let ac = aa.hypot(ab);
            return Color::from_oklch(l, chroma.min(ac).max(ac * 0.6), ab.atan2(aa).to_degrees());
        }
        let t = l.clamp(0.0, 1.0);
        if t < 0.001 {
            return self.dark;
        }
        if t > 0.999 {
            return self.light;
        }
        let (p, q) = (self.dark.to_oklab(), self.light.to_oklab());
        let mixed = [0, 1, 2].map(|k| p[k] + (q[k] - p[k]) * t);
        Color::from_oklab(mixed).unwrap_or_else(|| self.dark.mix(self.light, t))
    }

    /// `xcursor` with its pictures recoloured, or `None` if it is not an
    /// Xcursor file this reads.
    pub fn apply(&self, xcursor: &[u8]) -> Option<Vec<u8>> {
        let ranges = pictures(xcursor)?;
        let mut out = xcursor.to_vec();
        let mut seen: BTreeMap<Color, Color> = BTreeMap::new();
        for range in ranges {
            for px in out[range].as_chunks_mut::<4>().0 {
                // Blue, green, red, alpha; the colour times the alpha.
                let alpha = px[3];
                if alpha == 0 {
                    continue;
                }
                let plain = |v: u8| {
                    ((u16::from(v) * 255 + u16::from(alpha) / 2) / u16::from(alpha)).min(255) as u8
                };
                let c = Color::rgb(plain(px[2]), plain(px[1]), plain(px[0]));
                let to = *seen.entry(c).or_insert_with(|| self.moved(c));
                let times = |v: u8| ((u16::from(v) * u16::from(alpha) + 127) / 255) as u8;
                px[0] = times(to.b);
                px[1] = times(to.g);
                px[2] = times(to.r);
            }
        }
        Some(out)
    }

    /// What decides the result, as text.
    pub fn recipe(&self) -> String {
        let mut out = format!("format {FORMAT} cursors\n");
        let _ = writeln!(
            out,
            "{} {} {}",
            self.dark.hex(),
            self.light.hex(),
            self.accent.hex()
        );
        out
    }
}

/// True if `xcursor` is an Xcursor file this reads.
pub fn is_xcursor(xcursor: &[u8]) -> bool {
    pictures(xcursor).is_some()
}

/// The `index.theme` of a recoloured cursor set: hidden, and naming the
/// cursor theme it was made from for every cursor it does not hold.
pub fn index_theme(base: &str) -> String {
    format!(
        "[Icon Theme]\nName=Velatura cursors\nComment=Recolored by Velatura\nInherits={base}\nHidden=true\nX-Velatura-Base={base}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    /// An Xcursor file with one picture of `pixels` (blue, green, red,
    /// alpha each, premultiplied), as libXcursor writes them.
    pub(crate) fn xcursor(w: u32, h: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"Xcur");
        for v in [16u32, 0x1_0000, 1] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [IMAGE, 24, 28] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [36u32, IMAGE, 24, 1, w, h, 1, 1, 50] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for p in pixels {
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn body_and_outline_take_the_themes_dark_and_light_and_colour_the_accent() {
        let themes = bundled::themes();
        let r = Resolved::new(&themes[1], None).unwrap();
        let ui = &r.colors.ui;
        let rec = CursorRecolor::new(&r);
        // Black, white, half-transparent white, transparent, a red.
        let file = xcursor(
            5,
            1,
            &[
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [128, 128, 128, 128],
                [0, 0, 0, 0],
                [0, 0, 255, 255],
            ],
        );
        let out = rec.apply(&file).unwrap();
        assert_eq!(out.len(), file.len());
        assert_eq!(out[..64], file[..64], "everything but the pixels stays");
        let px = |i: usize| {
            let p = &out[64 + i * 4..64 + i * 4 + 4];
            (Color::rgb(p[2], p[1], p[0]), p[3])
        };
        assert_eq!(px(0), (ui.bg, 255));
        assert_eq!(px(1), (ui.fg, 255));
        // Premultiplied again: half of the theme's text colour.
        let (half, alpha) = px(2);
        assert_eq!(alpha, 128);
        assert!((i16::from(half.r) - i16::from(ui.fg.r) / 2).abs() <= 1);
        assert_eq!(px(3), (Color::rgb(0, 0, 0), 0));
        // The red is a shade of the accent: the accent's hue.
        let (red, _) = px(4);
        let hue = |c: Color| {
            let [_, a, b] = c.to_oklab();
            b.atan2(a).to_degrees()
        };
        assert!((hue(red) - hue(ui.accent)).abs() < 8.0, "{}", red.hex());
    }

    #[test]
    fn what_is_not_an_xcursor_file_is_refused() {
        let good = xcursor(2, 2, &[[0, 0, 0, 255]; 4]);
        assert!(is_xcursor(&good));
        for cut in [0, 4, 16, 40, good.len() - 1] {
            assert!(!is_xcursor(&good[..cut]), "cut at {cut}");
        }
        let mut huge = good.clone();
        huge[44..48].copy_from_slice(&5000u32.to_le_bytes());
        assert!(!is_xcursor(&huge));
        assert!(!is_xcursor(b"\x89PNG"));
    }

    #[test]
    fn a_light_theme_turns_the_body_light() {
        let mut theme = bundled::themes().remove(1);
        let v = theme.variants.values_mut().next().unwrap();
        std::mem::swap(&mut v.ui.bg, &mut v.ui.fg);
        let r = Resolved::new(&theme, None).unwrap();
        let rec = CursorRecolor::new(&r);
        // Still dark on light: the darker of the two is the body.
        assert_eq!(rec.dark, r.colors.ui.fg);
        assert!(
            index_theme("capitaine-cursors").contains("Inherits=capitaine-cursors\nHidden=true\n")
        );
    }
}
