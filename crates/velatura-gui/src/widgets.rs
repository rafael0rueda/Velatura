//! The custom-drawn parts: colour swatches, the small pictures of a theme,
//! and the theme preview; and the app's own few style rules.
//!
//! A theme's colours appear only here. They reach the screen through typed
//! [`Color`] values: as numbers handed to the drawing code, or as text made
//! by the closed set of formatters. No string from a theme file is used.

use std::cell::Cell;
use std::fmt::Write;
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;
use velatura_theme::{Color, Resolved, gtk as gtk_colors};

use crate::words::tr;

fn set(cr: &cairo::Context, c: Color) {
    cr.set_source_rgb(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
    );
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -pi / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, pi / 2.0);
    cr.arc(x + r, y + h - r, r, pi / 2.0, pi);
    cr.arc(x + r, y + r, r, pi, pi * 1.5);
    cr.close_path();
}

/// The eight colours a theme is shown by, with their names, in the order
/// the drawings use them: background, surface, accent, text, red, yellow,
/// green and blue.
fn named(theme: &Resolved<'_>) -> [(String, Color); 8] {
    let ui = &theme.colors.ui;
    [
        (tr("background"), ui.bg),
        (tr("surface"), ui.surface),
        (tr("accent"), ui.accent),
        (tr("text"), ui.fg),
        (tr("red"), theme.colors.ansi[1]),
        (tr("yellow"), theme.colors.ansi[3]),
        (tr("green"), theme.colors.ansi[2]),
        (tr("blue"), theme.colors.ansi[4]),
    ]
}

/// The theme's eight colours, as [`named`] orders them.
pub fn palette(theme: &Resolved<'_>) -> [Color; 8] {
    named(theme).map(|(_, c)| c)
}

/// The colours are also there as text, for people who do not see them.
fn speak(area: &gtk::DrawingArea, named: &[(String, Color)]) {
    let spoken: Vec<String> = named
        .iter()
        .map(|(name, c)| format!("{name} {}", c.hex()))
        .collect();
    area.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}: {}",
        tr("Colors"),
        spoken.join(", ")
    ))]);
    area.set_accessible_role(gtk::AccessibleRole::Img);
}

fn set_alpha(cr: &cairo::Context, c: Color, alpha: f64) {
    cr.set_source_rgba(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        alpha,
    );
}

/// A filled bar with round ends.
fn bar(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    rounded(cr, x, y, w, h, h / 2.0);
    let _ = cr.fill();
}

fn disc(cr: &cairo::Context, x: f64, y: f64, d: f64) {
    cr.arc(
        x + d / 2.0,
        y + d / 2.0,
        d / 2.0,
        0.0,
        std::f64::consts::TAU,
    );
    let _ = cr.fill();
}

/// A strip of the theme's main colours for a list row.
pub fn swatches(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let named = named(theme);
    let colors: Vec<Color> = named.iter().map(|(_, c)| *c).collect();
    let border = theme.colors.ui.border;
    let area = gtk::DrawingArea::builder()
        .content_width(84)
        .content_height(24)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 6.0);
        cr.clip_preserve();
        let step = w / colors.len() as f64;
        for (i, c) in colors.iter().enumerate() {
            set(cr, *c);
            cr.rectangle(step * i as f64, 0.0, step + 1.0, h);
            let _ = cr.fill();
        }
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 6.0);
        set(cr, border);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    speak(&area, &named);
    area
}

/// The theme in use as a small window: a title bar, three lines of text,
/// a button in the accent and four dots in red, yellow, green and blue.
pub fn window_art(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let named = named(theme);
    let c = palette(theme);
    let border = theme.colors.ui.border;
    let area = gtk::DrawingArea::builder()
        .content_width(240)
        .content_height(144)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(cr, 0.0, 0.0, w, h, 8.0);
        cr.clip();
        set(cr, c[0]);
        let _ = cr.paint();
        set(cr, c[1]);
        cr.rectangle(0.0, 0.0, w, 22.0);
        let _ = cr.fill();
        // The title, centred in the bar.
        let mid = w / 2.0;
        set_alpha(cr, c[3], 0.7);
        bar(cr, mid - 30.0, 8.5, 34.0, 5.0);
        set_alpha(cr, c[3], 0.35);
        bar(cr, mid + 10.0, 8.5, 20.0, 5.0);
        // The body's text: a heading and two dim lines.
        set(cr, c[3]);
        bar(cr, 14.0, 36.0, 110.0, 7.0);
        set_alpha(cr, c[3], 0.45);
        bar(cr, 14.0, 51.0, 150.0, 5.0);
        bar(cr, 14.0, 64.0, 130.0, 5.0);
        // An outlined button in the accent, and the four dots.
        rounded(cr, 14.75, 81.75, 50.5, 16.5, 5.0);
        set(cr, c[2]);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
        for (i, color) in c[4..8].iter().enumerate() {
            set(cr, *color);
            disc(cr, 72.0 + 18.0 * i as f64, 84.0, 12.0);
        }
        cr.reset_clip();
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 8.0);
        set(cr, border);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    speak(&area, &named);
    area
}

/// The top of a theme's card: its background with one line of its text
/// and a row of six dots, from the accent to blue.
pub fn card_art(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let named = named(theme);
    let c = palette(theme);
    let area = gtk::DrawingArea::builder()
        .content_height(64)
        .hexpand(true)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        set(cr, c[0]);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
        let dots = h - 10.0 - 16.0;
        set_alpha(cr, c[3], 0.8);
        bar(cr, 12.0, dots - 8.0 - 5.0, 60.0, 5.0);
        for (i, color) in c[2..8].iter().enumerate() {
            let x = 12.0 + 20.0 * i as f64;
            set_alpha(cr, Color::rgb(0, 0, 0), 0.15);
            disc(cr, x - 1.0, dots - 1.0, 18.0);
            set(cr, *color);
            disc(cr, x, dots, 16.0);
        }
    });
    speak(&area, &named);
    area
}

/// Five equal parts, background to red, for a row in History.
pub fn strip(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let named = named(theme);
    let c = palette(theme);
    let border = theme.colors.ui.border;
    let area = gtk::DrawingArea::builder()
        .content_width(44)
        .content_height(14)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 4.0);
        cr.clip_preserve();
        let step = w / 5.0;
        for (i, color) in c[..5].iter().enumerate() {
            set(cr, *color);
            cr.rectangle(step * i as f64, 0.0, step + 1.0, h);
            let _ = cr.fill();
        }
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, 4.0);
        set(cr, border);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    speak(&area, &named[..5]);
    area
}

/// Four small dots, from the accent to green, beside a theme's name in a
/// drop-down. Nothing is drawn for `None`.
pub fn small_dots() -> gtk::DrawingArea {
    gtk::DrawingArea::builder()
        .content_width(4 * 10 + 3 * 3)
        .content_height(10)
        .valign(gtk::Align::Center)
        .build()
}

/// Paint `colors` into an area made by [`small_dots`].
pub fn paint_dots(area: &gtk::DrawingArea, colors: Option<[Color; 4]>) {
    area.set_visible(colors.is_some());
    let Some(colors) = colors else {
        return;
    };
    area.set_draw_func(move |_, cr, _, h| {
        let y = (f64::from(h) - 10.0) / 2.0;
        for (i, color) in colors.iter().enumerate() {
            set(cr, *color);
            disc(cr, 13.0 * i as f64, y, 10.0);
        }
    });
    area.queue_draw();
}

/// What libadwaita has no style class for: the outline of the theme card
/// in use, the tile behind a part's icon, a tag beside a part's name, and a
/// part that cannot be themed. Named colours only, so it follows the
/// system's style, accent and High Contrast. One of the app's three
/// recorded styling exceptions.
pub const APP_CSS: &str = "\
button.card.velatura-theme { padding: 0; }
button.card.velatura-in-use:not(:focus-visible) { outline: 2px solid @accent_color; outline-offset: -2px; }
.velatura-tile { min-width: 32px; min-height: 32px; border-radius: 8px; background-color: alpha(currentColor, 0.08); }
.velatura-tag { padding: 1px 6px; border-radius: 4px; border: 1px solid alpha(currentColor, 0.15); background-color: alpha(currentColor, 0.05); }
row.velatura-unavailable .velatura-tile, row.velatura-unavailable .title { opacity: 0.5; }
";

/// One colour as a filled circle, for a button that chooses it. While
/// `marked` is true it carries a check mark, so the choice does not rest
/// on telling the colours apart.
pub fn dot(color: Color, outline: Color, marked: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(28)
        .content_height(28)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        let r = w.min(h) / 2.0 - 0.5;
        cr.arc(w / 2.0, h / 2.0, r, 0.0, std::f64::consts::TAU);
        set(cr, color);
        let _ = cr.fill_preserve();
        set(cr, outline);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if marked.get() {
            let (black, white) = (Color::rgb(0, 0, 0), Color::rgb(255, 255, 255));
            set(
                cr,
                if color.contrast(black) >= color.contrast(white) {
                    black
                } else {
                    white
                },
            );
            cr.set_line_width(2.5);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.move_to(w * 0.30, h * 0.52);
            cr.line_to(w * 0.44, h * 0.66);
            cr.line_to(w * 0.71, h * 0.36);
            let _ = cr.stroke();
        }
    });
    area
}

/// The stylesheet that gives `.velatura-preview` the theme's colours and
/// nothing outside it. One of the app's three recorded styling exceptions.
pub fn preview_css(theme: &Resolved<'_>) -> String {
    let mut css = String::from(".velatura-preview {\n");
    for (name, color) in gtk_colors::colors(theme) {
        let value = if color.is_opaque() {
            color.hex()
        } else {
            color.css_rgba()
        };
        let _ = writeln!(css, "  --{}: {value};", name.replace('_', "-"));
    }
    css.push_str(
        "  background-color: var(--window-bg-color);\n  color: var(--window-fg-color);\n  border-radius: 12px;\n}\n\
         .velatura-preview .velatura-bar {\n  background-color: var(--headerbar-bg-color);\n  color: var(--headerbar-fg-color);\n  border-radius: 12px 12px 0 0;\n  padding: 6px 12px;\n}\n\
         .velatura-preview .velatura-body {\n  padding: 12px;\n}\n",
    );
    css
}

fn terminal(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let v = theme.colors.clone();
    let area = gtk::DrawingArea::builder()
        .content_height(118)
        .hexpand(true)
        .build();
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(cr, 0.0, 0.0, w, h, 12.0);
        set(cr, v.ui.bg);
        let _ = cr.fill();
        cr.select_font_face(
            "monospace",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
        cr.set_font_size(13.0);
        let lines: [&[(&str, Color)]; 4] = [
            &[("~/projects ", v.ansi[4]), ("git status", v.ui.fg)],
            &[("modified: ", v.ansi[3]), ("theme.toml", v.ui.fg)],
            &[("added: ", v.ansi[2]), ("preview.rs", v.ui.fg)],
            &[("deleted: ", v.ansi[1]), ("old.conf", v.ui.fg_subtle)],
        ];
        for (i, line) in lines.iter().enumerate() {
            cr.move_to(14.0, 26.0 + 18.0 * i as f64);
            for (text, color) in *line {
                set(cr, *color);
                let _ = cr.show_text(text);
            }
        }
        // The sixteen terminal colours along the bottom.
        let size = ((w - 28.0) / 16.0).min(18.0);
        for (i, c) in v.ansi.iter().enumerate() {
            set(cr, *c);
            cr.rectangle(
                14.0 + size * i as f64,
                h - size - 10.0,
                size - 2.0,
                size - 2.0,
            );
            let _ = cr.fill();
        }
    });
    area.set_accessible_role(gtk::AccessibleRole::Img);
    area.update_property(&[gtk::accessible::Property::Label(&tr(
        "Illustration of a terminal in this theme",
    ))]);
    area
}

/// What the theme looks like: real widgets wearing its colours, and a
/// drawing of a terminal.
pub fn preview(theme: &Resolved<'_>) -> gtk::Box {
    let bar = gtk::CenterBox::new();
    bar.add_css_class("velatura-bar");
    let title = gtk::Label::new(Some(&tr("Window")));
    title.add_css_class("heading");
    bar.set_center_widget(Some(&title));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("velatura-body");
    body.append(
        &gtk::Label::builder()
            .label(tr("Text in this theme"))
            .xalign(0.0)
            .build(),
    );
    let dim = gtk::Label::builder()
        .label(tr("Secondary text"))
        .xalign(0.0)
        .build();
    dim.add_css_class("dim-label");
    body.append(&dim);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let button = gtk::Button::with_label(&tr("Button"));
    button.add_css_class("suggested-action");
    let plain = gtk::Button::with_label(&tr("Button"));
    let switch = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    // A picture of controls, not controls: nothing here does anything.
    for widget in [
        button.upcast_ref::<gtk::Widget>(),
        plain.upcast_ref(),
        switch.upcast_ref(),
    ] {
        widget.set_can_target(false);
        widget.set_focusable(false);
        controls.append(widget);
    }
    body.append(&controls);

    let window = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add_css_class("velatura-preview");
    window.append(&bar);
    window.append(&body);

    let caption = gtk::Label::builder()
        .label(tr("Illustration. Some apps keep their own colors"))
        .wrap(true)
        .build();
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");

    let all = gtk::Box::new(gtk::Orientation::Vertical, 12);
    all.append(&window);
    all.append(&terminal(theme));
    all.append(&caption);
    all
}
