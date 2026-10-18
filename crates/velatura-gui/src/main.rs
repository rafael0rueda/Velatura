//! The Velatura app: pick a theme, apply it, undo it.
//!
//! Presentation only. Every read and every change goes through the engine,
//! on a worker thread; the window never touches a file. Layout and wording
//! follow docs/hig.html.

mod data;
mod editor;
mod keep;
mod shots;
mod stock;
mod widgets;
mod words;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use velatura_engine::engine::{self, ApplyOpts, Error, Report};
use velatura_engine::host::StdSystemd;
use velatura_engine::library::{self, Entry};
use velatura_engine::look::{self, Kind};
use velatura_engine::plan::{Note, Plan, Skip};
use velatura_engine::schedule::{self, Refused, Times};
use velatura_engine::store::DayNight;
use velatura_engine::targets;
use velatura_theme::shell::{BarStyle, TopBar};
use velatura_theme::{Resolved, nearest_accent};

use data::{RowState, Snapshot};
use words::{plural, tr};

const APP_ID: &str = "io.github.rafael0rueda.Velatura";

/// Below this width the view switcher moves to the bottom of the window.
const NARROW: f64 = 550.0;

/// History lists this many changes, and this many more each time older
/// ones are asked for.
const HISTORY_PAGE: usize = 40;

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
    /// True while the banner's button rolls an interrupted change back.
    banner_rolls_back: Cell<bool>,
    /// Which themes the grid shows; kept when the views are rebuilt.
    filter: Rc<Cell<Filter>>,
    /// How many changes History lists before "Show Older".
    history_shown: Cell<usize>,
    /// Apps & Desktop's "Apply Changes", shown while something is waiting
    /// for the next apply; kept to show it without rebuilding the page.
    apply_changes: RefCell<Option<adw::PreferencesGroup>>,
    /// The top bar's opacity waiting to be saved while the slider moves.
    opacity_pending: Cell<Option<glib::SourceId>>,
}

type U = Rc<Ui>;

fn main() -> glib::ExitCode {
    // The sentences shared with the command use the same catalog.
    velatura_text::set_language(&|name| std::env::var(name).ok());
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
    app.set_accels_for_action("win.new-theme", &["<Control>n"]);
    app.set_accels_for_action("win.import", &["<Control>o"]);
    app.set_accels_for_action("win.save-theme", &["<Control>s"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    let quit = gio::ActionEntry::builder("quit")
        // Closed as the close button would, so unsaved work is asked about.
        .activate(|app: &adw::Application, _, _| {
            for window in app.windows() {
                window.close();
            }
        })
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
            "view-app-grid-symbolic",
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
    actions.append(Some(&tr("_Undo Last Change")), Some("win.undo"));
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
        .default_width(880)
        .default_height(800)
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

    keep_stock_colors(&window);

    let app_css = gtk::CssProvider::new();
    app_css.load_from_string(widgets::APP_CSS);
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &app_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
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
        banner_rolls_back: Cell::new(false),
        filter: Rc::default(),
        history_shown: Cell::new(HISTORY_PAGE),
        apply_changes: RefCell::new(None),
        opacity_pending: Cell::new(None),
    });
    // Every handler holds the state weakly; the window holds the one strong
    // reference, and lets go of it when it is destroyed. Without it the
    // state goes as soon as start-up is over, and no row or button responds.
    let owner = RefCell::new(Some(ui.clone()));
    ui.window.connect_destroy(move |_| {
        owner.take();
    });
    add_actions(&ui);
    // GTK gives a key to the window's shortcuts before the widget with the
    // focus: Ctrl+Z typed in a text field, or in the theme editor, would
    // undo the last apply instead. Undo is off there, and the key goes on
    // to the field.
    let weak = Rc::downgrade(&ui);
    ui.window.connect_focus_widget_notify(move |_| {
        if let Some(ui) = weak.upgrade() {
            sync_undo(&ui);
        }
    });
    let weak = Rc::downgrade(&ui);
    ui.nav.connect_visible_page_notify(move |_| {
        if let Some(ui) = weak.upgrade() {
            sync_undo(&ui);
        }
    });
    ui.window.set_help_overlay(Some(&shortcuts_window()));

    let weak = Rc::downgrade(&ui);
    ui.banner.connect_button_clicked(move |_| {
        if let Some(ui) = weak.upgrade().filter(|ui| ui.banner_rolls_back.get()) {
            glib::spawn_future_local(async move { roll_back(&ui).await });
        }
    });
    let weak = Rc::downgrade(&ui);
    ui.window.connect_close_request(move |window| {
        if let Some(ui) = weak.upgrade().filter(|_| editor::unsaved()) {
            let window = window.clone();
            glib::spawn_future_local(async move {
                if editor::may_discard(&ui).await {
                    window.close();
                }
            });
            return glib::Propagation::Stop;
        }
        if !window.is_maximized() {
            // Written here and not on a worker thread: the app quits as the
            // window closes, and would cut a write in progress short. One
            // small file.
            data::save_window_size(window.width(), window.height());
        }
        glib::Propagation::Proceed
    });
    ui
}

/// libadwaita's own colours for the current style, as definitions that
/// take precedence over the user's stylesheet.
fn stock_css() -> String {
    let style = adw::StyleManager::default();
    let table: &[(&str, &str)] = if style.is_dark() {
        &stock::DARK
    } else {
        &stock::LIGHT
    };
    // The accent is the system's, read now, so it follows a change of it.
    let accent = style.accent_color().to_rgba().to_string();
    let accent_text = style.accent_color_rgba().to_string();
    let mut css = String::new();
    for (name, value) in table {
        let value = if name.ends_with("selected_bg_color") {
            accent.as_str()
        } else {
            value
        };
        css.push_str(&format!("@define-color {name} {value};\n"));
    }
    css.push_str(&format!(
        "@define-color accent_bg_color {accent};\n@define-color accent_fg_color #ffffff;\n@define-color accent_color {accent_text};\n"
    ));
    css
}

/// Keep this app's window in stock colours whatever theme is applied, so a
/// theme that turns out unreadable cannot hide the way back. GTK loads the
/// user's stylesheet into every app, this one included; these definitions
/// sit one step above it. One of the app's three recorded styling
/// exceptions: it holds colour definitions only.
fn keep_stock_colors(window: &adw::ApplicationWindow) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&stock_css());
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(window),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );
    let style = adw::StyleManager::default();
    for property in ["dark", "accent-color", "high-contrast"] {
        let provider = provider.clone();
        style.connect_notify_local(Some(property), move |_, _| {
            provider.load_from_string(&stock_css());
        });
    }
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
        entry("new-theme", ui, |ui| editor::new_theme(&ui)),
        entry("import", ui, |ui| {
            glib::spawn_future_local(async move { import_theme(&ui).await });
        }),
        entry("save-theme", ui, |_| {
            glib::spawn_future_local(editor::save_current());
        }),
    ]);
    // What a theme's card and its menu do, given its `theme/variant`.
    let theme_entry = |name: &str, ui: &U, f: fn(U, String, String)| {
        let weak = Rc::downgrade(ui);
        gio::ActionEntry::builder(name)
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(move |_: &adw::ApplicationWindow, _, spec| {
                let (Some(ui), Some(spec)) = (weak.upgrade(), spec.and_then(|s| s.str())) else {
                    return;
                };
                let (id, variant) = spec.split_once('/').unwrap_or((spec, ""));
                f(ui, id.to_owned(), variant.to_owned());
            })
            .build()
    };
    ui.window.add_action_entries([
        theme_entry("theme-details", ui, |ui, id, variant| {
            open_theme(&ui, &id, &variant);
        }),
        theme_entry("edit-theme", ui, |ui, id, variant| {
            editor::edit(&ui, &id, &variant);
        }),
        theme_entry("export-theme", ui, |ui, id, _| {
            glib::spawn_future_local(async move { export_theme(&ui, id).await });
        }),
        theme_entry("remove-theme", ui, |ui, id, _| {
            glib::spawn_future_local(async move { remove_theme(&ui, id).await });
        }),
    ]);
}

/// Undo is there except while text is typed or a theme is edited.
fn sync_undo(ui: &Ui) {
    let typing = gtk::prelude::GtkWindowExt::focus(&ui.window)
        .is_some_and(|w| w.is::<gtk::Editable>() || w.is::<gtk::TextView>());
    let editing = ui.nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("editor");
    if let Some(undo) = ui
        .window
        .lookup_action("undo")
        .and_downcast::<gio::SimpleAction>()
    {
        undo.set_enabled(!typing && !editing);
    }
}

fn shortcuts_window() -> gtk::ShortcutsWindow {
    let row = |title: &str, accel: &str| {
        format!(
            "<child><object class=\"GtkShortcutsShortcut\"><property name=\"title\">{}</property><property name=\"accelerator\">{accel}</property></object></child>",
            glib::markup_escape_text(title)
        )
    };
    let xml = format!(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"help_overlay\"><property name=\"modal\">1</property><child><object class=\"GtkShortcutsSection\"><property name=\"section-name\">shortcuts</property><child><object class=\"GtkShortcutsGroup\"><property name=\"title\">{}</property>{}{}{}{}{}{}{}{}{}</object></child></object></child></object></interface>",
        glib::markup_escape_text(&tr("General")),
        row(&tr("Undo Last Change"), "&lt;Control&gt;z"),
        row(&tr("New Theme"), "&lt;Control&gt;n"),
        row(&tr("Import Theme"), "&lt;Control&gt;o"),
        row(&tr("Save Theme"), "&lt;Control&gt;s"),
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
    keep::replace(&ui.themes, &themes_view(ui, &snap));
    keep::replace(&ui.apps, &apps_view(ui, &snap));
    keep::replace(&ui.history, &history_view(ui, &snap));

    // A lasting state gets a banner; single events get toasts.
    ui.banner_rolls_back.set(snap.unfinished);
    let left = snap
        .schedule
        .last
        .as_ref()
        .and_then(velatura_text::last_run);
    if snap.unfinished {
        ui.banner
            .set_title(&tr("An earlier change was interrupted"));
        ui.banner.set_button_label(Some(&tr("_Roll Back")));
        ui.banner.set_revealed(true);
    } else if adw::StyleManager::default().is_high_contrast() {
        ui.banner
            .set_title(&tr("High Contrast is on, so apps are not recolored"));
        ui.banner.set_button_label(None);
        ui.banner.set_revealed(true);
    } else if let Some(left) = left {
        // What a switch made with no one present could not do by itself.
        ui.banner.set_title(&left);
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

/// Which themes the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Dark,
    Light,
}

/// A view's scrolling column: content no wider than 680 pixels, its
/// sections 32 apart, and what changes on a narrow window.
struct Column {
    view: adw::BreakpointBin,
    column: gtk::Box,
    /// Below 700 pixels.
    narrow: adw::Breakpoint,
    /// Below 450 pixels. Only one breakpoint applies at a time, the last
    /// added that matches, so this one holds the first one's setters too.
    narrowest: adw::Breakpoint,
}

impl Column {
    fn new() -> Self {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .margin_top(28)
            .margin_bottom(48)
            .margin_start(24)
            .margin_end(24)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(680)
            .tightening_threshold(560)
            .child(&column)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let below = |width| {
            adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MaxWidth,
                width,
                adw::LengthUnit::Sp,
            ))
        };
        let this = Self {
            view: adw::BreakpointBin::builder()
                .width_request(300)
                .height_request(200)
                .child(&scroll)
                .build(),
            column,
            narrow: below(700.0),
            narrowest: below(450.0),
        };
        for side in ["margin-start", "margin-end"] {
            this.when_narrowest(&this.column, side, 12);
        }
        this
    }

    /// Set `property` of `object` to `value` below 700 pixels.
    fn when_narrow(&self, object: &impl IsA<glib::Object>, property: &str, value: impl ToValue) {
        self.narrow
            .add_setter(object, property, Some(&value.to_value()));
        self.narrowest
            .add_setter(object, property, Some(&value.to_value()));
    }

    /// Set `property` of `object` to `value` below 450 pixels.
    fn when_narrowest(&self, object: &impl IsA<glib::Object>, property: &str, value: impl ToValue) {
        self.narrowest
            .add_setter(object, property, Some(&value.to_value()));
    }

    fn append(&self, child: &impl IsA<gtk::Widget>) {
        self.column.append(child);
    }

    /// The finished view, once everything is in it.
    fn done(self) -> gtk::Widget {
        self.view.add_breakpoint(self.narrow);
        self.view.add_breakpoint(self.narrowest);
        self.view.upcast()
    }
}

/// A button with an icon and a label, and an access key in the label.
fn icon_button(icon: &str, label: &str) -> gtk::Button {
    gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name(icon)
                .label(label)
                .use_underline(true)
                .build(),
        )
        .valign(gtk::Align::Center)
        .build()
}

/// The `theme/variant` label of a theme and variant.
fn spec_of(id: &str, variant: &str) -> String {
    format!("{id}/{variant}")
}

/// A day as History heads it and as a sentence names it: today, yesterday,
/// or the weekday and date.
fn day_name(at: &glib::DateTime) -> String {
    let Ok(today) = glib::DateTime::now_local() else {
        return String::new();
    };
    let yesterday = today.add_days(-1).ok();
    if at.ymd() == today.ymd() {
        tr("Today")
    } else if yesterday.is_some_and(|y| y.ymd() == at.ymd()) {
        tr("Yesterday")
    } else {
        let format = if at.year() == today.year() {
            // Translators: a day in History, as in "Wednesday 14 October".
            // The codes are those of strftime: %A the weekday, %-d the
            // day of the month, %B the month.
            tr("%A %-d %B")
        } else {
            // Translators: a day in History of another year, as in
            // "Wednesday 14 October 2025"; %Y is the year.
            tr("%A %-d %B %Y")
        };
        at.format(&format).map(String::from).unwrap_or_default()
    }
}

/// "Applied today at 08:55", from the newest change that applied `label`.
fn applied_when(snap: &Snapshot, label: &str) -> Option<String> {
    let change = snap.history.iter().find(|m| {
        !m.undone
            && matches!(m.cause.as_str(), "apply" | "scheduled")
            && m.theme.as_deref() == Some(label)
    })?;
    let at = glib::DateTime::from_unix_local(change.time as i64).ok()?;
    let time = at.format("%H:%M").ok()?.to_string();
    let today = glib::DateTime::now_local().ok()?;
    let text = if at.ymd() == today.ymd() {
        tr("Applied today at {time}")
    } else if today.add_days(-1).ok().is_some_and(|y| y.ymd() == at.ymd()) {
        tr("Applied yesterday at {time}")
    } else {
        tr("Applied on {day} at {time}").replace("{day}", &day_name(&at))
    };
    Some(text.replace("{time}", &time))
}

fn themes_view(ui: &U, snap: &Snapshot) -> gtk::Widget {
    let page = Column::new();
    page.append(&current_card(ui, snap, &page));
    page.append(&theme_grid(ui, snap, &page));
    page.append(&day_night_section(ui, snap, &page));
    page.done()
}

/// The theme in use, large: a picture of it, its name, when it was
/// applied, and what can be done with it.
/// Narrow, it stacks its picture over its words, and its buttons too.
fn current_card(ui: &U, snap: &Snapshot, page: &Column) -> gtk::Widget {
    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(22)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    page.when_narrow(&inner, "orientation", gtk::Orientation::Vertical);
    for side in ["margin-top", "margin-bottom", "margin-start", "margin-end"] {
        page.when_narrowest(&inner, side, 12);
    }
    let card = adw::Bin::builder()
        .child(&inner)
        .css_classes(["card"])
        .build();
    let words = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    let Some(label) = snap.applied.clone() else {
        let title = gtk::Label::builder()
            .label(tr("No Theme in Use"))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-2"])
            .build();
        let hint = gtk::Label::builder()
            .label(tr(
                "Selecting a theme below applies it to the whole desktop",
            ))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        words.append(&title);
        words.append(&hint);
        inner.append(&words);
        return card.upcast();
    };
    let (id, variant) = label.split_once('/').unwrap_or((label.as_str(), ""));
    let entry = snap.themes.iter().find(|e| e.theme.id == id);
    let resolved = entry.and_then(|e| Resolved::new(&e.theme, Some(variant)));
    if let Some(resolved) = &resolved {
        inner.append(&widgets::window_art(resolved));
    }

    let style = entry.map_or_else(|| tr("Dark"), |e| appearance_word(e, variant));
    let kicker = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .css_classes(["accent"])
        .build();
    kicker.append(&gtk::Image::from_icon_name("object-select-symbolic"));
    kicker.append(
        &gtk::Label::builder()
            .label(tr("In use · {style}").replace("{style}", &style))
            .xalign(0.0)
            .css_classes(["caption-heading"])
            .build(),
    );
    words.append(&kicker);
    words.append(
        &gtk::Label::builder()
            .label(theme_name(snap, &label))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-2"])
            .build(),
    );
    if let Some(when) = applied_when(snap, &label) {
        words.append(
            &gtk::Label::builder()
                .label(when)
                .xalign(0.0)
                .wrap(true)
                .css_classes(["dim-label"])
                .build(),
        );
    }

    // A part turned on or off since takes effect when applied again.
    if pending(snap) {
        let apply = gtk::Button::builder()
            .label(tr("Apply _Changes"))
            .use_underline(true)
            .halign(gtk::Align::Start)
            .css_classes(["suggested-action"])
            .build();
        let weak = Rc::downgrade(ui);
        let spec = (id.to_owned(), variant.to_owned());
        apply.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                let spec = spec.clone();
                glib::spawn_future_local(async move { apply_theme(&ui, spec.0, spec.1).await });
            }
        });
        words.append(&apply);
    }
    // What can be done with it; the rest is in the same menu as its card's.
    if let Some(entry) = entry.filter(|_| resolved.is_some()) {
        let target = label.to_variant();
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(4)
            .build();
        let edit = icon_button(
            "document-edit-symbolic",
            &if entry.bundled {
                tr("_Duplicate to Edit")
            } else {
                tr("_Edit Theme")
            },
        );
        edit.set_action_name(Some("win.edit-theme"));
        edit.set_action_target_value(Some(&target));
        buttons.append(&edit);
        let export = icon_button("document-send-symbolic", &tr("E_xport…"));
        export.set_action_name(Some("win.export-theme"));
        export.set_action_target_value(Some(&target));
        buttons.append(&export);
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&theme_menu(&label, entry.bundled))
            .tooltip_text(tr("More"))
            .valign(gtk::Align::Center)
            .build();
        buttons.append(&more);
        // Stacked when narrow, each as wide as its label.
        for button in [
            edit.upcast_ref::<gtk::Widget>(),
            export.upcast_ref(),
            more.upcast_ref(),
        ] {
            button.set_halign(gtk::Align::Start);
        }
        page.when_narrowest(&buttons, "orientation", gtk::Orientation::Vertical);
        words.append(&buttons);
    }
    inner.append(&words);
    card.upcast()
}

/// Every theme in the library as a card; selecting one applies it.
/// Narrow, the buttons go under the heading.
fn theme_grid(ui: &U, snap: &Snapshot, page: &Column) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let title = gtk::Label::builder()
        .label(tr("Themes"))
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["title-4"])
        .accessible_role(gtk::AccessibleRole::Heading)
        .build();
    let filters = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["linked"])
        .valign(gtk::Align::Center)
        .build();
    filters.update_property(&[gtk::accessible::Property::Label(&tr("Show"))]);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    top.set_hexpand(true);
    top.append(&title);
    top.append(&filters);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let import = icon_button("document-open-symbolic", &tr("_Import…"));
    import.set_action_name(Some("win.import"));
    let new = icon_button("list-add-symbolic", &tr("_New Theme"));
    new.set_action_name(Some("win.new-theme"));
    actions.append(&import);
    actions.append(&new);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    header.append(&top);
    header.append(&actions);
    page.when_narrow(&header, "orientation", gtk::Orientation::Vertical);
    section.append(&header);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .column_spacing(12)
        .row_spacing(12)
        .min_children_per_line(2)
        .max_children_per_line(3)
        .valign(gtk::Align::Start)
        .build();
    // Light or not, per card, for the filter.
    let mut light = Vec::new();
    for entry in &snap.themes {
        for variant in entry.theme.variants.keys() {
            let Some(resolved) = Resolved::new(&entry.theme, Some(variant)) else {
                continue;
            };
            let spec = spec_of(&entry.theme.id, variant);
            let used = snap.applied.as_deref() == Some(spec.as_str());
            let card = theme_card(ui, entry, variant, &resolved, used);
            flow.insert(&card, -1);
            if let Some(child) = card.parent() {
                child.set_focusable(false);
            }
            light.push(matches!(
                resolved.colors.appearance,
                velatura_theme::Appearance::Light
            ));
        }
    }
    let none = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let filter = ui.filter.clone();
    let light = Rc::new(light);
    let shown = {
        let light = light.clone();
        move |which: Filter| match which {
            Filter::All => light.len(),
            Filter::Dark => light.iter().filter(|l| !**l).count(),
            Filter::Light => light.iter().filter(|l| **l).count(),
        }
    };
    {
        let filter = filter.clone();
        flow.set_filter_func(move |child| {
            let is_light = light.get(child.index() as usize).copied().unwrap_or(false);
            match filter.get() {
                Filter::All => true,
                Filter::Dark => !is_light,
                Filter::Light => is_light,
            }
        });
    }
    let say_none = {
        let none = none.clone();
        move |which: Filter| {
            none.set_label(&match which {
                Filter::Light => tr("No light themes"),
                _ => tr("No dark themes"),
            });
            none.set_visible(shown(which) == 0);
        }
    };
    say_none(filter.get());
    let say_none = Rc::new(say_none);
    let mut first: Option<gtk::ToggleButton> = None;
    for (which, label) in [
        (Filter::All, tr("All")),
        (Filter::Dark, tr("Dark")),
        (Filter::Light, tr("Light")),
    ] {
        let button = gtk::ToggleButton::builder()
            .label(label)
            .active(filter.get() == which)
            .build();
        match &first {
            Some(group) => button.set_group(Some(group)),
            None => first = Some(button.clone()),
        }
        let (filter, flow, say_none) = (filter.clone(), flow.clone(), say_none.clone());
        button.connect_toggled(move |button| {
            if button.is_active() {
                filter.set(which);
                flow.invalidate_filter();
                say_none(which);
            }
        });
        filters.append(&button);
    }
    section.append(&flow);
    section.append(&none);
    section
}

/// One theme as a card: its colours, its name and style, and a check mark
/// on the one in use. A menu holds what else can be done with it.
fn theme_card(
    ui: &U,
    entry: &Entry,
    variant: &str,
    resolved: &Resolved<'_>,
    used: bool,
) -> gtk::Widget {
    let name = resolved.colors.name.clone();
    let style = appearance_word(entry, variant);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&widgets::card_art(resolved));
    let bottom = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        // Room for the menu button laid over the card's corner.
        .margin_end(48)
        .build();
    let words = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    words.append(
        &gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build(),
    );
    words.append(
        &gtk::Label::builder()
            .label(&style)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    bottom.append(&words);
    if used {
        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .css_classes(["accent"])
            .tooltip_text(tr("In Use"))
            .valign(gtk::Align::Center)
            .build();
        bottom.append(&check);
    }
    content.append(&bottom);
    let button = gtk::Button::builder()
        .child(&content)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["card", "velatura-theme"])
        .build();
    if used {
        button.add_css_class("velatura-in-use");
    }
    let spoken = if used {
        tr("{name}, {style}, in use")
    } else {
        // Only the two names: nothing to translate.
        String::from("{name}, {style}")
    };
    button.update_property(&[gtk::accessible::Property::Label(
        &spoken.replace("{name}", &name).replace("{style}", &style),
    )]);
    let weak = Rc::downgrade(ui);
    let spec = (entry.theme.id.clone(), variant.to_owned());
    button.connect_clicked(move |_| {
        // The one in use is already applied; its card in use says how to
        // apply changes.
        if let (Some(ui), false) = (weak.upgrade(), used) {
            let spec = spec.clone();
            glib::spawn_future_local(async move { apply_theme(&ui, spec.0, spec.1).await });
        }
    });
    let spec = spec_of(&entry.theme.id, variant);
    card_menu(&button, &spec, entry.bundled);
    // The same menu, in sight: on the card, but not inside its button,
    // which applies the theme.
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&theme_menu(&spec, entry.bundled))
        .tooltip_text(tr("More"))
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .margin_end(6)
        .margin_bottom(9)
        .css_classes(["flat", "circular"])
        .build();
    more.update_property(&[gtk::accessible::Property::Label(
        &tr("More for {name}").replace("{name}", &name),
    )]);
    let card = gtk::Overlay::builder().child(&button).build();
    card.add_overlay(&more);
    card.upcast()
}

/// What else can be done with a theme, given its `theme/variant`.
fn theme_menu(spec: &str, bundled: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    let item = |label: String, action: &str| {
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some(action), Some(&spec.to_variant()));
        menu.append_item(&item);
    };
    item(tr("De_tails"), "win.theme-details");
    item(
        if bundled {
            tr("_Duplicate to Edit")
        } else {
            tr("_Edit Theme")
        },
        "win.edit-theme",
    );
    item(tr("E_xport…"), "win.export-theme");
    if !bundled {
        item(tr("_Remove Theme"), "win.remove-theme");
    }
    menu
}

/// The menu of a theme's card, opened from the card itself as well as its
/// menu button: with the secondary button, a long press, or the Menu key
/// or Shift+F10.
fn card_menu(button: &gtk::Button, spec: &str, bundled: bool) {
    let menu = theme_menu(spec, bundled);
    let show = Rc::new(move |button: &gtk::Widget, at: Option<(f64, f64)>| {
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(button);
        if let Some((x, y)) = at {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        // Made for each opening, and let go of once closed.
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    });
    let click = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    let open = show.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        if let Some(widget) = gesture.widget() {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open(&widget, Some((x, y)));
        }
    });
    button.add_controller(click);
    let press = gtk::GestureLongPress::builder().touch_only(true).build();
    let open = show.clone();
    press.connect_pressed(move |gesture, x, y| {
        if let Some(widget) = gesture.widget() {
            open(&widget, Some((x, y)));
        }
    });
    button.add_controller(press);
    let keys = gtk::ShortcutController::new();
    let open = show;
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Menu|<Shift>F10"),
        Some(gtk::CallbackAction::new(move |widget, _| {
            open(widget, None);
            glib::Propagation::Stop
        })),
    ));
    button.add_controller(keys);
}

/// How the pair of themes switches by itself, if it does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Auto {
    Off,
    Clock,
    Style,
}

/// The pair of themes for the day and the night, the button that goes from
/// one to the other, and how they switch by themselves.
/// Narrow, the three ways of switching go under their title.
fn day_night_section(ui: &U, snap: &Snapshot, page: &Column) -> gtk::Box {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Day and Night"))
        .description(tr(
            "A pair of themes to switch between by hand or on a schedule",
        ))
        .build();
    // Every theme and variant, as the engine names them and as shown.
    let choices: Vec<(
        String,
        String,
        Option<[velatura_theme::Color; 4]>,
        velatura_theme::Appearance,
    )> = snap
        .themes
        .iter()
        .flat_map(|e| {
            e.theme.variants.iter().map(move |(key, v)| {
                let spec = if key == e.theme.default_variant() {
                    e.theme.id.clone()
                } else {
                    spec_of(&e.theme.id, key)
                };
                let dots = Resolved::new(&e.theme, Some(key)).map(|r| {
                    let c = widgets::palette(&r);
                    [c[2], c[3], c[4], c[5]]
                });
                (spec, v.name.clone(), dots, v.appearance)
            })
        })
        .collect();
    let mut names = vec![tr("None")];
    names.extend(choices.iter().map(|(_, name, ..)| name.clone()));
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut dots = vec![None];
    dots.extend(choices.iter().map(|(_, _, d, _)| *d));
    let dots = Rc::new(dots);
    let appearance = |spec: &Option<String>| {
        let spec = spec.as_ref()?;
        choices.iter().find(|(s, ..)| s == spec).map(|(.., a)| *a)
    };

    for (which, title, icon, current) in [
        (
            "day",
            tr("_Daytime Theme"),
            "weather-clear-symbolic",
            &snap.day_night.day,
        ),
        (
            "night",
            tr("_Nighttime Theme"),
            "weather-clear-night-symbolic",
            &snap.day_night.night,
        ),
    ] {
        let selected = current
            .as_ref()
            .and_then(|spec| choices.iter().position(|(s, ..)| s == spec))
            .map_or(0, |i| i + 1);
        let model = gtk::StringList::new(&names);
        let row = adw::ComboRow::builder()
            .title(title)
            .use_underline(true)
            .factory(&theme_factory(&model, dots.clone()))
            .model(&model)
            .selected(selected as u32)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        let weak = Rc::downgrade(ui);
        let specs: Vec<String> = choices.iter().map(|(s, ..)| s.clone()).collect();
        row.connect_selected_notify(move |row| {
            let picked = (row.selected() as usize)
                .checked_sub(1)
                .and_then(|i| specs.get(i).cloned());
            if let Some(ui) = weak.upgrade() {
                glib::spawn_future_local(async move {
                    let saved = data::run(move |host| {
                        let mut chosen = DayNight::load(host);
                        if which == "day" {
                            chosen.day = picked;
                        } else {
                            chosen.night = picked;
                        }
                        chosen.save(host)
                    })
                    .await;
                    if !matches!(saved, Some(Ok(()))) {
                        toast(&ui, &tr("The choice could not be saved"));
                    }
                    refresh(&ui).await;
                });
            }
        });
        group.add(&row);
    }

    // The button names where it goes: the one of the two not in use.
    let is = |spec: &Option<String>| {
        spec.as_ref()
            .zip(snap.applied.as_ref())
            .is_some_and(|(spec, label)| {
                label == spec || (!spec.contains('/') && label.split('/').next() == Some(spec))
            })
    };
    let to_night = is(&snap.day_night.day) || !is(&snap.day_night.night);
    let (next, icon, label) = if to_night && snap.day_night.night.is_some() {
        (
            snap.day_night.night.clone(),
            "weather-clear-night-symbolic",
            tr("S_witch to Night"),
        )
    } else {
        (
            snap.day_night.day.clone(),
            "weather-clear-symbolic",
            tr("S_witch to Day"),
        )
    };
    let switch = icon_button(icon, &label);
    switch.set_sensitive(next.is_some() && !is(&next));
    let weak = Rc::downgrade(ui);
    switch.connect_clicked(move |_| {
        if let (Some(ui), Some(spec)) = (weak.upgrade(), next.clone()) {
            glib::spawn_future_local(async move {
                let (id, variant) = match spec.split_once('/') {
                    Some((id, v)) => (id.to_owned(), Some(v.to_owned())),
                    None => (spec.clone(), None),
                };
                let variant = variant.or_else(|| {
                    ui.snap
                        .borrow()
                        .themes
                        .iter()
                        .find(|e| e.theme.id == id)
                        .map(|e| e.theme.default_variant().to_owned())
                });
                match variant {
                    Some(variant) => apply_theme(&ui, id, variant).await,
                    None => toast(&ui, &tr("That theme is no longer available")),
                }
            });
        }
    });
    group.set_header_suffix(Some(&switch));

    // One choice of three: by hand, by the clock, or with the dark style.
    let status = &snap.schedule;
    let mode = match (status.on, status.follows_style) {
        (false, _) => Auto::Off,
        (true, false) => Auto::Clock,
        (true, true) => Auto::Style,
    };
    let chosen = snap.day_night.day.is_some() && snap.day_night.night.is_some();
    // Why a way of switching cannot be chosen now, if it cannot.
    let blocked = if !snap.can_schedule {
        Some(tr(
            "Needs the velatura command installed, for example in ~/.local/bin",
        ))
    } else if !chosen {
        Some(tr("Choose a daytime and a nighttime theme first"))
    } else {
        None
    };
    let pair = appearance(&snap.day_night.day) == Some(velatura_theme::Appearance::Light)
        && appearance(&snap.day_night.night) == Some(velatura_theme::Appearance::Dark);
    let style_blocked = blocked
        .clone()
        .or_else(|| (!pair).then(|| tr("Needs a light daytime theme and a dark nighttime theme")));
    let hint = if status.on && status.damaged {
        tr("The timer was changed outside Velatura. Turn this off and on to repair it")
    } else {
        match (mode, &blocked) {
            (Auto::Off, Some(why)) => why.clone(),
            (Auto::Off, None) => tr("Switch by hand with the button above"),
            (Auto::Clock, _) => tr("Changes between the two themes by the clock"),
            (Auto::Style, _) => tr(
                "Uses the daytime theme with the light style and the nighttime theme with the dark style",
            ),
        }
    };
    // Laid out like an action row, which cannot put its suffix under its
    // title when narrow.
    let texts = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title = gtk::Label::builder()
        .label(tr("Switch Automatically"))
        .xalign(0.0)
        .wrap(true)
        .build();
    texts.append(&title);
    texts.append(
        &gtk::Label::builder()
            .label(hint)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    let ways = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["linked"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Start)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body.append(&texts);
    body.append(&ways);
    page.when_narrowest(&body, "orientation", gtk::Orientation::Vertical);
    let line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    line.append(
        &gtk::Image::builder()
            .icon_name("preferences-system-time-symbolic")
            .valign(gtk::Align::Center)
            .build(),
    );
    line.append(&body);
    let auto = adw::PreferencesRow::builder()
        .title(tr("Switch Automatically"))
        .activatable(false)
        .child(&line)
        .build();
    ways.update_property(&[gtk::accessible::Property::Label(&tr(
        "Switch Automatically",
    ))]);
    let mut first: Option<gtk::ToggleButton> = None;
    for (way, label, why) in [
        (Auto::Off, tr("_Off"), None),
        (Auto::Clock, tr("_Schedule"), blocked.clone()),
        (Auto::Style, tr("Dark St_yle"), style_blocked),
    ] {
        // The way in use can always be chosen again, or left.
        let why = why.filter(|_| mode != way);
        let button = gtk::ToggleButton::builder()
            .label(label)
            .use_underline(true)
            .active(mode == way)
            .sensitive(why.is_none())
            .build();
        button.set_tooltip_text(why.as_deref());
        match &first {
            Some(group) => button.set_group(Some(group)),
            None => first = Some(button.clone()),
        }
        let weak = Rc::downgrade(ui);
        button.connect_toggled(move |button| {
            let Some(ui) = weak.upgrade().filter(|_| button.is_active()) else {
                return;
            };
            glib::spawn_future_local(async move {
                match way {
                    Auto::Off => set_schedule(&ui, false, None, false).await,
                    Auto::Clock => set_schedule(&ui, true, None, false).await,
                    Auto::Style => set_schedule(&ui, true, None, true).await,
                }
            });
        });
        ways.append(&button);
    }
    group.add(&auto);

    // The times, only while the clock leads.
    if mode == Auto::Clock {
        for (is_day, title, icon, minutes) in [
            (
                true,
                tr("Day _Begins"),
                "weather-clear-symbolic",
                status.times.day,
            ),
            (
                false,
                tr("Night B_egins"),
                "weather-clear-night-symbolic",
                status.times.night,
            ),
        ] {
            let row = adw::EntryRow::builder()
                .title(title)
                .use_underline(true)
                .text(schedule::format_time(minutes))
                .input_purpose(gtk::InputPurpose::Digits)
                .show_apply_button(true)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            // Feedback while typing: an entry that is not a time is marked.
            row.connect_changed(|row| {
                if schedule::parse_time(&row.text()).is_some() {
                    row.remove_css_class("error");
                } else {
                    row.add_css_class("error");
                }
            });
            let weak = Rc::downgrade(ui);
            row.connect_apply(move |row| {
                let (Some(ui), Some(minutes)) = (weak.upgrade(), schedule::parse_time(&row.text()))
                else {
                    return;
                };
                glib::spawn_future_local(async move {
                    let mut times = ui.snap.borrow().schedule.times;
                    if is_day {
                        times.day = minutes;
                    } else {
                        times.night = minutes;
                    }
                    set_schedule(&ui, true, Some(times), false).await;
                });
            });
            group.add(&row);
        }
    }

    let note = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    note.append(
        &gtk::Image::builder()
            .icon_name("dialog-information-symbolic")
            .valign(gtk::Align::Start)
            .css_classes(["dim-label"])
            .build(),
    );
    note.append(
        &gtk::Label::builder()
            .label(tr(
                "Apps that are already open keep their colors until reopened",
            ))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    let section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    section.append(&group);
    section.append(&note);
    section
}

/// Each theme's name in a drop-down, after four of its colours. `dots`
/// follows the order of `model`.
fn theme_factory(
    model: &gtk::StringList,
    dots: Rc<Vec<Option<[velatura_theme::Color; 4]>>>,
) -> gtk::SignalListItemFactory {
    let model = model.downgrade();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.append(&widgets::small_dots());
        line.append(
            &gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        item.set_child(Some(&line));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(line) = item.child() else {
            return;
        };
        let Some(shown) = item.item() else {
            return;
        };
        let name = shown
            .downcast_ref::<gtk::StringObject>()
            .map(|s| s.string().to_string())
            .unwrap_or_default();
        // The row's own display of the choice has no position in the list:
        // the item is looked up instead.
        let at = model.upgrade().and_then(|model| {
            (0..model.n_items()).find(|i| model.item(*i).as_ref() == Some(&shown))
        });
        if let Some(area) = line.first_child().and_downcast::<gtk::DrawingArea>() {
            widgets::paint_dots(
                &area,
                at.and_then(|i| dots.get(i as usize).copied().flatten()),
            );
        }
        if let Some(label) = line.last_child().and_downcast::<gtk::Label>() {
            label.set_label(&name);
        }
    });
    factory
}

/// Turn the schedule on or off, or give a running one new times.
async fn set_schedule(ui: &U, on: bool, times: Option<Times>, follow: bool) {
    let home = ui.snap.borrow().home.clone();
    let times = times.unwrap_or(ui.snap.borrow().schedule.times);
    let command = ui.snap.borrow().command.clone();
    let mut anyway = false;
    loop {
        let command = command.clone();
        let done = data::run(move |host| match (on, command) {
            (true, Some(command)) if follow => {
                schedule::enable_follow(host, &StdSystemd, &command, anyway)
            }
            (true, Some(command)) => schedule::enable(host, &StdSystemd, &command, times, anyway),
            (true, None) => Err(Refused::NotInstalled("velatura".into())),
            (false, _) => schedule::disable(host, &StdSystemd).map(|_| ()),
        })
        .await;
        match done {
            Some(Ok(())) => toast(
                ui,
                &if on {
                    tr("Schedule turned on")
                } else {
                    tr("Schedule turned off")
                },
            ),
            // Two schedules would undo each other's work: ask first.
            Some(Err(Refused::OtherTimers(units))) => {
                let dialog = adw::AlertDialog::new(
                    Some(&tr("Turn the Schedule On?")),
                    Some(
                        &tr("Another timer that may change the theme is turned on: {units}. Two schedules would work against each other.")
                            .replace("{units}", &units.join(", ")),
                    ),
                );
                dialog.add_responses(&[("cancel", &tr("_Cancel")), ("on", &tr("_Turn On"))]);
                dialog.set_close_response("cancel");
                if dialog.choose_future(Some(&ui.window)).await == "on" {
                    anyway = true;
                    continue;
                }
            }
            Some(Err(why)) => toast(ui, &velatura_text::refused(&why, &home)),
            None => toast(ui, &tr("The change could not be started")),
        }
        break;
    }
    refresh(ui).await;
}

/// What else can be done with a theme, below its facts: edit it (one that
/// comes with Velatura, as a copy), pass its file on, or remove it.
fn theme_actions(ui: &U, id: &str, variant: &str, bundled: bool) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    let row = |title: String, run: fn(U, String, String)| {
        let row = adw::ButtonRow::builder()
            .title(title)
            .use_underline(true)
            .build();
        let weak = Rc::downgrade(ui);
        let spec = (id.to_owned(), variant.to_owned());
        row.connect_activated(move |_| {
            if let Some(ui) = weak.upgrade() {
                run(ui, spec.0.clone(), spec.1.clone());
            }
        });
        row
    };
    group.add(&row(
        if bundled {
            tr("_Duplicate to Edit")
        } else {
            tr("_Edit Theme")
        },
        |ui, id, variant| editor::edit(&ui, &id, &variant),
    ));
    group.add(&row(tr("E_xport…"), |ui, id, _| {
        glib::spawn_future_local(async move { export_theme(&ui, id).await });
    }));
    if !bundled {
        group.add(&row(tr("_Remove Theme"), |ui, id, _| {
            glib::spawn_future_local(async move { remove_theme(&ui, id).await });
        }));
    }
    group
}

/// Velatura's own theme files, and `also` for what else can be read.
fn toml_filter(also: &[&str]) -> gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&tr("Theme Files")));
    filter.add_pattern("*.toml");
    for other in also {
        filter.add_pattern(other);
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    filters
}

/// Write a theme's file where the user says, to pass on to someone else.
async fn export_theme(ui: &U, id: String) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Export Theme"))
        .initial_name(format!("{id}.toml"))
        .filters(&toml_filter(&[]))
        .build();
    // Cancelling is not an error.
    let Some(path) = dialog
        .save_future(Some(&ui.window))
        .await
        .ok()
        .and_then(|file| file.path())
    else {
        return;
    };
    let home = ui.snap.borrow().home.clone();
    // The file chooser has already asked about replacing a file.
    match data::run(move |host| library::export(host, &id, &path, true)).await {
        Some(Ok(())) => toast(ui, &tr("Theme exported")),
        Some(Err(why)) => toast(ui, &velatura_text::library_refused(&why, &home)),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// Undoable, so it is done at once and Undo is offered.
async fn remove_theme(ui: &U, id: String) {
    let home = ui.snap.borrow().home.clone();
    let removed = data::run(move |host| library::remove(host, &id)).await;
    match removed {
        Some(Ok(text)) => {
            refresh(ui).await;
            ui.nav.pop_to_tag("main");
            let undo = adw::Toast::builder()
                .title(tr("Theme removed"))
                .button_label(tr("_Undo"))
                .build();
            let weak = Rc::downgrade(ui);
            undo.connect_button_clicked(move |_| {
                let (Some(ui), text) = (weak.upgrade(), text.clone()) else {
                    return;
                };
                glib::spawn_future_local(async move {
                    let back = data::run(move |host| {
                        let theme = velatura_theme::parse_theme(&text).ok()?.theme;
                        library::save(host, &theme, Some(&text), false).ok()
                    })
                    .await;
                    if back.flatten().is_none() {
                        toast(&ui, &tr("The theme could not be put back"));
                    }
                    refresh(&ui).await;
                });
            });
            ui.toasts.add_toast(undo);
        }
        Some(Err(why)) => toast(ui, &velatura_text::library_refused(&why, &home)),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// Add a theme file to the library: look at it, show what it is, and ask.
async fn import_theme(ui: &U) {
    let dialog = gtk::FileDialog::builder()
        .title(tr("Import Theme"))
        .filters(&toml_filter(&["*.yaml", "*.yml", "*.conf"]))
        .build();
    let Some(path) = dialog
        .open_future(Some(&ui.window))
        .await
        .ok()
        .and_then(|file| file.path())
    else {
        return;
    };
    import_file(ui, path).await;
}

/// Look at a file as a theme, ask, and put it in the library.
async fn import_file(ui: &U, path: std::path::PathBuf) {
    let home = ui.snap.borrow().home.clone();
    let looked = data::run(move |host| library::inspect(host, &path)).await;
    let (loaded, mut source, from, accents, usual) = match looked {
        Some(Ok(found)) => (
            found.loaded,
            found.source,
            found.from,
            found.accents,
            found.accent,
        ),
        Some(Err(why)) => {
            toast(ui, &velatura_text::library_refused(&why, &home));
            return;
        }
        None => return,
    };
    let mut theme = loaded.theme;
    let clash = ui
        .snap
        .borrow()
        .themes
        .iter()
        .find(|e| e.theme.id == theme.id)
        .map(|e| e.bundled);

    // Name and author are shown as plain text, never as markup.
    let mut body = if theme.author.is_empty() {
        String::new()
    } else {
        format!(
            "{}\n\n",
            tr("By {author}.").replace("{author}", &theme.author)
        )
    };
    if let Some(format) = from {
        body.push_str(&velatura_text::imported_from(format));
        body.push_str(".\n\n");
    }
    body.push_str(&velatura_text::import_promise());
    body.push('.');
    if clash.is_some() {
        body.push_str("\n\n");
        body.push_str(&tr("A theme with the same id is already in the library."));
    }
    let alert = adw::AlertDialog::new(
        Some(&tr("Import {name}?").replace("{name}", &theme.name)),
        Some(&body),
    );
    // A file in another format names no accent: any of its colours can
    // be it, chosen here, with the strip above showing the result.
    let chosen = Rc::new(Cell::new(usual));
    if let Some(resolved) = Resolved::new(&theme, None) {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        let strip = adw::Bin::builder().halign(gtk::Align::Center).build();
        strip.set_child(Some(&widgets::swatches(&resolved)));
        column.append(&strip);
        if accents.len() > 1 {
            let title = gtk::Label::builder()
                .label(tr("Accent Color"))
                .css_classes(["heading"])
                .build();
            column.append(&title);
            // Up to eight in one line; more, as a base24 scheme has, in
            // lines of seven.
            let per_line = if accents.len() <= 8 { accents.len() } else { 7 } as u32;
            let choices = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .max_children_per_line(per_line)
                .min_children_per_line(per_line)
                .homogeneous(true)
                .column_spacing(4)
                .row_spacing(4)
                .halign(gtk::Align::Center)
                .build();
            choices
                .update_relation(&[gtk::accessible::Relation::LabelledBy(&[title.upcast_ref()])]);
            let outline = resolved.colors.ui.border;
            let mut first: Option<gtk::ToggleButton> = None;
            for (i, (name, color)) in accents.iter().enumerate() {
                // The name is one of the format's own, not text of the file.
                let said = format!("{name} {}", color.hex());
                let marked = Rc::new(Cell::new(i == usual));
                let dot = widgets::dot(*color, outline, marked.clone());
                let button = gtk::ToggleButton::builder()
                    .child(&dot)
                    .tooltip_text(&said)
                    .css_classes(["flat", "circular"])
                    .active(i == usual)
                    .build();
                button.update_property(&[gtk::accessible::Property::Label(&said)]);
                match &first {
                    Some(group) => button.set_group(Some(group)),
                    None => first = Some(button.clone()),
                }
                let (chosen, strip, theme, color) =
                    (chosen.clone(), strip.clone(), theme.clone(), *color);
                button.connect_toggled(move |button| {
                    marked.set(button.is_active());
                    dot.queue_draw();
                    if !button.is_active() {
                        return;
                    }
                    chosen.set(i);
                    if let Some(with) = velatura_theme::import::with_accent(&theme, color)
                        && let Some(resolved) = Resolved::new(&with, None)
                    {
                        strip.set_child(Some(&widgets::swatches(&resolved)));
                    }
                });
                choices.insert(&button, -1);
            }
            column.append(&choices);
        }
        alert.set_extra_child(Some(&column));
    }
    alert.add_response("cancel", &tr("_Cancel"));
    match clash {
        None => {
            alert.add_response("import", &tr("_Import"));
            alert.set_response_appearance("import", adw::ResponseAppearance::Suggested);
            alert.set_default_response(Some("import"));
        }
        Some(bundled) => {
            alert.add_response("both", &tr("_Keep Both"));
            alert.set_default_response(Some("both"));
            if !bundled {
                alert.add_response("replace", &tr("_Replace"));
            }
        }
    }
    alert.set_close_response("cancel");
    let answer = alert.choose_future(Some(&ui.window)).await;
    if answer == "cancel" {
        return;
    }
    if chosen.get() != usual
        && let Some((_, color)) = accents.get(chosen.get())
        && let Some(with) = velatura_theme::import::with_accent(&theme, *color)
    {
        source = velatura_theme::model::to_toml(&with);
        theme = with;
    }
    let replace = answer == "replace";
    let keep_both = answer == "both";
    let saved = data::run(move |host| {
        if keep_both {
            theme.id = velatura_theme::editor::free_id(&theme.id, &|id| library::taken(host, id));
        }
        library::save(host, &theme, Some(&source), replace)
    })
    .await;
    match saved {
        Some(Ok(_)) => {
            refresh(ui).await;
            toast(ui, &tr("Theme imported"));
        }
        Some(Err(why)) => toast(ui, &velatura_text::library_refused(&why, &home)),
        None => toast(ui, &tr("The change could not be started")),
    }
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
            .subtitle(glib::markup_escape_text(&value))
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
    // What the theme's author suggests to go with it. Only a name, matched
    // against what is installed; used only if the user says so.
    for (key, kind, title) in [
        ("icon_theme", Kind::Icons, tr("Suggested Icons")),
        ("cursor_theme", Kind::Cursor, tr("Suggested Cursor")),
    ] {
        let Some(name) = resolved.colors.suggests.get(key) else {
            continue;
        };
        let look = snap.look.iter().find(|(k, ..)| *k == kind);
        let installed = look.is_some_and(|(_, _, found)| found.iter().any(|c| c.value == *name));
        let in_use = look.is_some_and(|(_, current, _)| current == name);
        let row = fact(
            title,
            if in_use {
                format!("{name} · {}", tr("In use"))
            } else if installed {
                name.clone()
            } else {
                format!("{name} · {}", tr("Not installed"))
            },
        );
        if installed && !in_use {
            let button = gtk::Button::builder()
                .label(tr("Use"))
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(ui);
            let name = name.clone();
            button.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    let name = name.clone();
                    glib::spawn_future_local(async move {
                        choose_look(&ui, kind, name).await;
                        ui.nav.pop_to_tag("main");
                    });
                }
            });
            row.add_suffix(&button);
        }
        facts.add(&row);
    }

    // The theme in use can be applied again when a part was turned on or
    // off since, which is the only way that change takes effect.
    let pending = used && pending(&snap);
    let apply = gtk::Button::builder()
        .label(if pending {
            tr("Apply _Changes")
        } else if used {
            tr("Applied")
        } else {
            tr("_Apply Theme")
        })
        .use_underline(true)
        .sensitive(!used || pending)
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
    column.append(&theme_actions(ui, id, variant, entry.bundled));
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
        Error::UnreadableBackup(_) | Error::ForeignBackup { .. } => {
            velatura_text::backup_refused(e).unwrap_or_default()
        }
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

/// A page that says Velatura is at work, put up as soon as the work
/// starts and kept long enough to be read, so even a quick change shows
/// that something happened.
struct Working {
    ui: U,
    page: adw::NavigationPage,
    status: adw::StatusPage,
    since: std::time::Instant,
}

/// The shortest time the page stays up.
const WORKING_AT_LEAST: std::time::Duration = std::time::Duration::from_millis(700);

impl Working {
    fn new(ui: &U, title: &str, detail: Option<String>) -> Self {
        let spinner = adw::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::Center)
            .build();
        let status = adw::StatusPage::builder()
            .title(glib::markup_escape_text(title))
            .child(&spinner)
            .build();
        status.set_description(detail.as_deref());
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::builder().show_back_button(false).build());
        view.set_content(Some(&status));
        let page = adw::NavigationPage::builder()
            .title(title)
            .tag("working")
            .can_pop(false)
            .child(&view)
            .build();
        ui.nav.push(&page);
        Self {
            ui: ui.clone(),
            page,
            status,
            since: std::time::Instant::now(),
        }
    }

    /// Say what is being done now, on the same page.
    fn retitle(&self, title: &str) {
        self.status.set_title(&glib::markup_escape_text(title));
        self.status.set_description(None);
        self.page.set_title(title);
    }

    /// Take the page away, once it has been up long enough to be read.
    async fn done(&self) {
        if let Some(rest) = WORKING_AT_LEAST.checked_sub(self.since.elapsed()) {
            glib::timeout_future(rest).await;
        }
        // The page on show is popped: removing it would leave it there.
        if self.ui.nav.visible_page().as_ref() == Some(&self.page) {
            self.ui.nav.pop();
        } else {
            self.ui.nav.remove(&self.page);
        }
    }
}

/// True if applying may recolour icons or cursors: either part is
/// included and can be themed. Making a set the first time takes seconds.
fn recolors(snap: &Snapshot) -> bool {
    snap.rows.iter().any(|r| {
        matches!(r.instance.as_str(), targets::ICONS | targets::CURSORS)
            && r.included
            && !matches!(r.state, RowState::Unavailable(_))
    })
}

/// Apply a theme: plan, confirm when the guidelines call for it, execute,
/// and say what happened.
async fn apply_theme(ui: &U, id: String, variant: String) {
    if busy(ui) {
        return;
    }
    let first = ui.snap.borrow().applied.is_none();
    let home = ui.snap.borrow().home.clone();
    let detail = recolors(&ui.snap.borrow()).then(|| {
        tr("Recoloring icons takes some seconds the first time. Velatura keeps the result, so coming back to these colors is quick")
    });
    let shown_name = ui
        .snap
        .borrow()
        .themes
        .iter()
        .find(|e| e.theme.id == id)
        .map(|e| e.theme.name.clone())
        .unwrap_or_default();
    let applying = tr("Applying {name}…").replace("{name}", &shown_name);
    // Getting ready is where icons are recoloured, before anything is
    // asked or changed.
    let work = Working::new(
        ui,
        &tr("Preparing {name}…").replace("{name}", &shown_name),
        detail,
    );
    ui.busy.set(true);
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
    ui.busy.set(false);
    let Some((name, plan)) = planned else {
        work.done().await;
        toast(ui, &tr("That theme is no longer available"));
        return;
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(e) => {
            work.done().await;
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
        work.done().await;
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
    // Before the very first apply, say what else changes the same colours.
    let tools = if first {
        data::run(targets::other_tools).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    // The question is asked over the main view, not over a page saying
    // the theme is being applied; that page comes back once agreed to.
    let work = if first || !asking.is_empty() || low {
        work.done().await;
        if !confirm(ui, &name, &plan, &home, low, &tools).await {
            return;
        }
        Working::new(ui, &applying, None)
    } else {
        work.retitle(&applying);
        work
    };

    ui.busy.set(true);
    let executed = data::run(move |host| engine::execute(host, &plan, &asking)).await;
    ui.busy.set(false);
    refresh(ui).await;
    work.done().await;
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

async fn confirm(
    ui: &U,
    name: &str,
    plan: &Plan,
    home: &Path,
    low: bool,
    tools: &[targets::OtherTool],
) -> bool {
    let mut body = words::summary(words::count(plan));
    for tool in tools {
        body = format!("{body}\n\n{}.", velatura_text::other_tool(tool, home));
    }
    // Neovim's settings choosing another colourscheme is a question of its
    // own: say what the added line does, not only that a line is added.
    for note in plan
        .targets
        .iter()
        .filter(|t| t.skipped.is_none() && t.needs_consent)
        .flat_map(|t| &t.notes)
        .filter(|n| matches!(n, Note::NeovimOtherScheme { .. }))
    {
        body = format!("{body}\n\n{}.", words::note(note, home));
    }
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
            velatura_text::changes(&t.changes, home)
                .into_iter()
                .map(|line| format!("  {line}")),
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
                Skip::NotSelected | Skip::NotInstalled | Skip::NeedsAdwGtk3 | Skip::NeedsUserThemes
            )
        })
        .map(|(instance, why)| {
            format!(
                "{}: {}",
                velatura_text::instance(instance),
                words::reason(&words::skip(why, home))
            )
        })
        .collect();
    lines.extend(
        report
            .notes
            .iter()
            .filter(|(_, n)| worth_showing(n))
            .map(|(_, n)| words::note(n, home)),
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
    let done = data::run(engine::undo_last).await;
    ui.busy.set(false);
    refresh(ui).await;
    ui.nav.pop_to_tag("main");
    match done {
        Some(Ok(reports)) => {
            let kept: Vec<String> = reports
                .iter()
                .flat_map(|report| &report.notes)
                .filter(|(_, n)| {
                    matches!(
                        n,
                        Note::KeptFile { .. }
                            | Note::KeptSetting { .. }
                            | Note::SettingNeedsSession { .. }
                    )
                })
                .map(|(_, n)| words::note(n, &home))
                .collect();
            if kept.is_empty() {
                toast(ui, &tr("Last change undone"));
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

/// The symbolic icon shown for a part.
fn part_icon(instance: &str) -> &'static str {
    match instance {
        targets::APPEARANCE => "preferences-color-symbolic",
        targets::SHELL => "video-display-symbolic",
        targets::ICONS => "folder-symbolic",
        targets::CURSORS => "input-mouse-symbolic",
        targets::TOP_BAR => "focus-top-bar-symbolic",
        targets::UPS => "uninterruptible-power-supply-symbolic",
        targets::OUTLINE => "focus-windows-symbolic",
        targets::KITTY | targets::GHOSTTY | targets::PTYXIS => "utilities-terminal-symbolic",
        targets::TMUX => "view-grid-symbolic",
        targets::STARSHIP => "starred-symbolic",
        targets::GTK4 => "applications-system-symbolic",
        targets::GTK3 => "application-x-executable-symbolic",
        targets::TEXT_EDITOR => "text-editor-symbolic",
        targets::FIREFOX => "web-browser-symbolic",
        targets::FLATPAK => "application-x-addon-symbolic",
        _ => "text-x-generic-symbolic",
    }
}

/// A short word beside a part's name, with more in its tooltip.
fn tag(text: &str, icon: Option<&str>, more: Option<&str>) -> gtk::Box {
    let tag = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .valign(gtk::Align::Center)
        .css_classes(["velatura-tag"])
        .build();
    if let Some(icon) = icon {
        tag.append(&gtk::Image::builder().icon_name(icon).pixel_size(12).build());
    }
    tag.append(
        &gtk::Label::builder()
            .label(text)
            .css_classes(["caption"])
            .build(),
    );
    if let Some(more) = more {
        tag.set_tooltip_text(Some(more));
        tag.update_property(&[gtk::accessible::Property::Description(more)]);
    }
    tag
}

/// True if a part can be themed now, whether or not it is turned on.
fn available(row: &data::AppRow) -> bool {
    !matches!(row.state, RowState::Unavailable(_))
}

/// One app or desktop part, or template, with its switch.
/// `place` is where a template's output goes, said under its name.
/// Narrow, a part that cannot be themed keeps only its words and button.
fn part_row(
    ui: &U,
    snap: &Snapshot,
    page: &Column,
    row: &data::AppRow,
    title: &str,
    place: Option<&str>,
) -> adw::ActionRow {
    // The switch says whether a part follows the theme; a line under the
    // name only says what the switch cannot.
    let subtitle = match &row.state {
        RowState::Themed if row.included => None,
        // Turned off, with Velatura's colors still on it.
        RowState::Themed => Some(tr("Taken away when a theme is next applied")),
        RowState::Ready if row.included => Some(tr("Included when a theme is next applied")),
        // Off until turned on, so the row says what turning on does.
        RowState::Ready if row.instance == targets::ICONS => Some(tr(
            "Gives the folders and other marked icons of the icon theme in use the theme's accent color. Making them takes some seconds the first time; open apps show them once reopened, and everything after signing out and in",
        )),
        RowState::Ready if row.instance == targets::CURSORS => Some(tr(
            "Gives the cursor in use the theme's colors: its body and outline, and its accent",
        )),
        RowState::Ready if row.instance == targets::TOP_BAR => Some(tr(
            "Draws the top bar as one full bar or as a pill per button, in the theme's colors and as opaque as chosen",
        )),
        RowState::Ready if row.instance == targets::UPS => Some(tr(
            "Shows in the top bar the watts a UPS draws, with its battery and runtime in a menu. It reads Network UPS Tools on this computer, and stays hidden while no UPS answers",
        )),
        RowState::Ready if row.instance == targets::OUTLINE => Some(tr(
            "Draws a thin line in the theme's accent color around the window in use, except when it is maximized or full screen",
        )),
        RowState::Ready if row.instance == targets::FIREFOX => Some(tr(
            "Gives the window of Firefox the theme's colors, through a stylesheet in its default profile",
        )),
        RowState::Ready if row.instance == targets::FLATPAK => Some(tr(
            "Lets every Flatpak app read the two GTK settings folders, so the colors reach them",
        )),
        RowState::Ready => None,
        RowState::Shadowed(note) => Some(format!(
            "{}. {}",
            tr("Set up, but not taking effect"),
            words::note(note, &snap.home)
        )),
        RowState::Unavailable(why) => Some(words::reason(&words::skip(why, &snap.home))),
    };
    let can = available(row);
    let subtitle = match (subtitle, place.filter(|p| !p.is_empty())) {
        (Some(s), Some(p)) => Some(format!("{s} · {p}")),
        (s, p) => s.or(p.map(str::to_owned)),
    };
    let action = adw::ActionRow::builder()
        .title(glib::markup_escape_text(title))
        .subtitle(glib::markup_escape_text(&subtitle.unwrap_or_default()))
        .subtitle_lines(0)
        .build();
    action.add_prefix(
        &gtk::Image::builder()
            .icon_name(part_icon(&row.instance))
            .valign(gtk::Align::Center)
            .css_classes(["velatura-tile"])
            .build(),
    );
    let unofficial = match row.instance.as_str() {
        targets::GTK4 | targets::GTK3 | targets::SHELL => {
            Some(tr("May stop working after a GNOME update"))
        }
        targets::FIREFOX => Some(tr("May stop working after a Firefox update")),
        _ => None,
    };
    if let Some(more) = unofficial.filter(|_| can) {
        action.add_suffix(&tag(
            &tr("Unofficial"),
            Some("dialog-warning-symbolic"),
            Some(&more),
        ));
    }
    if let RowState::Unavailable(why) = &row.state {
        action.add_css_class("velatura-unavailable");
        if matches!(why, Skip::NotInstalled | Skip::NeedsAdwGtk3) {
            let missing = tag(&tr("Not installed"), None, None);
            page.when_narrowest(&missing, "visible", false);
            action.add_suffix(&missing);
        }
        if matches!(why, Skip::NeedsAdwGtk3) {
            let how = gtk::Button::builder()
                .label(tr("_How to Install"))
                .use_underline(true)
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(ui);
            how.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    gtk::UriLauncher::new(ADW_GTK3).launch(
                        Some(&ui.window),
                        None::<&gio::Cancellable>,
                        |_| {},
                    );
                }
            });
            action.add_suffix(&how);
        }
    }
    let switch = gtk::Switch::builder()
        .active(row.included && can)
        .sensitive(can)
        .valign(gtk::Align::Center)
        .build();
    switch.update_property(&[gtk::accessible::Property::Label(title)]);
    action.add_suffix(&switch);
    if can {
        action.set_activatable_widget(Some(&switch));
    } else {
        page.when_narrowest(&switch, "visible", false);
    }
    let weak = Rc::downgrade(ui);
    let instance = row.instance.clone();
    switch.connect_active_notify(move |switch| {
        let on = switch.is_active();
        if let Some(ui) = weak.upgrade() {
            let instance = instance.clone();
            glib::spawn_future_local(async move {
                let saved = data::run(move |host| engine::set_included(host, &instance, on)).await;
                if !matches!(saved, Some(Ok(()))) {
                    toast(&ui, &tr("The choice could not be saved"));
                }
                refresh(&ui).await;
            });
        }
    });
    action
}

/// The Top Bar Style part's two choices: one full bar or a pill per
/// button, and how opaque. Saved at once; the next apply draws them.
fn top_bar_rows(ui: &U, page: &Column, bar: TopBar) -> [adw::PreferencesRow; 2] {
    let ways = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["linked"])
        .valign(gtk::Align::Center)
        .build();
    ways.update_property(&[gtk::accessible::Property::Label(&tr("Top Bar Shape"))]);
    let mut first: Option<gtk::ToggleButton> = None;
    for (style, label) in [
        (BarStyle::Bar, tr("Complete _Bar")),
        (BarStyle::Pills, tr("_Pills")),
    ] {
        let button = gtk::ToggleButton::builder()
            .label(label)
            .use_underline(true)
            .active(bar.style == style)
            .build();
        match &first {
            Some(group) => button.set_group(Some(group)),
            None => first = Some(button.clone()),
        }
        let weak = Rc::downgrade(ui);
        button.connect_toggled(move |button| {
            let Some(ui) = weak.upgrade().filter(|_| button.is_active()) else {
                return;
            };
            glib::spawn_future_local(async move {
                save_top_bar(&ui, move |bar| bar.style = style).await;
            });
        });
        ways.append(&button);
    }
    let shape = adw::ActionRow::builder().title(tr("Shape")).build();
    shape.add_suffix(&ways);

    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 5.0);
    scale.set_value(f64::from(bar.opacity));
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Left);
    scale.set_digits(0);
    scale.set_format_value_func(|_, value| format!("{value:.0}%"));
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.set_size_request(200, -1);
    scale.update_property(&[gtk::accessible::Property::Label(&tr("Top Bar Opacity"))]);
    page.when_narrowest(&scale, "width-request", 120);
    let weak = Rc::downgrade(ui);
    scale.connect_value_changed(move |scale| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Steps of 5, saved once the slider rests.
        let opacity = ((scale.value() / 5.0).round() * 5.0).clamp(0.0, 100.0) as u8;
        if scale.value() != f64::from(opacity) {
            // Dragging does not snap to the step by itself; this comes
            // back here with the snapped value.
            scale.set_value(f64::from(opacity));
            return;
        }
        if let Some(waiting) = ui.opacity_pending.take() {
            waiting.remove();
        }
        let weak = Rc::downgrade(&ui);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            if let Some(ui) = weak.upgrade() {
                ui.opacity_pending.set(None);
                glib::spawn_future_local(async move {
                    save_top_bar(&ui, move |bar| bar.opacity = opacity).await;
                });
            }
        });
        ui.opacity_pending.set(Some(id));
    });
    let opacity = adw::ActionRow::builder().title(tr("Opacity")).build();
    opacity.add_suffix(&scale);
    [shape.upcast(), opacity.upcast()]
}

/// Save a change to the top bar's choices, then show "Apply Changes" if
/// the theme in use now draws it differently. The page is not rebuilt, so
/// the slider stays where the pointer has it.
async fn save_top_bar(ui: &U, change: impl FnOnce(&mut TopBar) + Send + 'static) {
    let saved = data::run(move |host| {
        let mut bar = engine::top_bar(host)?;
        change(&mut bar);
        engine::set_top_bar(host, bar)
    })
    .await;
    if !matches!(saved, Some(Ok(()))) {
        toast(ui, &tr("The choice could not be saved"));
        return;
    }
    if let Some(snap) = data::run(data::snapshot).await {
        if let Some(group) = ui.apply_changes.borrow().as_ref() {
            group.set_visible(pending(&snap));
        }
        *ui.snap.borrow_mut() = snap;
    }
}

/// Where adw-gtk3 is found, with how to install it.
const ADW_GTK3: &str = "https://github.com/lassekongo83/adw-gtk3";

/// A titled group of parts, with how many of them follow the theme.
fn parts_group(
    title: &str,
    description: Option<&str>,
    rows: &[&data::AppRow],
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    group.set_description(description);
    let can: Vec<_> = rows.iter().filter(|r| available(r)).collect();
    if !can.is_empty() {
        let on = can.iter().filter(|r| r.included).count();
        group.set_header_suffix(Some(
            &gtk::Label::builder()
                .label(
                    plural("{on} of {n} themed", "{on} of {n} themed", can.len())
                        .replace("{on}", &on.to_string()),
                )
                .valign(gtk::Align::End)
                .css_classes(["caption", "dim-label", "numeric"])
                .build(),
        ));
    }
    group
}

fn apps_view(ui: &U, snap: &Snapshot) -> gtk::Widget {
    let page = Column::new();
    let groups = [
        (
            tr("Desktop"),
            vec![
                targets::SHELL,
                targets::TOP_BAR,
                targets::UPS,
                targets::OUTLINE,
                targets::APPEARANCE,
                targets::ICONS,
                targets::CURSORS,
            ],
        ),
        (
            tr("Terminals"),
            vec![targets::KITTY, targets::GHOSTTY, targets::PTYXIS],
        ),
        (
            tr("Command-Line Apps"),
            vec![targets::TMUX, targets::NVIM, targets::STARSHIP],
        ),
        (
            tr("Apps"),
            vec![
                targets::GTK4,
                targets::GTK3,
                targets::TEXT_EDITOR,
                targets::FIREFOX,
                targets::FLATPAK,
            ],
        ),
    ];
    // How many of the parts that can be themed follow the theme.
    let can: Vec<_> = snap.rows.iter().filter(|r| available(r)).collect();
    let on = can.iter().filter(|r| r.included).count();
    page.append(
        &gtk::Label::builder()
            .label(
                plural(
                    "{on} of {n} part of the desktop and apps follows the theme",
                    "{on} of {n} parts of the desktop and apps follow the theme",
                    can.len(),
                )
                .replace("{on}", &on.to_string()),
            )
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    // Turning a part on or off takes effect with the next apply: offer it
    // here, where the switches are.
    *ui.apply_changes.borrow_mut() = None;
    if let Some(label) = snap.applied.as_deref() {
        let group = adw::PreferencesGroup::builder()
            .visible(pending(snap))
            .build();
        let row = adw::ButtonRow::builder()
            .title(tr("Apply _Changes to {theme}").replace("{theme}", &theme_name(snap, label)))
            .use_underline(true)
            .build();
        row.add_css_class("suggested-action");
        let weak = Rc::downgrade(ui);
        let spec = label
            .split_once('/')
            .map(|(id, v)| (id.to_owned(), v.to_owned()));
        row.connect_activated(move |_| {
            if let (Some(ui), Some((id, variant))) = (weak.upgrade(), spec.clone()) {
                glib::spawn_future_local(async move { apply_theme(&ui, id, variant).await });
            }
        });
        group.add(&row);
        page.append(&group);
        *ui.apply_changes.borrow_mut() = Some(group);
    }
    for (title, members) in groups {
        // In the group's order, not the engine's.
        let rows: Vec<&data::AppRow> = members
            .iter()
            .filter_map(|m| snap.rows.iter().find(|r| r.instance == *m))
            .collect();
        let group = parts_group(&title, None, &rows);
        for row in rows {
            group.add(&part_row(
                ui,
                snap,
                &page,
                row,
                &velatura_text::instance(&row.instance),
                None,
            ));
            // The top bar's choices, under its switch while it is on.
            if row.instance == targets::TOP_BAR && row.included && available(row) {
                for choice in top_bar_rows(ui, &page, snap.top_bar) {
                    group.add(&choice);
                }
            }
        }
        page.append(&group);
    }

    // The user's own templates, shown only when there are some.
    if !snap.templates.is_empty() {
        let rows: Vec<(&data::AppRow, &String, &String)> = snap
            .templates
            .iter()
            .filter_map(|(instance, name, place)| {
                Some((
                    snap.rows.iter().find(|r| &r.instance == instance)?,
                    name,
                    place,
                ))
            })
            .collect();
        let group = parts_group(
            &tr("Templates"),
            Some(&tr(
                "Files written from the templates in ~/.local/share/velatura/templates. Added and removed with: velatura template",
            )),
            &rows.iter().map(|(r, ..)| *r).collect::<Vec<_>>(),
        );
        for (row, name, place) in rows {
            group.add(&part_row(ui, snap, &page, row, name, Some(place)));
        }
        page.append(&group);
    }

    page.append(&extension_group(ui, snap));
    page.append(&look_group(ui, snap));

    // Shown only while there is something of Velatura's to remove.
    if snap.applied.is_some() || !snap.history.is_empty() {
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
        page.append(&group);
    }
    page.done()
}

/// Velatura's own Shell extension: what it is for, where it stands, and
/// one button to install or remove it. Turning it on is left to the user.
fn extension_group(ui: &U, snap: &Snapshot) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Shell Extension"))
        .description(tr(
            "Velatura's own extension themes the top bar and overview where other extensions bring their own colors",
        ))
        .build();
    let status = &snap.extension;
    let row = adw::ActionRow::builder()
        .title(tr("Velatura Extension"))
        .subtitle(glib::markup_escape_text(&velatura_text::extension(status)))
        .subtitle_lines(0)
        .build();
    // A copy from a package is not Velatura's to remove or replace.
    let action = match (&status.installed, status.by_velatura, status.outdated) {
        (None, ..) => Some((tr("_Install"), true)),
        (Some(_), true, true) => Some((tr("_Update"), true)),
        (Some(_), true, false) => Some((tr("_Remove"), false)),
        (Some(_), false, _) => None,
    };
    if let Some((label, install)) = action {
        let button = gtk::Button::builder()
            .label(label)
            .use_underline(true)
            .valign(gtk::Align::Center)
            .build();
        let weak = Rc::downgrade(ui);
        button.connect_clicked(move |_| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            glib::spawn_future_local(async move {
                let home = ui.snap.borrow().home.clone();
                let done = data::run(move |host| {
                    if install {
                        velatura_engine::companion::install(host).map(|_| ())
                    } else {
                        velatura_engine::companion::remove(host).map(|_| ())
                    }
                })
                .await;
                refresh(&ui).await;
                match done {
                    Some(Ok(())) if install => toast(
                        &ui,
                        &tr("Extension installed. It can be turned on after signing out and in"),
                    ),
                    Some(Ok(())) => toast(&ui, &tr("Extension removed")),
                    Some(Err(why)) => toast(&ui, &velatura_text::refused(&why, &home)),
                    None => toast(&ui, &tr("The change could not be started")),
                }
            });
        });
        row.add_suffix(&button);
    }
    group.add(&row);
    group
}

/// Icons, cursor, wallpaper and fonts: a choice among what is installed,
/// made at once and undone like any other change.
fn look_group(ui: &U, snap: &Snapshot) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title(tr("Icons, Wallpaper and Fonts"))
        .description(tr("These stay as chosen when the theme changes"))
        .build();
    for (kind, current, choices) in &snap.look {
        let kind = *kind;
        let title = velatura_text::look_name(kind);
        if matches!(kind, Kind::Font | Kind::DocumentFont | Kind::MonospaceFont) {
            let button = gtk::FontDialogButton::builder()
                .dialog(&gtk::FontDialog::builder().title(title.as_str()).build())
                .font_desc(&gtk::pango::FontDescription::from_string(current))
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(ui);
            button.connect_font_desc_notify(move |button| {
                let (Some(ui), Some(font)) = (weak.upgrade(), button.font_desc()) else {
                    return;
                };
                let font = font.to_string();
                glib::spawn_future_local(async move { choose_look(&ui, kind, font).await });
            });
            let row = adw::ActionRow::builder().title(title).build();
            row.add_suffix(&button);
            row.set_activatable_widget(Some(&button));
            group.add(&row);
            continue;
        }
        // What is in use may be something that was not found installed.
        let mut values: Vec<String> = choices.iter().map(|c| c.value.clone()).collect();
        let mut names: Vec<String> = choices.iter().map(|c| c.name.clone()).collect();
        let selected = values.iter().position(|v| v == current).unwrap_or_else(|| {
            values.insert(0, current.clone());
            names.insert(0, velatura_text::look_value(kind, current));
            0
        });
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&names))
            .selected(selected as u32)
            .sensitive(values.len() > 1)
            .build();
        let weak = Rc::downgrade(ui);
        row.connect_selected_notify(move |row| {
            let (Some(ui), Some(value)) =
                (weak.upgrade(), values.get(row.selected() as usize).cloned())
            else {
                return;
            };
            glib::spawn_future_local(async move { choose_look(&ui, kind, value).await });
        });
        group.add(&row);
    }
    group
}

async fn choose_look(ui: &U, kind: Kind, value: String) {
    if busy(ui) {
        return;
    }
    ui.busy.set(true);
    let done = data::run(move |host| {
        look::plan_select(host, &data::env, kind, Some(&value))
            .map(|plan| engine::execute(host, &plan, &[]))
    })
    .await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok(Ok(report))) if report.changes.is_empty() => {}
        Some(Ok(Ok(_))) => toast_with_undo(
            ui,
            &tr("{what} changed").replace("{what}", &velatura_text::look_name(kind)),
        ),
        Some(Ok(Err(e))) => result_page(ui, &tr("Not Changed"), false, &[error_text(&e)], false),
        Some(Err(why)) => toast(
            ui,
            &velatura_text::rejected_choice(kind, &why)
                .unwrap_or_else(|| tr("The choice could not be saved")),
        ),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// Undoable, so it is done at once and Undo is offered, with no question.
async fn remove_all(ui: &U) {
    if busy(ui) {
        return;
    }
    ui.busy.set(true);
    let home = ui.snap.borrow().home.clone();
    let done = data::run(move |host| {
        // The timer first, or it would bring a theme back.
        let timer = match schedule::disable(host, &StdSystemd) {
            Ok(was_on) => was_on.is_some(),
            Err(why) => return Err(velatura_text::refused(&why, &home)),
        };
        engine::plan_restore_original(host)
            .and_then(|plan| engine::execute(host, &plan, &[]))
            .map(|report| (report, timer))
            .map_err(|e| error_text(&e))
    })
    .await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok((report, false))) if report.changes.is_empty() => {
            toast(ui, &tr("Nothing to remove"));
        }
        Some(Ok(_)) => toast_with_undo(ui, &tr("Everything Velatura added was removed")),
        Some(Err(why)) => result_page(ui, &tr("Not Removed"), false, &[why], false),
        None => toast(ui, &tr("The change could not be started")),
    }
}

/// The display name for a stored `theme/variant`, or the label itself if
/// that theme is no longer in the library.
/// A part turned on and not applied yet, or turned off while Velatura's
/// colors are still on it: the next apply of the theme in use changes
/// something.
fn pending(snap: &Snapshot) -> bool {
    snap.applied.is_some()
        && snap.rows.iter().any(|r| match r.state {
            RowState::Ready => r.included,
            RowState::Themed => !r.included,
            _ => false,
        })
}

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
            .title(tr("No Changes Yet"))
            .description(tr(
                "Each change Velatura makes is listed here, and can be undone",
            ))
            .build()
            .upcast();
    }
    let page = Column::new();
    page.append(
        &gtk::Label::builder()
            .label(tr(
                "Restoring goes back to how things were just before that change",
            ))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );
    // The newest change that stands is how things are now.
    let current = snap.history.iter().find(|m| !m.undone).map(|m| m.id);
    let shown = ui.history_shown.get();
    // One group per day, newest first, as the changes come.
    let mut day: Option<((i32, i32, i32), adw::PreferencesGroup)> = None;
    for m in snap.history.iter().take(shown) {
        let at = glib::DateTime::from_unix_local(m.time as i64).ok();
        let key = at.as_ref().map_or((0, 0, 0), glib::DateTime::ymd);
        if day.as_ref().is_none_or(|(k, _)| *k != key) {
            let group = adw::PreferencesGroup::builder()
                .title(at.as_ref().map(day_name).unwrap_or_default())
                .build();
            page.append(&group);
            day = Some((key, group));
        }
        let Some((_, group)) = &day else {
            continue;
        };
        group.add(&history_row(
            ui,
            snap,
            m,
            at.as_ref(),
            current == Some(m.id),
        ));
    }
    if snap.history.len() > shown {
        let group = adw::PreferencesGroup::new();
        let more = adw::ButtonRow::builder()
            .title(tr("_Show Older Changes"))
            .use_underline(true)
            .build();
        let weak = Rc::downgrade(ui);
        more.connect_activated(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.history_shown.set(ui.history_shown.get() + HISTORY_PAGE);
                let snap = ui.snap.borrow().clone();
                keep::replace(&ui.history, &history_view(&ui, &snap));
            }
        });
        group.add(&more);
        page.append(&group);
    }
    page.done()
}

/// One change: when, the colours it applied, what it was, and the way
/// back to before it.
fn history_row(
    ui: &U,
    snap: &Snapshot,
    m: &velatura_engine::store::Manifest,
    at: Option<&glib::DateTime>,
    current: bool,
) -> adw::ActionRow {
    let what = match (m.cause.as_str(), &m.theme) {
        ("apply", Some(label)) => {
            tr("Applied {theme}").replace("{theme}", &theme_name(snap, label))
        }
        ("scheduled", Some(label)) => {
            tr("Switched to {theme} on schedule").replace("{theme}", &theme_name(snap, label))
        }
        ("select", _) => {
            let what = m.records.iter().find_map(|r| match r {
                velatura_engine::store::Record::Setting { key, .. } => {
                    Some(velatura_text::setting_title(key))
                }
                _ => None,
            });
            tr("Changed {what}").replace("{what}", &what.unwrap_or_default())
        }
        ("restore-original", _) => tr("Removed everything Velatura added"),
        _ => tr("Change"),
    };
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&what))
        .build();
    if m.undone {
        row.set_subtitle(&tr("Undone"));
    }
    let time = at
        .and_then(|d| d.format("%H:%M").ok())
        .map(String::from)
        .unwrap_or_default();
    let time = gtk::Label::builder()
        .label(time)
        .width_chars(5)
        .xalign(0.0)
        .css_classes(["dim-label", "numeric"])
        .build();
    // The colours of the theme it applied, or room for them, so the names
    // line up.
    let strip: gtk::Widget = m
        .theme
        .as_deref()
        .and_then(|label| {
            let (id, variant) = label.split_once('/')?;
            let entry = snap.themes.iter().find(|e| e.theme.id == id)?;
            Resolved::new(&entry.theme, Some(variant)).map(|r| widgets::strip(&r).upcast())
        })
        .unwrap_or_else(|| gtk::Box::builder().width_request(44).build().upcast());
    // Each prefix goes before the ones already there.
    row.add_prefix(&strip);
    row.add_prefix(&time);
    if current {
        let now = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .valign(gtk::Align::Center)
            .css_classes(["accent"])
            .build();
        now.append(&gtk::Image::from_icon_name("object-select-symbolic"));
        now.append(
            &gtk::Label::builder()
                .label(tr("Current"))
                .css_classes(["caption-heading"])
                .build(),
        );
        row.add_suffix(&now);
    } else if !m.undone {
        let restore = icon_button("edit-undo-symbolic", &tr("Go Back to Before This Change…"));
        restore.add_css_class("flat");
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
    row
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
    dialog.add_responses(&[("cancel", &tr("_Cancel")), ("restore", &tr("_Go Back"))]);
    dialog.set_close_response("cancel");
    if dialog.choose_future(Some(&ui.window)).await != "restore" {
        return;
    }
    ui.busy.set(true);
    let done = data::run(move |host| engine::restore_to(host, id)).await;
    ui.busy.set(false);
    refresh(ui).await;
    match done {
        Some(Ok(_)) => toast(ui, &tr("Gone back to before the change")),
        Some(Err(e)) => result_page(ui, &tr("Not Undone"), false, &[error_text(&e)], false),
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
        .title(tr("Apps & Desktop"))
        .build();
    for row in &snap.rows {
        let state = match &row.state {
            RowState::Unavailable(why) => words::reason(&words::skip(why, &home)),
            RowState::Shadowed(note) => words::note(note, &home),
            _ => tr("Can be themed"),
        };
        parts.add(
            &adw::ActionRow::builder()
                .title(velatura_text::instance(&row.instance))
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
            targets::OtherTool::PinnedColors { name, keys } => (
                name.clone(),
                tr("An extension with colors of its own in its settings ({keys}). They stay the same whatever theme is applied")
                    .replace("{keys}", &keys.join(", ")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_colors_are_valid_css() {
        for (name, value) in stock::LIGHT.iter().chain(stock::DARK.iter()) {
            assert!(
                gtk::gdk::RGBA::parse(*value).is_ok(),
                "{name}: {value} is not a colour"
            );
            if let Some(hex) = value.strip_prefix('#') {
                assert!(
                    hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()),
                    "{name}: {value} is not #rrggbb"
                );
            }
        }
    }
}
