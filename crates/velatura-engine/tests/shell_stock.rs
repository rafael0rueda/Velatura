//! The Shell's own stylesheet, read from where the Shell is installed and
//! restated in a theme's colours (phase 4b). The fixture bundle was made
//! by GLib's own `glib-compile-resources` from excerpts of GNOME Shell
//! 50's two stylesheets.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::host::{Dirs, Host, MemSettings, StdFs};
use velatura_engine::plan::{Change, Note};
use velatura_engine::store::State;
use velatura_engine::targets::{self, USER_THEMES};
use velatura_theme::{Appearance, Resolved, bundled, shell};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const BUNDLE: &[u8] = include_bytes!("fixtures/gnome-shell-theme.gresource");

/// A home with User Themes, and a folder standing in for the system's
/// shared data, holding the Shell's bundle when `with_bundle`.
struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new(with_bundle: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-shell-stock-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        let ext = home
            .join(".local/share/gnome-shell/extensions")
            .join(USER_THEMES);
        fs::create_dir_all(&ext).unwrap();
        fs::write(ext.join("metadata.json"), "{}").unwrap();
        fs::create_dir_all(home.join(".local/state")).unwrap();
        let system = root.join("system/gnome-shell");
        fs::create_dir_all(&system).unwrap();
        if with_bundle {
            fs::write(system.join("gnome-shell-theme.gresource"), BUNDLE).unwrap();
        }
        Self { root, dirs }
    }

    fn host<'a>(&'a self, settings: &'a MemSettings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs: &StdFs,
            settings,
            now: 1_800_000_000,
        }
    }

    fn env(&self, desktop: &'static str) -> impl Fn(&str) -> Option<OsString> + '_ {
        move |key| match key {
            "XDG_DATA_DIRS" => Some(self.root.join("system").into_os_string()),
            "XDG_CURRENT_DESKTOP" => Some(OsString::from(desktop)),
            _ => None,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// What the Shell target would write, and its notes.
fn planned(home: &Home, desktop: &'static str) -> (String, Vec<Note>) {
    let themes = bundled::themes();
    let theme = Resolved::new(&themes[1], None).unwrap();
    let settings = MemSettings::gnome();
    let env = home.env(desktop);
    let plan = targets::plan_shell(
        &home.host(&settings),
        &env,
        &theme,
        &State::default(),
        true,
        false,
    );
    assert!(plan.skipped.is_none(), "{:?}", plan.skipped);
    let css = plan
        .changes
        .iter()
        .find_map(|c| match c {
            Change::Owned {
                content: Some(bytes),
                ..
            } => Some(String::from_utf8(bytes.clone()).unwrap()),
            _ => None,
        })
        .unwrap();
    (css, plan.notes)
}

#[test]
fn the_shells_own_stylesheet_is_restated_with_the_exact_accent() {
    let home = Home::new(true);
    let (css, notes) = planned(&home, "GNOME");
    assert!(!notes.contains(&Note::ShellShortList), "{notes:?}");
    // Ember: the panel in its darkest background, toggles in its accent.
    assert!(
        css.contains("\n#panel { background-color: #110d0f; }\n"),
        "{css}"
    );
    // Its focus rings: the accent itself, in the Shell's own mix.
    assert!(css.contains("st-mix(#e0914f, #dbc7b0, 60%)"), "{css}");
    assert!(!css.contains("-st-accent"));
    // The Shell's own sizes stay its own.
    assert!(!css.contains("height") && !css.contains("padding"));
    assert!(
        css.lines()
            .nth(1)
            .unwrap()
            .contains("from the Shell's stylesheet")
    );
}

#[test]
fn where_the_stylesheet_cannot_be_read_the_short_list_is_used_and_said() {
    // No bundle, and Ubuntu's own session, whose Shell draws with Yaru.
    for (home, desktop) in [
        (Home::new(false), "GNOME"),
        (Home::new(true), "ubuntu:GNOME"),
    ] {
        let (css, notes) = planned(&home, desktop);
        assert!(notes.contains(&Note::ShellShortList), "{desktop}");
        assert!(css.contains("#panel { background-color: #110d0f; color: #dbc7b0; }"));
    }
    // A damaged bundle is the same as none.
    let home = Home::new(true);
    let bundle = home
        .root
        .join("system/gnome-shell/gnome-shell-theme.gresource");
    fs::write(&bundle, &BUNDLE[..BUNDLE.len() / 3]).unwrap();
    assert!(planned(&home, "GNOME").1.contains(&Note::ShellShortList));
}

#[test]
fn each_appearance_takes_its_own_stylesheet() {
    let home = Home::new(true);
    let settings = MemSettings::gnome();
    let env = home.env("GNOME");
    let host = home.host(&settings);
    let dark = targets::shell_stylesheet(&host, &env, Appearance::Dark).unwrap();
    let light = targets::shell_stylesheet(&host, &env, Appearance::Light).unwrap();
    assert!(dark.contains("#panel {\n  background-color: #000000;"));
    assert!(light.contains("#panel {\n  background-color: #fafafb;"));
}

/// The stylesheets of the Shell installed on this machine, when there is
/// one: CI has GNOME 47 to 51. Every rule that holds a colour is restated
/// and nothing else, with no colour of the Shell's own left.
#[test]
fn the_installed_shells_stylesheets_are_restated_whole() {
    let home = Home::new(false);
    let settings = MemSettings::gnome();
    let host = home.host(&settings);
    let system = |key: &str| match key {
        "XDG_CURRENT_DESKTOP" => Some(OsString::from("GNOME")),
        _ => None,
    };
    let themes = bundled::themes();
    for appearance in [Appearance::Dark, Appearance::Light] {
        let Some(stock) = targets::shell_stylesheet(&host, &system, appearance) else {
            return;
        };
        let mut theme = themes[1].clone();
        let variant = theme.variants.values_mut().next().unwrap();
        if appearance == Appearance::Light {
            variant.appearance = Appearance::Light;
            std::mem::swap(&mut variant.ui.bg, &mut variant.ui.fg);
        }
        let r = Resolved::new(&theme, None).unwrap();
        let css = shell::restate(&r, &stock, false).unwrap();
        let heavy = shell::restate(&r, &stock, true).unwrap();
        assert_eq!(css.lines().count(), heavy.lines().count());
        assert!(!css.contains("-st-accent"), "{appearance:?}");
        assert!(
            css.lines().count() > 400,
            "{appearance:?}: {}",
            css.lines().count()
        );
        for line in css.lines().skip(2) {
            let (selectors, body) = line.split_once(" { ").expect(line);
            assert!(
                body.ends_with("; }") && !body[..body.len() - 1].contains(['{', '}']),
                "{line}"
            );
            assert!(
                stock.contains(selectors.split(", ").next().unwrap()),
                "{line}"
            );
        }
        for grey in [
            "#222226", "#2e2e33", "#36363a", "#38383b", "#fafafb", "#282828",
        ] {
            assert!(!css.contains(grey), "{appearance:?}: {grey} is left");
        }
    }
}
