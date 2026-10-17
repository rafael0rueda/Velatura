//! The theme editor: three colours first, everything else worked out and
//! shown, each of which can be given a colour of its own.
//!
//! All the thinking is in `velatura_theme::editor`; this is rows and
//! buttons around a [`Draft`]. Saving goes through the engine's library.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use velatura_engine::library;
use velatura_theme::editor::{Draft, Problem, free_id, problems, slug};
use velatura_theme::model::{ANSI_NAMES, SYNTAX_ROLES, UI_OPTIONAL};
use velatura_theme::{Appearance, Color, Resolved};

use crate::words::{plural, tr};
use crate::{U, data, refresh, toast, widgets};

struct Session {
    ui: U,
    draft: RefCell<Draft>,
    /// True when saving replaces a theme of the user's; false for a new
    /// theme, whose id then follows its name.
    replaces: bool,
    page: adw::NavigationPage,
    /// Rebuilt after every change of colour.
    body: adw::Bin,
    /// Which of Interface, Terminal and Code are open.
    open: Cell<[bool; 3]>,
}

type S = Rc<Session>;

/// Makes a colour follow the others again, or unsets it.
type Reset = Box<dyn Fn(&mut Draft)>;

fn rgba(c: Color) -> gdk::RGBA {
    gdk::RGBA::new(
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        1.0,
    )
}

fn color(c: &gdk::RGBA) -> Color {
    let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::rgb(byte(c.red()), byte(c.green()), byte(c.blue()))
}

/// What a role is called. One literal each, so all can be translated.
fn role_name(role: &str) -> String {
    match role {
        "bg" => tr("Background"),
        "fg" => tr("Text"),
        "accent" => tr("Accent"),
        "bg_dim" => tr("Dim Background"),
        "surface" => tr("Surface"),
        "border" => tr("Border"),
        "fg_muted" => tr("Secondary Text"),
        "fg_subtle" => tr("Faint Text"),
        "accent_fg" => tr("Text on Accent"),
        "selection_bg" => tr("Selection"),
        "selection_fg" => tr("Selected Text"),
        "cursor" => tr("Cursor"),
        "cursor_fg" => tr("Text Under Cursor"),
        "link" => tr("Link"),
        "success" => tr("Success"),
        "warning" => tr("Warning"),
        "error" => tr("Error"),
        "black" => tr("Black"),
        "red" => tr("Red"),
        "green" => tr("Green"),
        "yellow" => tr("Yellow"),
        "blue" => tr("Blue"),
        "magenta" => tr("Magenta"),
        "cyan" => tr("Cyan"),
        "white" => tr("White"),
        "bright_black" => tr("Bright Black"),
        "bright_red" => tr("Bright Red"),
        "bright_green" => tr("Bright Green"),
        "bright_yellow" => tr("Bright Yellow"),
        "bright_blue" => tr("Bright Blue"),
        "bright_magenta" => tr("Bright Magenta"),
        "bright_cyan" => tr("Bright Cyan"),
        "bright_white" => tr("Bright White"),
        "comment" => tr("Comment"),
        "keyword" => tr("Keyword"),
        "string" => tr("String"),
        "number" => tr("Number"),
        "constant" => tr("Constant"),
        "function" => tr("Function"),
        "type" => tr("Type"),
        "variable" => tr("Variable"),
        "parameter" => tr("Parameter"),
        "operator" => tr("Operator"),
        "punctuation" => tr("Punctuation"),
        "property" => tr("Property"),
        "tag" => tr("Tag"),
        "attribute" => tr("Attribute"),
        "builtin" => tr("Built-In"),
        "escape" => tr("Escape Sequence"),
        other => other.to_owned(),
    }
}

/// One colour: its name, its value as text, a button that opens the
/// desktop's colour chooser, and for a colour that can follow others a
/// button that makes it do so again.
fn color_row(
    s: &S,
    title: &str,
    value: Option<Color>,
    shown: Color,
    state: Option<&str>,
    set: impl Fn(&mut Draft, Color) + 'static,
    reset: Option<Reset>,
) -> adw::ActionRow {
    let subtitle = match (value, state) {
        (_, Some(state)) => state.to_owned(),
        (Some(c), None) => c.hex(),
        (None, None) => String::new(),
    };
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    if let Some(reset) = reset {
        let button = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .tooltip_text(tr("Reset to Automatic"))
            .valign(gtk::Align::Center)
            .build();
        button.add_css_class("flat");
        let weak = Rc::downgrade(s);
        button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                reset(&mut s.draft.borrow_mut());
                changed(&s);
            }
        });
        row.add_suffix(&button);
    }
    let dialog = gtk::ColorDialog::builder()
        .title(title)
        .with_alpha(false)
        .build();
    let button = gtk::ColorDialogButton::builder()
        .dialog(&dialog)
        .rgba(&rgba(shown))
        .valign(gtk::Align::Center)
        .build();
    // The swatch says its colour in words too.
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "{title} {}",
        shown.hex()
    ))]);
    let weak = Rc::downgrade(s);
    button.connect_rgba_notify(move |button| {
        if let Some(s) = weak.upgrade() {
            set(&mut s.draft.borrow_mut(), color(&button.rgba()));
            changed(&s);
        }
    });
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    row
}

fn problem_text(p: &Problem) -> String {
    tr("{role} is hard to read on {on}: {ratio} to 1, where {need} is needed")
        .replace("{role}", &role_name(p.role))
        .replace("{on}", &role_name(p.on))
        .replace("{ratio}", &format!("{:.1}", p.ratio))
        .replace("{need}", &format!("{:.1}", p.need))
}

/// Everything below the name: rebuilt from the draft after each change.
fn build_body(s: &S) -> gtk::Widget {
    let draft = s.draft.borrow();
    let v = draft.colors();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);

    if let Some(resolved) = Resolved::new(&draft.theme, Some(&draft.variant)) {
        s.ui.preview_css
            .load_from_string(&widgets::preview_css(&resolved));
        column.append(&widgets::preview(&resolved));
    }

    let found = problems(v);
    if !found.is_empty() {
        let group = adw::PreferencesGroup::builder()
            .title(tr("Hard to Read"))
            .build();
        for p in found {
            let row = adw::ActionRow::builder()
                .title(problem_text(&p))
                .title_lines(0)
                .build();
            let fix = gtk::Button::builder()
                .label(tr("Fix"))
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(s);
            fix.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.draft.borrow_mut().fix(&p);
                    changed(&s);
                }
            });
            row.add_suffix(&fix);
            group.add(&row);
        }
        column.append(&group);
    }

    let main = adw::PreferencesGroup::builder()
        .title(tr("Main Colors"))
        .description(tr(
            "Every other color follows these three until it is given one of its own",
        ))
        .build();
    for role in ["bg", "fg", "accent"] {
        let c = v.ui.get(role).unwrap_or(v.ui.bg);
        main.add(&color_row(
            s,
            &role_name(role),
            Some(c),
            c,
            None,
            move |d, c| d.set_role(role, c),
            None,
        ));
    }
    let dark = adw::SwitchRow::builder()
        .title(tr("_Dark Theme"))
        .use_underline(true)
        .subtitle(tr("Apps and the desktop use their dark style with it"))
        .active(v.appearance == Appearance::Dark)
        .build();
    let weak = Rc::downgrade(s);
    dark.connect_active_notify(move |row| {
        if let Some(s) = weak.upgrade() {
            s.draft.borrow_mut().set_appearance(if row.is_active() {
                Appearance::Dark
            } else {
                Appearance::Light
            });
            changed(&s);
        }
    });
    main.add(&dark);
    column.append(&main);

    let more = adw::PreferencesGroup::new();
    let open = s.open.get();
    let expander = |index: usize, title: String, subtitle: String| {
        let row = adw::ExpanderRow::builder()
            .title(title)
            .use_underline(true)
            .subtitle(subtitle)
            .expanded(open[index])
            .build();
        let weak = Rc::downgrade(s);
        row.connect_expanded_notify(move |row| {
            if let Some(s) = weak.upgrade() {
                let mut open = s.open.get();
                open[index] = row.is_expanded();
                s.open.set(open);
            }
        });
        row
    };

    let custom = UI_OPTIONAL
        .iter()
        .filter(|r| !draft.is_automatic(r))
        .count();
    let interface = expander(
        0,
        tr("_Interface"),
        plural("{n} color set by hand", "{n} colors set by hand", custom),
    );
    for role in UI_OPTIONAL {
        let c = v.ui.get(role).unwrap_or(v.ui.bg);
        let auto = draft.is_automatic(role);
        interface.add_row(&color_row(
            s,
            &role_name(role),
            Some(c),
            c,
            auto.then(|| format!("{} · {}", c.hex(), tr("Automatic")))
                .as_deref(),
            move |d, c| d.set_role(role, c),
            (!auto).then(|| Box::new(move |d: &mut Draft| d.reset_role(role)) as Reset),
        ));
    }
    more.add(&interface);

    let terminal = expander(
        1,
        tr("_Terminal"),
        if draft.auto_terminal {
            tr("Automatic")
        } else {
            tr("Set by hand")
        },
    );
    for (index, name) in ANSI_NAMES.iter().enumerate() {
        let c = v.ansi[index];
        terminal.add_row(&color_row(
            s,
            &role_name(name),
            Some(c),
            c,
            None,
            move |d, c| d.set_terminal(index, c),
            None,
        ));
    }
    if !draft.auto_terminal {
        let reset = adw::ButtonRow::builder()
            .title(tr("_Reset Terminal Colors to Automatic"))
            .use_underline(true)
            .build();
        let weak = Rc::downgrade(s);
        reset.connect_activated(move |_| {
            if let Some(s) = weak.upgrade() {
                s.draft.borrow_mut().reset_terminal();
                changed(&s);
            }
        });
        terminal.add_row(&reset);
    }
    more.add(&terminal);

    let code = expander(
        2,
        tr("_Code"),
        plural("{n} color set", "{n} colors set", v.syntax.len()),
    );
    for role in SYNTAX_ROLES {
        let set = v.syntax.get(role).copied();
        code.add_row(&color_row(
            s,
            &role_name(role),
            set,
            set.unwrap_or(v.ui.fg),
            set.is_none().then(|| tr("Not set")).as_deref(),
            move |d, c| d.set_syntax(role, Some(c)),
            set.map(|_| Box::new(move |d: &mut Draft| d.set_syntax(role, None)) as Reset),
        ));
    }
    more.add(&code);
    column.append(&more);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_halign(gtk::Align::Center);
    let discard = gtk::Button::builder()
        .label(tr("_Discard"))
        .use_underline(true)
        .build();
    discard.add_css_class("pill");
    let weak = Rc::downgrade(s);
    discard.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            s.page.set_can_pop(true);
            s.ui.nav.pop_to_tag("main");
        }
    });
    let save = gtk::Button::builder()
        .label(tr("_Save Theme"))
        .use_underline(true)
        .action_name("win.save-theme")
        .build();
    save.add_css_class("pill");
    save.add_css_class("suggested-action");
    buttons.append(&discard);
    buttons.append(&save);
    column.append(&buttons);
    column.upcast()
}

/// After any change to the draft: show it, and stop the page from being
/// left by accident with the work unsaved.
fn changed(s: &S) {
    s.page.set_can_pop(false);
    // Not while a signal from a widget about to be replaced is running.
    let weak = Rc::downgrade(s);
    glib::idle_add_local_once(move || {
        if let Some(s) = weak.upgrade() {
            crate::keep::replace(&s.body, &build_body(&s));
        }
    });
}

thread_local! {
    /// The editor that is open, for the Save action and its shortcut.
    static CURRENT: RefCell<Option<S>> = const { RefCell::new(None) };
}

/// True while the open editor holds changes that are not saved.
pub fn unsaved() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(|s| !s.page.can_pop()))
}

/// Whether the open editor's unsaved changes may go: asked, if there are
/// any. Once they may, the editor no longer holds on to them.
pub async fn may_discard(ui: &U) -> bool {
    if !unsaved() {
        return true;
    }
    let dialog = adw::AlertDialog::new(
        Some(&tr("Discard Changes?")),
        Some(&tr("The changes to this theme have not been saved.")),
    );
    dialog.add_responses(&[("cancel", &tr("_Cancel")), ("discard", &tr("_Discard"))]);
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    if dialog.choose_future(Some(&ui.window)).await != "discard" {
        return false;
    }
    if let Some(s) = CURRENT.with(|c| c.borrow().clone()) {
        s.page.set_can_pop(true);
    }
    true
}

/// Back to the themes, once unsaved changes may go.
fn leave(ui: &U) {
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        if may_discard(&ui).await {
            ui.nav.pop_to_tag("main");
        }
    });
}

/// Open the editor on `draft`. With `replaces`, saving writes over the
/// theme of the user's that the draft was opened from. An editor already
/// open with unsaved changes asks first, as it is replaced.
pub fn open(ui: &U, draft: Draft, replaces: bool) {
    if !unsaved() {
        open_now(ui, draft, replaces);
        return;
    }
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        if may_discard(&ui).await {
            open_now(&ui, draft, replaces);
        }
    });
}

fn open_now(ui: &U, draft: Draft, replaces: bool) {
    let name = adw::EntryRow::builder()
        .title(tr("_Name"))
        .use_underline(true)
        .text(draft.colors().name.as_str())
        .build();
    let names = adw::PreferencesGroup::new();
    names.add(&name);

    let body = adw::Bin::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    column.set_margin_top(24);
    column.set_margin_bottom(24);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.append(&names);
    column.append(&body);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(
            &adw::Clamp::builder()
                .maximum_size(600)
                .child(&column)
                .build(),
        )
        .build();
    // While there are changes to lose, the page cannot be left by
    // accident: the header bar's own Back button goes, and this one asks.
    let header = adw::HeaderBar::new();
    let back = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text(tr("Back"))
        .build();
    back.update_property(&[gtk::accessible::Property::Label(&tr("Back"))]);
    header.pack_start(&back);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&scroll));
    let page = adw::NavigationPage::builder()
        .title(if replaces {
            tr("Edit Theme")
        } else {
            tr("New Theme")
        })
        .tag("editor")
        .child(&view)
        .build();
    page.bind_property("can-pop", &back, "visible")
        .invert_boolean()
        .sync_create()
        .build();
    let weak = Rc::downgrade(ui);
    back.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            leave(&ui);
        }
    });
    // The keys that go back, which the page no longer answers to by itself.
    let keys = gtk::ShortcutController::new();
    let weak = Rc::downgrade(ui);
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape|<Alt>Left"),
        Some(gtk::CallbackAction::new(move |_, _| match weak.upgrade() {
            Some(ui) if unsaved() => {
                leave(&ui);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        })),
    ));
    page.add_controller(keys);

    let s = Rc::new(Session {
        ui: ui.clone(),
        draft: RefCell::new(draft),
        replaces,
        page: page.clone(),
        body,
        open: Cell::new([false; 3]),
    });
    s.body.set_child(Some(&build_body(&s)));
    // The name is typed, not picked: it changes the draft without the
    // rows being rebuilt under the cursor.
    let weak = Rc::downgrade(&s);
    name.connect_changed(move |row| {
        if let Some(s) = weak.upgrade() {
            s.draft.borrow_mut().set_name(&row.text());
            s.page.set_can_pop(false);
        }
    });
    page.connect_hidden(|_| {
        CURRENT.with(|c| c.borrow_mut().take());
    });
    CURRENT.with(|c| *c.borrow_mut() = Some(s));
    ui.nav.pop_to_tag("main");
    ui.nav.push(&page);
}

/// Save the theme in the open editor, if there is one.
pub async fn save_current() {
    let Some(s) = CURRENT.with(|c| c.borrow().clone()) else {
        return;
    };
    let mut theme = s.draft.borrow().theme.clone();
    let replaces = s.replaces;
    if theme.variants[&s.draft.borrow().variant].name.is_empty() {
        toast(&s.ui, &tr("A theme needs a name"));
        return;
    }
    let saved = data::run(move |host| {
        if !replaces {
            // A new theme's id follows its name, and is never one in use.
            theme.id = free_id(&slug(&theme.name), &|id| library::taken(host, id));
        }
        library::save(host, &theme, None, replaces).map(|_| theme.id)
    })
    .await;
    let home = s.ui.snap.borrow().home.clone();
    match saved {
        Some(Ok(id)) => {
            println_saved(&id);
            s.page.set_can_pop(true);
            refresh(&s.ui).await;
            s.ui.nav.pop_to_tag("main");
            toast(&s.ui, &tr("Theme saved"));
        }
        Some(Err(why)) => toast(&s.ui, &velatura_text::library_refused(&why, &home)),
        None => toast(&s.ui, &tr("The change could not be started")),
    }
}

/// For the check that drives the editor without a person: say what was saved.
fn println_saved(id: &str) {
    if std::env::var_os("VELATURA_SHOTS").is_some() {
        println!("SHOTS editor saved={id}");
    }
}

/// A new theme: the editor on three starting colours.
pub fn new_theme(ui: &U) {
    let c = |hex: &str| Color::parse(hex).unwrap_or(Color::rgb(0, 0, 0));
    let dark = adw::StyleManager::default().is_dark();
    let (appearance, colors) = if dark {
        (Appearance::Dark, [c("#1d1f27"), c("#e3e5ee"), c("#7a9cf0")])
    } else {
        (
            Appearance::Light,
            [c("#fafafb"), c("#1f2026"), c("#3a6fd8")],
        )
    };
    open(
        ui,
        Draft::new("theme", &tr("New Theme"), appearance, colors),
        false,
    );
}

/// The editor on a theme from the library. One of the user's is edited in
/// place; one that comes with Velatura is edited as a copy.
pub fn edit(ui: &U, id: &str, variant: &str) {
    let snap = ui.snap.borrow();
    let Some(entry) = snap.themes.iter().find(|e| e.theme.id == id) else {
        return;
    };
    let Some(mut draft) = Draft::open(&entry.theme, variant) else {
        return;
    };
    let bundled = entry.bundled;
    if bundled {
        let name = tr("{name} (Copy)").replace("{name}", &draft.colors().name);
        draft.theme.name.clone_from(&name);
        draft.set_name(&name);
    }
    drop(snap);
    open(ui, draft, !bundled);
}

/// For the gate: change one colour in the open editor as a person would.
pub fn poke_current() {
    if let Some(s) = CURRENT.with(|c| c.borrow().clone()) {
        s.draft
            .borrow_mut()
            .set_role("accent", Color::rgb(0xe0, 0x86, 0x4a));
        s.draft
            .borrow_mut()
            .set_role("fg", Color::rgb(0x50, 0x50, 0x58));
        let first = problems(s.draft.borrow().colors()).first().copied();
        if let Some(p) = first {
            println!("SHOTS editor problem={} on {}", p.role, p.on);
            s.draft.borrow_mut().fix(&p);
        }
        changed(&s);
    }
}
