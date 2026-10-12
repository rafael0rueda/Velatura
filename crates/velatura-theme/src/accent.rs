use crate::Color;

/// The nine accent colours GNOME offers in `org.gnome.desktop.interface
/// accent-color` (GNOME 47 and later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    Blue,
    Teal,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Slate,
}

impl Accent {
    pub const ALL: [Accent; 9] = [
        Accent::Blue,
        Accent::Teal,
        Accent::Green,
        Accent::Yellow,
        Accent::Orange,
        Accent::Red,
        Accent::Pink,
        Accent::Purple,
        Accent::Slate,
    ];

    /// The value GSettings expects.
    pub const fn setting_value(self) -> &'static str {
        match self {
            Accent::Blue => "blue",
            Accent::Teal => "teal",
            Accent::Green => "green",
            Accent::Yellow => "yellow",
            Accent::Orange => "orange",
            Accent::Red => "red",
            Accent::Pink => "pink",
            Accent::Purple => "purple",
            Accent::Slate => "slate",
        }
    }

    /// The colour libadwaita draws for this accent (`--accent-*` in 1.6 to 1.9).
    pub const fn color(self) -> Color {
        match self {
            Accent::Blue => Color::rgb(0x35, 0x84, 0xe4),
            Accent::Teal => Color::rgb(0x21, 0x90, 0xa4),
            Accent::Green => Color::rgb(0x3a, 0x94, 0x4a),
            Accent::Yellow => Color::rgb(0xc8, 0x88, 0x00),
            Accent::Orange => Color::rgb(0xed, 0x5b, 0x00),
            Accent::Red => Color::rgb(0xe6, 0x2d, 0x42),
            Accent::Pink => Color::rgb(0xd5, 0x61, 0x99),
            Accent::Purple => Color::rgb(0x91, 0x41, 0xac),
            Accent::Slate => Color::rgb(0x6f, 0x83, 0x96),
        }
    }
}

/// Below this OKLab chroma a colour reads as grey and maps to slate. Slate's
/// own chroma is about 0.037.
const GREY_CHROMA: f64 = 0.05;

/// The system accent that looks closest to `color`.
///
/// A near-grey colour maps to slate. Any other colour maps to the chromatic
/// accent with the nearest OKLab hue. Lightness and saturation are ignored on
/// purpose: a theme accent is usually a paler or darker shade of the hue the
/// author means, and comparing full colour distance sends every pastel to
/// slate.
pub fn nearest_accent(color: Color) -> Accent {
    let (chroma, hue) = chroma_hue(color);
    if chroma < GREY_CHROMA {
        return Accent::Slate;
    }
    let mut best = (Accent::Blue, f64::INFINITY);
    for accent in Accent::ALL {
        if accent == Accent::Slate {
            continue;
        }
        let d = (hue - chroma_hue(accent.color()).1).abs();
        let d = d.min(360.0 - d);
        if d < best.1 {
            best = (accent, d);
        }
    }
    best.0
}

/// OKLab chroma and hue angle in degrees, 0 to 360.
fn chroma_hue(color: Color) -> (f64, f64) {
    let [_, a, b] = color.to_oklab();
    (a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_accent_maps_to_itself() {
        for accent in Accent::ALL {
            assert_eq!(nearest_accent(accent.color()), accent);
        }
    }

    #[test]
    fn bundled_theme_accents() {
        // Aetherius: periwinkle. Ember: orange.
        assert_eq!(
            nearest_accent(Color::parse("#8493D8").unwrap()),
            Accent::Blue
        );
        assert_eq!(
            nearest_accent(Color::parse("#E0914F").unwrap()),
            Accent::Orange
        );
    }

    #[test]
    fn near_grey_lands_on_slate() {
        assert_eq!(
            nearest_accent(Color::parse("#7a8088").unwrap()),
            Accent::Slate
        );
        assert_eq!(nearest_accent(Color::rgb(128, 128, 128)), Accent::Slate);
    }

    #[test]
    fn pastels_keep_their_hue() {
        // Pale shades must not collapse to slate.
        assert_eq!(
            nearest_accent(Color::parse("#f5c2e7").unwrap()),
            Accent::Pink
        );
        assert_eq!(
            nearest_accent(Color::parse("#a6e3a1").unwrap()),
            Accent::Green
        );
        assert_eq!(
            nearest_accent(Color::parse("#89b4fa").unwrap()),
            Accent::Blue
        );
    }
}
