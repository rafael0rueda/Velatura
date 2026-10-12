//! The Velatura app: pick a theme, apply it, undo it.
//!
//! Presentation only. Every read and every change goes through the engine,
//! on a worker thread; the window never touches a file. Layout and wording
//! follow docs/hig.html.

mod data;
mod shots;
mod widgets;
mod words;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use velatura_engine::engine::{self, ApplyOpts, Error, Report};
use velatura_engine::library::Entry;
use velatura_engine::plan::{Note, Plan, Skip};
use velatura_engine::targets;
use velatura_theme::{Resolved, nearest_accent};

use data::{RowState, Snapshot};
use words::{plural, tr};

const APP_ID: &str = "io.github.rafael0rueda.Velatura";

/// Below this width the view switcher moves to the bottom of the window.
const NARROW: f64 = 550.0;

struct Ui {
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    stack: adw::ViewStack,
    themes: adw::Bin,
    apps: adw::Bin,
    history: adw::Bin,
    banner: adw::Banner,
    /// Scoped colours of the preview; replaced when a theme page opens.
    preview_css: gtk::CssProvider,
    snap: RefCell<Snapshot>,
    /// True while an engine call that changes things is running.
    busy: Cell<bool>,
}

type U = Rc<Ui>;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let ui = build(app);
        ui.window.present();
        let ui2 = ui.clone();
        glib::spawn_future_local(async move {
            refresh(&ui2).await;
            shots::run_if_asked(&ui2).await;
        });
    });
    app.set_accels_for_action("win.undo", &["<Control>z"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    app.add_action_entries([quit]);
    app.run()
}

fn build(app: &adw::Application) -> U {
    let stack = adw::ViewStack::new();
    let (themes, apps, history) = (adw::Bin::new(), adw::Bin::new(), adw::Bin::new());
    for (bin, name, title, icon) in [
        (
            &themes,
            "themes",
            tr("_Themes"),
            "preferences-desktop-appearance-symbolic",
        ),
        (
            &apps,
            "apps",
            tr("_Apps & Desktop"),
            "preferences-desktop-apps-symbolic",
        ),
        (
            &history,
            "history",
            tr("_History"),
            "document-open-recent-symbolic",
        ),
    ] {
        let page = stack.add_titled_with_icon(bin, Some(name), &title, icon);
        page.set_use_underline(true);
    }

    let menu = gio::Menu::new();
    let actions = gio::Menu::new();
    actions.append(Some(&tr("_Undo Last Apply")), Some("win.undo"));
    actions.append(Some(&tr("_Troubleshooting")), Some("win.troubleshooting"));
    menu.append_section(None, &actions);
    let standard = gio::Menu::new();
    standard.append(
        Some(&tr("_Keyboard Shortcuts")),
        Some("win.show-help-overlay"),
    );
    standard.append(Some(&tr("_About Velatura")), Some("win.about"));
    menu.append_section(None, &standard);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text(tr("Main Menu"))
        .primary(true)
        .build();

    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    header.pack_end(&menu_button);
    let bottom = adw::ViewSwitcherBar::builder().stack(&stack).build();
    let banner = adw::Banner::new("");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&stack);
    stack.set_vexpand(true);
    let top = adw::ToolbarView::new();
    top.add_top_bar(&header);
    top.add_bottom_bar(&bottom);
    top.set_content(Some(&content));

    let nav = adw::NavigationView::new();
    nav.add(
        &adw::NavigationPage::builder()
            .title("Velatura")
            .tag("main")
            .child(&top)
            .build(),
    );
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Velatura")
        .default_width(760)
        .default_height(640)
        .width_request(360)
        .height_request(294)
        .content(&toasts)
        .build();

    // Narrow: no title switcher, the bar at the bottom edge takes over.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        NARROW,
        adw::LengthUnit::Sp,
    ));
    narrow.add_setter(&bottom, "reveal", Some(&true.to_value()));
    narrow.add_setter(
        &header,
        "title-widget",
        Some(&None::<gtk::Widget>.to_value()),
    );
    window.add_breakpoint(narrow);

    let preview_css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &preview_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let ui = Rc::new(Ui {
        window,
        nav,
        toasts,
        stack,
        themes,
        apps,
        history,
        banner,
        preview_css,
        snap: RefCell::new(Snapshot::default()),
        busy: Cell::new(false),
    });
    add_actions(&ui);
    ui.window.set_help_overlay(Some(&shortcuts_window()));

    let weak = Rc::downgrade(&ui);
    ui.banner.connect_button_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            glib::spawn_future_local(async move { roll_back(&ui).await });
        }
    });
    ui.window.connect_close_request(|window| {
        if !window.is_maximized() {
            let (w, h) = (window.width(), window.height());
            // Fire and forget: the size is a convenience, not state.
            gio::spawn_blocking(move || {
                data::save_window_size(w, h);
            });
        }
        glib::Propagation::Proceed
    });
    ui
}

fn add_actions(ui: &U) {
    let entry = |name: &str, ui: &U, f: fn(U)| {
        let weak = Rc::downgrade(ui);
        gio::ActionEntry::builder(name)
            .activate(move |_: &adw::ApplicationWindow, _, _| {
                if let Some(ui) = weak.upgrade() {
                    f(ui);
                }
            })
            .build()
    };
    ui.window.add_action_entries([
        entry("undo", ui, |ui| {
            glib::spawn_future_local(async move { undo(&ui).await });
        }),
        entry("troubleshooting", ui, |ui| {
            glib::spawn_future_local(async move { troubleshooting(&ui).await });
        }),
        entry("about", ui, |ui| about(&ui)),
    ]);
}

fn shortcuts_window() -> gtk::ShortcutsWindow {
    let row = |title: &str, accel: &str| {
        format!(
            "<child><object class=\"GtkShortcutsShortcut\"><property name=\"title\">{}</property><property name=\"accelerator\">{accel}</property></object></child>",
            glib::markup_escape_text(title)
        )
    };
    let xml = format!(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"help_overlay\"><property name=\"modal\">1</property><child><object class=\"GtkShortcutsSection\"><property name=\"section-name\">shortcuts</property><child><object class=\"GtkShortcutsGroup\"><property name=\"title\">{}</property>{}{}{}{}{}{}</object></child></object></child></object></interface>",
        glib::markup_escape_text(&tr("General")),
        row(&tr("Undo Last Apply"), "&lt;Control&gt;z"),
        row(&tr("Back"), "&lt;Alt&gt;Left"),
        row(&tr("Main Menu"), "F10"),
        row(&tr("Keyboard Shortcuts"), "&lt;Control&gt;question"),
        row(&tr("Close Window"), "&lt;Control&gt;w"),
        row(&tr("Quit"), "&lt;Control&gt;q"),
    );
    gtk::Builder::from_string(&xml)
        .object("help_overlay")
        .unwrap_or_else(|| gtk::ShortcutsWindow::builder().build())
}

fn toast(ui: &U, text: &str) {
    ui.toasts.add_toast(adw::Toast::new(text));
}

fn toast_with_undo(ui: &U, text: &str) {
    ui.toasts.add_toast(
        adw::Toast::builder()
            .title(text)
            .button_label(tr("_Undo"))
            .action_name("win.undo")
            .build(),
    );
}

/// Reload the picture of the machine and rebuild the three views from it.
async fn refresh(ui: &U) {
    let Some(snap) = data::run(data::snapshot).await else {
        ui.themes.set_child(Some(
            &adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title(tr("No Home Folder"))
                .description(tr("Velatura needs a home folder to work in"))
                .build(),
        ));
        return;
    };
    if let Some((w, h)) = snap
        .window
        .filter(|_| !ui.window.is_visible() || ui.snap.borrow().home.as_os_str().is_empty())
    {
        ui.window.set_default_size(w.max(360), h.max(294));
    }
    ui.themes.set_child(Some(&themes_view(ui, &snap)));
    ui.apps.set_child(Some(&apps_view(ui, &snap)));
    ui.history.set_child(Some(&history_view(ui, &snap)));

    // A lasting state gets a banner; single events get toasts.
    if snap.unfinished {
        ui.banner
            .set_title(&tr("An earlier change was interrupted"));
        ui.banner.set_button_label(Some(&tr("_Roll Back")));
        ui.banner.set_revealed(true);
    } else if adw::StyleManager::default().is_high_contrast() {
        ui.banner
            .set_title(&tr("High Contrast is on ‒ apps are not recolored"));
        ui.banner.set_button_label(None);
        ui.banner.set_revealed(true);
    } else {
        ui.banner.set_revealed(false);
    }
    *ui.snap.borrow_mut() = snap;
}

fn appearance_word(entry: &Entry, variant: &str) -> String {
    match entry.theme.variants.get(variant).map(|v| v.appearance) {
        Some(velatura_theme::Appearance::Light) => tr("Light"),
        _ => tr("Dark"),
    }
}

fn themes_view(ui: &U, snap: &Snapshot) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    // The theme in use comes first; the rest keep the library's order.
    let mut entries: Vec<(&Entry, &String)> = snap
        .themes
        .iter()
        .flat_map(|e| e.theme.variants.keys().map(move |k| (e, k)))
        .collect();
    let in_use = |e: &Entry, k: &str| {
        snap.applied.as_deref() == Some(format!("{}/{k}", e.theme.id).as_str())
    };
    entries.sort_by_key(|(e, k)| !in_use(e, k));
    for (entry, variant) in entries {
        let Some(resolved) = Resolved::new(&entry.theme, Some(variant)) else {
            continue;
        };
        let used = in_use(entry, variant);
        let style = appearance_word(entry, variant);
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&resolved.colors.name))
            .subtitle(if used {
                format!("{style} · {}", tr("In use"))
            } else {
                style
            })
            .activatable(true)
            .build();
        row.add_prefix(&widgets::swatches(&resolved));
        if used {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_tooltip_text(Some(&tr("In Use")));
            row.add_suffix(&check);
        }
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let weak = Rc::downgrade(ui);
        let spec = (entry.theme.id.clone(), variant.clone());
        row.connect_activated(move |_| {
            if let Some(ui) = weak.upgrade() {
                open_theme(&ui, &spec.0, &spec.1);
            }
        });
        group.add(&row);
    }
    page.add(&group);
    page.upcast()
}

/// The page for one theme: preview, a few facts, and the one button.
fn open_theme(ui: &U, id: &str, variant: &str) {
    let snap = ui.snap.borrow();
    let Some(entry) = snap.themes.iter().find(|e| e.theme.id == id) else {
        return;
    };
    let Some(resolved) = Resolved::new(&entry.theme, Some(variant)) else {
        return;
    };
    let used = snap.applied.as_deref() == Some(resolved.label().as_str());
    ui.preview_css
        .load_from_string(&widgets::preview_css(&resolved));

    let facts = adw::PreferencesGroup::new();
    let fact = |title: String, value: String| {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(value)
            .build();
        row.add_css_class("property");
        row
    };
    facts.add(&fact(tr("Style"), appearance_word(entry, variant)));
    let accent = nearest_accent(resolved.colors.ui.accent);
    facts.add(&fact(
        tr("System Accent"),
        tr("{accent}, the nearest to {color}")
            .replace(
                "{accent}",
                &velatura_text::accent_name(accent.setting_value()),
            )
            .replace("{color}", &resolved.colors.ui.accent.hex()),
    ));
    let contrast = resolved.colors.ui.fg.contrast(resolved.colors.ui.bg);
    facts.add(&fact(
        tr("Text Contrast"),
        tr("{ratio} to 1").replace("{ratio}", &format!("{contrast:.1}")),
    ));
    if !entry.theme.author.is_empty() {
        facts.add(&fact(tr("Author"), entry.theme.author.clone()));
    }

    let apply = gtk::Button::builder()
        .label(if used {
            tr("Applied")
        } else {
            tr("_Apply Theme")
        })
        .use_underline(true)
        .sensitive(!used)
        .halign(gtk::Align::Center)
        .build();
    apply.add_css_class("pill");
    apply.add_css_class("suggested-action");
    let weak = Rc::downgrade(ui);
    let spec = (id.to_owned(), variant.to_owned());
    apply.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            let spec = spec.clone();
            glib::spawn_future_local(async move { apply_theme(&ui, spec.0, spec.1).await });
        }
    });

    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.set_margin_top(24);
    column.set_margin_bottom(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.append(&widgets::preview(&resolved));
    column.append(&apply);
    column.append(&facts);
    let clamp = adw::Clamp::builder()
        .maximum_size(600)
        .child(&column)
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroll));
    let name = resolved.colors.name.clone();
    drop(snap);
    ui.nav.push(
        &adw::NavigationPage::builder()
            .title(name)
            .tag("theme")
            .child(&view)
            .build(),
    );
}

fn busy(ui: &U) -> bool {
    if ui.busy.get() {
        toast(ui, &tr("Still working on the last change"));
        return true;
    }
    false
}

fn error_text(e: &Error) -> String {
    match e {
        Error::Busy => tr("Another Velatura is making changes"),
        Error::NothingToUndo => tr("Nothing to undo"),
        Error::Failed(f) => velatura_text::failure(f),
        Error::Recovery(r) => format!(
            "{}\n{}",
            tr(
                "An earlier interrupted change could not be fully rolled back, so nothing new was changed."
            ),
            velatura_text::rollback(r)
        ),
        Error::Io(why) => why.clone(),
        Error::Store(_) => tr("Velatura's own records cannot be read. Nothing was changed."),
    }
}

/// Apply a theme: plan, confirm when the guidelines call for it, execute,
/// and say what happened.
async fn apply_theme(ui: &U, id: String, variant: String) {
    if busy(ui) {
        return;
    }
    let first = ui.snap.borrow().applied.is_none();
    let home = ui.snap.borrow().home.clone();
    let (plan_id, plan_variant) = (id.clone(), variant.clone());
    let planned = data::run(move |host| {
        let (themes, _) = velatura_engine::library::load(host);
        let entry = themes.iter().find(|e| e.theme.id == plan_id)?;
        let resolved = Resolved::new(&entry.theme, Some(&plan_variant))?;
        Some((
            resolved.colors.name.clone(),
            engine::plan_apply(host, &data::env, &resolved, &ApplyOpts::default()),
        ))
    })
    .await
    .flatten();
    let Some((name, plan)) = planned else {
        toast(ui, &tr("That theme is no longer available"));
        return;
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(e) => {
            result_page(
                ui,
                &tr("Theme Not Applied"),
                false,
                &[error_text(&e)],
                false,
            );
            return;
        }
    };
    if plan.is_empty() {
        toast(ui, &tr("Nothing to change"));
        return;
    }

    // Confirm the first time, and whenever a file of the user's would be
    // edited for the first time or the result may be hard to read.
    let asking: Vec<String> = plan
        .needing_consent()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let low = plan
        .targets
        .iter()
        .any(|t| t.notes.contains(&Note::LowContrast));
    if (first || !asking.is_empty() || low) && !confirm(ui, &name, &plan, &home, low).await {
        return;
    }

    ui.busy.set(true);
    let executed = data::run(move |host| engine::execute(host, &plan, &asking)).await;
    ui.busy.set(false);
    refresh(ui).await;
    match executed {
        Some(Ok(report)) => applied(ui, &name, &report, &home),
        Some(Err(e)) => result_page(
            ui,
            &tr("Theme Not Applied"),
            false,
            &[error_text(&e)],
            false,
        ),
        None => toast(ui, &tr("The change could not be started")),
    }
}

async fn confirm(ui: &U, name: &str, plan: &Plan, home: &Path, low: bool) -> bool {
    let mut body = words::summary(words::count(plan));
    if low {
        body = format!(
            "{}\n\n{body}",
            tr("Text in this theme has low contrast and may be hard to read.")
        );
    }
    let dialog = adw::AlertDialog::new(
        Some(&tr("Apply {name}?").replace("{name}", name)),
        Some(&body),
    );
    let mut lines = Vec::new();
    for t in plan.targets.iter().filter(|t| t.skipped.is_none()) {
        if t.changes.is_empty() {
            continue;
        }
        lines.push(velatura_text::instance(&t.instance).to_owned());
        lines.extend(
            t.changes
                .iter()
                .map(|c| format!("  {}", velatura_text::change(c, home))),
        );
    }
    let details = gtk::Label::builder()
        .label(lines.join("\n"))
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    details.add_css_class("caption");
    let expander = gtk::Expander::builder()
        .label(tr("Show _Details"))
        .use_underline(true)
        .child(&details)
        .build();
    dialog.set_extra_child(Some(&expander));
    let go = if low {
        tr("Apply _Anyway")
    } else {
        tr("_Apply Theme")
    };
    dialog.add_responses(&[("cancel", &tr("_Cancel")), ("apply", &go)]);
    dialog.set_close_response("cancel");
    if low {
        // Never the default: Return must not apply an unreadable theme.
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Destructive);
    } else {
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
    }
    dialog.choose_future(Some(&ui.window)).await == "apply"
}

/// Notes that leave something for the user to do or to know.
fn worth_showing(note: &Note) -> bool {
    !matches!(note, Note::AccentNearest { .. } | Note::LinkedFile { .. })
}

fn applied(ui: &U, name: &str, report: &Report, home: &Path) {
    let mut lines: Vec<String> = report
        .skipped
        .iter()
        .filter(|(_, why)| {
            !matches!(
                why,
                Skip::NotSelected | Skip::NotInstalled | Skip::NeedsAdwGtk3
            )
        })
        .map(|(instance, why)| {
            format!(
                "{}: {}",
                velatura_text::instance(instance),
                words::reason(&velatura_text::skip(why, home))
            )
        })
        .collect();
    lines.extend(
        report
            .notes
            .iter()
            .filter(|(_, n)| worth_showing(n))
            .map(|(_, n)| velatura_text::note(n, home)),
    );
    lines.dedup();
    ui.nav.pop_to_tag("main");
    if report.changes.is_empty() {
        result_page(ui, &tr("Nothing Was Changed"), false, &lines, false);
    } else if lines.is_empty() {
        toast_with_undo(ui, &tr("{name} applied").replace("{name}", name));
    } else {
        result_page(
            ui,
            &tr("{name} Applied").replace("{name}", name),
            true,
            &lines,
            report.backup.is_some(),
        );
    }
}

/// What happened, and what is left for the user to do.
fn result_page(ui: &U, title: &str, ok: bool, lines: &[String], can_undo: bool) {
    let group = adw::PreferencesGroup::new();
    for line in lines {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(line))
            .title_lines(0)
            .build();
        group.add(&row);
    }
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.append(&group);
    if can_undo {
        let undo = gtk::Button::builder()
            .label(tr("_Undo"))
            .use_underline(true)
            .action_name("win.undo")
            .halign(gtk::Align::Center)
            .build();
        undo.add_css_class("pill");
        column.append(&undo);
    }
    let status = adw::StatusPage::builder()
        .icon_name(if ok {
            "object-select-symbolic"
        } else {
            "dialog-warning-symbolic"
        })
        .title(glib::markup_escape_text(title))
        .child(
            &adw::Clamp::builder()
                .maximum_size(600)
                .child(&column)
                .build(),
        )
        .build();
    if ok && !lines.is_empty() {
        status.set_description(Some(&tr("A few things to know")));
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&status));
    ui.nav.pop_to_tag("main");
    ui.nav.push(
        &adw::NavigationPage::builder()
            .title(title)
            .tag("result")
            .child(&view)
            .build(),
    );
}

async fn undo(ui: &U) {
    if busy(ui) {
        return;
    }
    ui.busy.set(true);
    let home = ui.snap.borrow().home.clone();
    let done = data::run(|host| {
        let plan = engine::plan_undo(host)?;
        engine::execute(host, &plan, &[])
    })
    .await;
    ui.busy.set(false);
    refresh(ui).await;
    ui.nav.pop_to_tag("main");
    match done {
        Some(Ok(report)) => {
            let kept: Vec<String> = report
                .notes
                .iter()
                .filter(|(_, n)| matches!(n, Note::KeptFile { .. } | Note::KeptSetting { .. }))
                .map(|(_, n)| velatura_text::note(n, &home))
                .collect();
            if kept.is_empty() {
                toast(ui, &tr("Last apply undone"));
            } else {
                result_page(ui, &tr("Undone, With Exceptions"), true, &kept, false);
            }
        }
        Some(Err(Error::NothingToUndo)) => toast(ui, &tr("Nothing to undo")),
        Some(Err(e)) => result_page(ui, &tr("Not Undone"), false, &[error_text(&e)], false),
        None => toast(ui, &tr("The change could not be started")),
    }
}

async fn roll_back(ui: &U) {
    if busy(ui) {
        return;
    }
    ui.busy.set(true);
    let done = data::run(engine::recover).await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok(Some(r))) if r.is_clean() && r.kept.is_empty() => {
            toast(ui, &tr("The interrupted change was rolled back"));
        }
        Some(Ok(Some(r))) => {
            let title = if r.is_clean() {
                tr("Rolled Back")
            } else {
                tr("Not Fully Rolled Back")
            };
            let lines: Vec<String> = velatura_text::rollback(&r)
                .lines()
                .map(|l| l.trim().to_owned())
                .collect();
            result_page(ui, &title, r.is_clean(), &lines, false);
        }
        Some(Ok(None)) => {}
        Some(Err(e)) => toast(ui, error_text(&e).lines().next().unwrap_or_default()),
        None => {}
    }
}

fn apps_view(ui: &U, snap: &Snapshot) -> gtk::Widget {
    let page = adw::PreferencesPage::new();
    let groups = [
        (tr("Desktop"), vec![targets::APPEARANCE]),
        (tr("Terminals"), vec![targets::KITTY, targets::GHOSTTY]),
        (tr("Apps"), vec![targets::GTK4, targets::GTK3]),
    ];
    for (title, members) in groups {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for row in snap.rows.iter().filter(|r| members.contains(&r.instance)) {
            let (subtitle, available) = match &row.state {
                RowState::Themed if row.included => (tr("Themed"), true),
                RowState::Themed => (tr("Themed, and left out of the next apply"), true),
                RowState::Ready if row.included => (tr("Included in the next apply"), true),
                RowState::Ready => (tr("Left out"), true),
                RowState::Unavailable(why) => {
                    (words::reason(&velatura_text::skip(why, &snap.home)), false)
                }
            };
            let unofficial = matches!(row.instance, targets::GTK4 | targets::GTK3);
            let switch = adw::SwitchRow::builder()
                .title(velatura_text::instance(row.instance))
                .subtitle(if unofficial && available {
                    format!(
                        "{subtitle} · {}",
                        tr("Unofficial: may stop working after a GNOME update")
                    )
                } else {
                    subtitle
                })
                .subtitle_lines(0)
                .active(row.included && available)
                .sensitive(available)
                .build();
            let weak = Rc::downgrade(ui);
            let instance = row.instance;
            switch.connect_active_notify(move |switch| {
                let on = switch.is_active();
                if let Some(ui) = weak.upgrade() {
                    glib::spawn_future_local(async move {
                        let saved =
                            data::run(move |host| engine::set_included(host, instance, on)).await;
                        if !matches!(saved, Some(Ok(()))) {
                            toast(&ui, &tr("The choice could not be saved"));
                        }
                        refresh(&ui).await;
                    });
                }
            });
            group.add(&switch);
        }
        page.add(&group);
    }

    // Shown only while there is something of Velatura's to remove.
    if snap.applied.is_some() {
        let group = adw::PreferencesGroup::new();
        let remove = adw::ButtonRow::builder()
            .title(tr("_Remove Everything Velatura Added"))
            .use_underline(true)
            .build();
        let weak = Rc::downgrade(ui);
        remove.connect_activated(move |_| {
            if let Some(ui) = weak.upgrade() {
                glib::spawn_future_local(async move { remove_all(&ui).await });
            }
        });
        group.add(&remove);
        page.add(&group);
    }
    page.upcast()
}

/// Undoable, so it is done at once and Undo is offered, with no question.
async fn remove_all(ui: &U) {
    if busy(ui) {
        return;
    }
    ui.busy.set(true);
    let done = data::run(|host| {
        let plan = engine::plan_restore_original(host)?;
        engine::execute(host, &plan, &[])
    })
    .await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok(report)) if report.changes.is_empty() => toast(ui, &tr("Nothing to remove")),
        Some(Ok(_)) => toast_with_undo(ui, &tr("Everything Velatura added was removed")),
        Some(Err(e)) => result_page(ui, &tr("Not Removed"), false, &[error_text(&e)], false),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// The display name for a stored `theme/variant`, or the label itself if
/// that theme is no longer in the library.
fn theme_name(snap: &Snapshot, label: &str) -> String {
    let (id, variant) = label.split_once('/').unwrap_or((label, ""));
    snap.themes
        .iter()
        .find(|e| e.theme.id == id)
        .and_then(|e| e.theme.variants.get(variant))
        .map_or_else(|| label.to_owned(), |v| v.name.clone())
}

fn history_view(ui: &U, snap: &Snapshot) -> gtk::Widget {
    if snap.history.is_empty() {
        return adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .title(tr("No Backups Yet"))
            .description(tr("A backup is made each time a theme is applied"))
            .build()
            .upcast();
    }
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .description(tr(
            "Restoring goes back to how things were just before that change",
        ))
        .build();
    for m in &snap.history {
        let what = match (m.cause.as_str(), &m.theme) {
            ("apply", Some(label)) => {
                tr("Applied {theme}").replace("{theme}", &theme_name(snap, label))
            }
            ("restore-original", _) => tr("Removed everything Velatura added"),
            _ => tr("Change"),
        };
        let when = glib::DateTime::from_unix_local(m.time as i64)
            .and_then(|d| d.format("%x %H:%M"))
            .map(String::from)
            .unwrap_or_default();
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&what))
            .subtitle(if m.undone {
                format!("{when} · {}", tr("Undone"))
            } else {
                when
            })
            .build();
        if !m.undone {
            let restore = gtk::Button::builder()
                .label(tr("Restore…"))
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(ui);
            let (id, label) = (m.id, what.clone());
            restore.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let label = label.clone();
                    glib::spawn_future_local(async move { restore_to(&ui, id, &label).await });
                }
            });
            row.add_suffix(&restore);
        }
        group.add(&row);
    }
    page.add(&group);
    page.upcast()
}

/// Several undo steps at once cannot be reversed with one Undo, so this
/// asks first.
async fn restore_to(ui: &U, id: u64, label: &str) {
    if busy(ui) {
        return;
    }
    let steps = ui
        .snap
        .borrow()
        .history
        .iter()
        .filter(|m| !m.undone && m.id >= id)
        .count();
    let dialog = adw::AlertDialog::new(
        Some(&tr("Go Back to Before This Change?")),
        Some(&format!(
            "{label}\n\n{}",
            plural(
                "{n} change is undone. Files edited by hand since then are kept.",
                "{n} changes are undone, newest first. Files edited by hand since then are kept.",
                steps
            )
        )),
    );
    dialog.add_responses(&[("cancel", &tr("_Cancel")), ("restore", &tr("_Restore"))]);
    dialog.set_close_response("cancel");
    if dialog.choose_future(Some(&ui.window)).await != "restore" {
        return;
    }
    ui.busy.set(true);
    let done = data::run(move |host| engine::restore_to(host, id)).await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok(_)) => toast(ui, &tr("Restored")),
        Some(Err(e)) => result_page(ui, &tr("Not Restored"), false, &[error_text(&e)], false),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// What `velatura doctor` reports, as a list.
async fn troubleshooting(ui: &U) {
    let home = ui.snap.borrow().home.clone();
    let tools = data::run(targets::other_tools).await.unwrap_or_default();
    let snap = ui.snap.borrow().clone();
    let page = adw::PreferencesPage::new();

    let parts = adw::PreferencesGroup::builder()
        .title(tr("Apps and Desktop"))
        .build();
    for row in &snap.rows {
        let state = match &row.state {
            RowState::Unavailable(why) => words::reason(&velatura_text::skip(why, &home)),
            _ => tr("Can be themed"),
        };
        parts.add(
            &adw::ActionRow::builder()
                .title(velatura_text::instance(row.instance))
                .subtitle(state)
                .subtitle_lines(0)
                .build(),
        );
    }
    page.add(&parts);

    let others = adw::PreferencesGroup::builder()
        .title(tr("Other Tools That Change Colors"))
        .build();
    for tool in &tools {
        let (title, subtitle) = match tool {
            targets::OtherTool::Known { name, path } => (
                (*name).to_owned(),
                tr("Found at {path}. It may write the same colors")
                    .replace("{path}", &velatura_text::path(path, &home)),
            ),
            targets::OtherTool::Timer { unit } => (
                unit.clone(),
                tr("A timer that may change the theme is turned on"),
            ),
            targets::OtherTool::Dotfiles { name, path } => (
                (*name).to_owned(),
                tr("Found at {path}. It can overwrite changes made here")
                    .replace("{path}", &velatura_text::path(path, &home)),
            ),
        };
        others.add(
            &adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle(glib::markup_escape_text(&subtitle))
                .subtitle_lines(0)
                .build(),
        );
    }
    if tools.is_empty() {
        others.add(&adw::ActionRow::builder().title(tr("None found")).build());
    }
    page.add(&others);

    let console = adw::PreferencesGroup::builder()
        .title(tr("If the Desktop Becomes Unreadable"))
        .description(tr(
            "Switch to a text console with Ctrl+Alt+F3, sign in, and run this command",
        ))
        .build();
    let command = adw::ActionRow::builder().title("velatura undo").build();
    command.add_css_class("monospace");
    console.add(&command);
    page.add(&console);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::Dialog::builder()
        .title(tr("Troubleshooting"))
        .content_width(560)
        .content_height(560)
        .child(&view)
        .build()
        .present(Some(&ui.window));
}

fn about(ui: &U) {
    adw::AboutDialog::builder()
        .application_name("Velatura")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Rafael Rueda")
        .license_type(gtk::License::Gpl30)
        .website("https://rafael0rueda.github.io/Velatura/")
        .issue_url("https://github.com/rafael0rueda/Velatura/issues")
        .comments(tr("One color theme for the whole desktop"))
        .build()
        .present(Some(&ui.window));
}
