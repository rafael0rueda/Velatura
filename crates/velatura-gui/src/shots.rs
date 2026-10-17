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
    // What this window computes for its own background, for the check that
    // an applied theme does not recolour the app itself.
    #[allow(deprecated)]
    if let Some(c) = ui.window.style_context().lookup_color("window_bg_color") {
        let byte = |v: f32| (v * 255.0).round() as u8;
        println!(
            "SHOTS own window_bg_color=#{:02x}{:02x}{:02x}",
            byte(c.red()),
            byte(c.green()),
            byte(c.blue())
        );
    }
    // The schedule as the window knows it, and what its banner says.
    println!(
        "SHOTS schedule on={} banner={:?}",
        ui.snap.borrow().schedule.on,
        ui.banner
            .is_revealed()
            .then(|| ui.banner.title().to_string())
    );
    for view in ["themes", "apps", "history"] {
        ui.stack.set_visible_child_name(view);
        pause(500).await;
        save(ui, dir, view);
        // And the end of the view, scrolled down.
        if let Some(scroll) = ui
            .stack
            .visible_child()
            .and_then(|bin| bin.first_child())
            .and_then(|view| view.first_child())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            let bar = scroll.vadjustment();
            bar.set_value(bar.upper());
            pause(300).await;
            save(ui, dir, &format!("{view}-end"));
            // Rebuilt, as after any change made on it, the view stays
            // scrolled where it was.
            let at = bar.value();
            if at > 0.0 {
                crate::refresh(ui).await;
                pause(700).await;
                let now = ui
                    .stack
                    .visible_child()
                    .and_then(|bin| bin.first_child())
                    .and_then(|view| view.first_child())
                    .and_downcast::<gtk::ScrolledWindow>()
                    .map_or(0.0, |s| s.vadjustment().value());
                println!("SHOTS kept {view}={}", (now - at).abs() < 1.0);
            }
        }
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
    if std::env::var_os("VELATURA_SHOTS_EDITOR").is_some() {
        editor(ui, dir).await;
    }
    if let Some(file) = std::env::var_os("VELATURA_SHOTS_IMPORT") {
        import(ui, dir, file.into()).await;
    }
    if std::env::var_os("VELATURA_SHOTS_FLOW").is_some() {
        flow(ui, dir).await;
    }
    if let Some(app) = ui.window.application() {
        app.quit();
    }
}

/// Every toggle button under `widget`, in the order they are laid out.
fn toggles(widget: &gtk::Widget, out: &mut Vec<gtk::ToggleButton>) {
    if let Some(button) = widget.downcast_ref::<gtk::ToggleButton>() {
        out.push(button.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        toggles(&c, out);
        child = c.next_sibling();
    }
}

/// Import a file in another format the way a person would: look at the
/// question, choose another accent than the usual one, and import.
async fn import(ui: &U, dir: &Path, file: std::path::PathBuf) {
    if let Some(dialog) = ui.window.visible_dialog() {
        dialog.force_close();
        pause(400).await;
    }
    let ui2 = ui.clone();
    glib::spawn_future_local(async move { crate::import_file(&ui2, file).await });
    pause(1200).await;
    save(ui, dir, "import");
    let Some(question) = ui.window.visible_dialog() else {
        println!("SHOTS import no question");
        return;
    };
    let mut choices = Vec::new();
    toggles(question.upcast_ref(), &mut choices);
    let usual = choices.iter().position(|b| b.is_active());
    println!("SHOTS import accents={} usual={usual:?}", choices.len());
    // The one before the last: for a base16 scheme, its purple.
    if let Some(other) = choices.len().checked_sub(2).and_then(|i| choices.get(i)) {
        other.set_active(true);
        println!(
            "SHOTS import chosen={}",
            other.tooltip_text().unwrap_or_default()
        );
    }
    pause(500).await;
    save(ui, dir, "import-chosen");
    question.activate_default();
    pause(1500).await;
    save(ui, dir, "imported");
}

/// Make a theme in the editor the way a person would: start a new one,
/// change colours until something is hard to read, fix it, and save.
async fn editor(ui: &U, dir: &Path) {
    if let Some(dialog) = ui.window.visible_dialog() {
        dialog.force_close();
        pause(400).await;
    }
    gtk::prelude::WidgetExt::activate_action(&ui.window, "win.new-theme", None).ok();
    pause(700).await;
    save(ui, dir, "editor");
    crate::editor::poke_current();
    pause(700).await;
    save(ui, dir, "editor-changed");
    // With a change not saved, Ctrl+Z is the editor's to leave alone, and
    // closing the window asks first instead of losing it.
    let undo = ui
        .window
        .lookup_action("undo")
        .is_some_and(|a| a.is_enabled());
    println!("SHOTS editor undo={}", if undo { "on" } else { "off" });
    ui.window.close();
    pause(700).await;
    let asks = ui
        .window
        .visible_dialog()
        .and_downcast::<adw::AlertDialog>()
        .map(|d| d.heading().unwrap_or_default().to_string());
    println!("SHOTS editor asks={asks:?}");
    save(ui, dir, "editor-asks");
    if let Some(dialog) = ui.window.visible_dialog() {
        dialog.force_close();
        pause(400).await;
    }
    gtk::prelude::WidgetExt::activate_action(&ui.window, "win.save-theme", None).ok();
    pause(1500).await;
    save(ui, dir, "editor-saved");
    let mine: Vec<String> = ui
        .snap
        .borrow()
        .themes
        .iter()
        .filter(|e| !e.bundled)
        .map(|e| e.theme.id.clone())
        .collect();
    println!("SHOTS library yours={mine:?}");
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
    // The page saying Velatura is at work comes up at once.
    pause(300).await;
    save(ui, dir, "preparing");
    let shown = ui
        .nav
        .visible_page()
        .and_then(|p| p.tag())
        .map(|t| t.to_string());
    println!(
        "FLOW working page {}",
        if shown.as_deref() == Some("working") {
            "shown"
        } else {
            "not shown"
        }
    );
    pause(900).await;
    save(ui, dir, "confirm");
    match ui.window.visible_dialog() {
        Some(confirm) => {
            println!("FLOW confirmation shown");
            confirm.activate_default();
        }
        None => println!("FLOW no confirmation"),
    }
    pause(500).await;
    save(ui, dir, "applying");
    pause(1500).await;
    save(ui, dir, "after-apply");
    println!("FLOW applied={:?}", ui.snap.borrow().applied);
    gtk::prelude::WidgetExt::activate_action(&ui.window, "win.undo", None).ok();
    pause(2000).await;
    save(ui, dir, "after-undo");
    println!("FLOW undone applied={:?}", ui.snap.borrow().applied);
}
