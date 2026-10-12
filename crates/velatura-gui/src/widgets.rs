//! The two custom-drawn parts: colour swatches and the theme preview.
//!
//! A theme's colours appear only here. They reach the screen through typed
//! [`Color`] values: as numbers handed to the drawing code, or as text made
//! by the closed set of formatters. No string from a theme file is used.

use std::fmt::Write;

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

/// A strip of the theme's main colours for a list row.
pub fn swatches(theme: &Resolved<'_>) -> gtk::DrawingArea {
    let ui = &theme.colors.ui;
    let named = [
        (tr("background"), ui.bg),
        (tr("surface"), ui.surface),
        (tr("accent"), ui.accent),
        (tr("text"), ui.fg),
        (tr("red"), theme.colors.ansi[1]),
        (tr("green"), theme.colors.ansi[2]),
        (tr("blue"), theme.colors.ansi[4]),
    ];
    let colors: Vec<Color> = named.iter().map(|(_, c)| *c).collect();
    let border = ui.border;
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
    // The colours are also there as text, for people who do not see them.
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
    area
}

/// The stylesheet that gives `.velatura-preview` the theme's colours and
/// nothing outside it. One of the app's two recorded styling exceptions.
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
