use std::fmt;

/// An sRGB colour with 8 bits per channel.
///
/// The only way to build one from text is [`Color::parse`], which accepts
/// exactly `#RRGGBB` and `#RRGGBBAA`. The only ways to turn one back into text
/// are the formatters below, none of which can emit anything but hex digits,
/// decimal digits and fixed punctuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseColorError {
    /// Does not start with `#`.
    MissingHash,
    /// Not 6 or 8 digits after the `#`.
    BadLength,
    /// Contains something that is not an ASCII hex digit.
    BadDigit,
}

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingHash => "color must start with '#'",
            Self::BadLength => "color must be #RRGGBB or #RRGGBBAA",
            Self::BadDigit => "color contains a character that is not a hex digit",
        })
    }
}

impl std::error::Error for ParseColorError {}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parse `#RRGGBB` or `#RRGGBBAA`. No whitespace, no short form, no names.
    pub fn parse(s: &str) -> Result<Self, ParseColorError> {
        let digits = s.strip_prefix('#').ok_or(ParseColorError::MissingHash)?;
        let bytes = digits.as_bytes();
        if bytes.len() != 6 && bytes.len() != 8 {
            return Err(ParseColorError::BadLength);
        }
        let mut ch = [255u8; 4];
        for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
            ch[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self {
            r: ch[0],
            g: ch[1],
            b: ch[2],
            a: ch[3],
        })
    }

    pub fn is_opaque(self) -> bool {
        self.a == 255
    }

    /// `#rrggbb`, dropping any transparency.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// `rrggbb`, for formats that want no prefix.
    pub fn hex_noprefix(self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// `rgb(r, g, b)`.
    pub fn css_rgb(self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }

    /// `rgba(r, g, b, a)` with alpha as a decimal between 0 and 1.
    pub fn css_rgba(self) -> String {
        let alpha = (f64::from(self.a) / 255.0 * 1000.0).round() / 1000.0;
        format!("rgba({}, {}, {}, {})", self.r, self.g, self.b, alpha)
    }

    /// Blend towards `other` by `t` (0 keeps `self`, 1 gives `other`), channel
    /// by channel in sRGB. Alpha is taken from `self`.
    pub fn mix(self, other: Self, t: f64) -> Self {
        let ch = |a: u8, b: u8| {
            let v = f64::from(a) + (f64::from(b) - f64::from(a)) * t.clamp(0.0, 1.0);
            v.round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: ch(self.r, other.r),
            g: ch(self.g, other.g),
            b: ch(self.b, other.b),
            a: self.a,
        }
    }

    /// Convert to OKLab (Björn Ottosson, 2020). Alpha is ignored.
    pub fn to_oklab(self) -> [f64; 3] {
        let [r, g, b] = [self.r, self.g, self.b].map(srgb_to_linear);
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        [
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        ]
    }

    /// From OKLab. `None` if the colour cannot be shown in sRGB.
    pub fn from_oklab([l, a, b]: [f64; 3]) -> Option<Self> {
        let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
        let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
        let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
        let linear = [
            4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
            -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
            -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_,
        ];
        let mut out = [0u8; 3];
        for (slot, v) in out.iter_mut().zip(linear) {
            if !(-0.0005..=1.0005).contains(&v) {
                return None;
            }
            let v = v.clamp(0.0, 1.0);
            let c = if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            *slot = (c * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        Some(Self::rgb(out[0], out[1], out[2]))
    }

    /// The colour of lightness `l` (0 to 1) and hue `h` (degrees) in OKLCH
    /// with as much of chroma `c` as sRGB can show.
    pub fn from_oklch(l: f64, c: f64, h: f64) -> Self {
        let (sin, cos) = h.to_radians().sin_cos();
        let l = l.clamp(0.0, 1.0);
        let mut chroma = c.max(0.0);
        loop {
            if let Some(color) = Self::from_oklab([l, chroma * cos, chroma * sin]) {
                return color;
            }
            if chroma < 0.002 {
                let v = (l * 255.0).round() as u8;
                return Self::rgb(v, v, v);
            }
            chroma *= 0.94;
        }
    }

    /// The same hue and chroma at another lightness, as far as sRGB allows.
    pub fn with_lightness(self, l: f64) -> Self {
        let [_, a, b] = self.to_oklab();
        Self::from_oklch(l, a.hypot(b), b.atan2(a).to_degrees())
    }

    /// Relative luminance as defined by WCAG 2.
    pub fn luminance(self) -> f64 {
        let [r, g, b] = [self.r, self.g, self.b].map(srgb_to_linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// WCAG 2 contrast ratio between two opaque colours, from 1 to 21.
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

fn nibble(c: u8) -> Result<u8, ParseColorError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(ParseColorError::BadDigit),
    }
}

fn srgb_to_linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_and_eight_digits() {
        assert_eq!(Color::parse("#141826"), Ok(Color::rgb(0x14, 0x18, 0x26)));
        assert_eq!(
            Color::parse("#D3D9EE80"),
            Ok(Color {
                r: 0xd3,
                g: 0xd9,
                b: 0xee,
                a: 0x80
            })
        );
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "#",
            "141826",
            "#1418",
            "#14182",
            "#1418266",
            "#141826000",
            " #141826",
            "#141826 ",
        ] {
            assert!(Color::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(Color::parse("#14182g"), Err(ParseColorError::BadDigit));
        assert_eq!(Color::parse("#14 826"), Err(ParseColorError::BadDigit));
        assert_eq!(Color::parse("#1418\n6"), Err(ParseColorError::BadDigit));
        // multi-byte input must not panic on a slice boundary
        assert!(Color::parse("#é4182").is_err());
        assert!(Color::parse("#１４１８２６").is_err());
    }

    #[test]
    fn formatters_emit_only_safe_characters() {
        // Every byte value in every channel: output stays inside a fixed alphabet.
        for v in 0..=255u8 {
            let c = Color {
                r: v,
                g: v.wrapping_mul(7),
                b: v.wrapping_add(13),
                a: v,
            };
            for out in [c.hex(), c.hex_noprefix(), c.css_rgb(), c.css_rgba()] {
                assert!(
                    out.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"#(), .".contains(&b)),
                    "unexpected character in {out:?}"
                );
            }
        }
    }

    #[test]
    fn hex_round_trips() {
        let c = Color::parse("#8493D8").unwrap();
        assert_eq!(c.hex(), "#8493d8");
        assert_eq!(Color::parse(&c.hex()), Ok(c));
        assert_eq!(c.hex_noprefix(), "8493d8");
        assert_eq!(c.css_rgb(), "rgb(132, 147, 216)");
        assert_eq!(
            Color::parse("#00000080").unwrap().css_rgba(),
            "rgba(0, 0, 0, 0.502)"
        );
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let (black, white) = (Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
        assert!((black.contrast(white) - 21.0).abs() < 1e-9);
        assert!((white.contrast(white) - 1.0).abs() < 1e-9);
        // #767676 on white is the well-known 4.54:1 boundary grey.
        let grey = Color::rgb(0x76, 0x76, 0x76);
        assert!((grey.contrast(white) - 4.54).abs() < 0.01);
    }

    #[test]
    fn oklab_reference_points() {
        let [l, a, b] = Color::rgb(255, 255, 255).to_oklab();
        assert!((l - 1.0).abs() < 1e-3 && a.abs() < 1e-3 && b.abs() < 1e-3);
        let [l, ..] = Color::rgb(0, 0, 0).to_oklab();
        assert!(l.abs() < 1e-9);
        // Pure sRGB red, from the OKLab reference implementation.
        let [l, a, b] = Color::rgb(255, 0, 0).to_oklab();
        assert!((l - 0.628).abs() < 1e-3 && (a - 0.225).abs() < 1e-3 && (b - 0.126).abs() < 1e-3);
    }
}
