//! Development aid: when `VELATURA_SHOTS` names a folder, the app renders
//! each of its views into a PNG there and quits. Used to look at the window
//! without a person in front of it, in a throw-away session.

use std::path::Path;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::U;

async fn pause(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

fn save(ui: &U, dir: &Path, name: &str) {
    let window = &ui.window;
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) else {
        eprintln!("shots: nothing to render for {name}");
        return;
    };
    let texture = renderer.render_texture(&node, None);
    if let Err(e) = texture.save_to_png(dir.join(format!("{name}.png"))) {
        eprintln!("shots: {name}: {e}");
    }
}

pub async fn run_if_asked(ui: &U) {
    let Some(dir) = std::env::var_os("VELATURA_SHOTS") else {
        return;
    };
    let dir = Path::new(&dir);
    if std::env::var_os("VELATURA_SHOTS_RTL").is_some() {
        gtk::Widget::set_default_direction(gtk::TextDirection::Rtl);
    }
    if let Some(size) = std::env::var("VELATURA_SHOTS_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?))
    }) {
        ui.window.set_default_size(size.0, size.1);
    }
    pause(700).await;
    for view in ["themes", "apps", "history"] {
        ui.stack.set_visible_child_name(view);
        pause(500).await;
        save(ui, dir, view);
    }
    ui.stack.set_visible_child_name("themes");
    let first = ui
        .snap
        .borrow()
        .themes
        .last()
        .map(|e| (e.theme.id.clone(), e.theme.default_variant().to_owned()));
    if let Some((id, variant)) = first {
        crate::open_theme(ui, &id, &variant);
        pause(700).await;
        save(ui, dir, "theme");
        ui.nav.pop_to_tag("main");
    }
    crate::result_page(
        ui,
        "Ember Applied",
        true,
        &[
            "Apps that are already open keep their colors until reopened".to_owned(),
            "If open Kitty windows keep the old colors, press Ctrl+Shift+F5 in each".to_owned(),
        ],
        true,
    );
    pause(700).await;
    save(ui, dir, "result");
    ui.nav.pop_to_tag("main");
    crate::troubleshooting(ui).await;
    pause(700).await;
    save(ui, dir, "troubleshooting");
    if std::env::var_os("VELATURA_SHOTS_FLOW").is_some() {
        flow(ui, dir).await;
    }
    if let Some(app) = ui.window.application() {
        app.quit();
    }
}

/// Drive the main flow for real, in the sandbox: apply the last theme
/// through the confirmation, then undo it. Prints the state after each step
/// so the calling script can check it.
async fn flow(ui: &U, dir: &Path) {
    let Some(dialog) = ui.window.visible_dialog() else {
        return;
    };
    dialog.force_close();
    pause(400).await;
    let Some((id, variant)) = ui
        .snap
        .borrow()
        .themes
        .last()
        .map(|e| (e.theme.id.clone(), e.theme.default_variant().to_owned()))
    else {
        return;
    };
    println!("FLOW start applied={:?}", ui.snap.borrow().applied);
    let ui2 = ui.clone();
    glib::spawn_future_local(async move { crate::apply_theme(&ui2, id, variant).await });
    pause(1200).await;
    save(ui, dir, "confirm");
    match ui.window.visible_dialog() {
        Some(confirm) => {
            println!("FLOW confirmation shown");
            confirm.activate_default();
        }
        None => println!("FLOW no confirmation"),
    }
    pause(2000).await;
    save(ui, dir, "after-apply");
    println!("FLOW applied={:?}", ui.snap.borrow().applied);
    gtk::prelude::WidgetExt::activate_action(&ui.window, "win.undo", None).ok();
    pause(2000).await;
    save(ui, dir, "after-undo");
    println!("FLOW undone applied={:?}", ui.snap.borrow().applied);
}
