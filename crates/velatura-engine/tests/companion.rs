//! Velatura's own Shell extension, and following the desktop's dark style:
//! installed and removed, written for by the Shell target, and switched
//! between, in a throw-away home folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::companion::{self, UUID};
use velatura_engine::engine::{self, ApplyOpts};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, MemSystemd, Settings, StdFs};
use velatura_engine::library;
use velatura_engine::plan::{Change, Skip};
use velatura_engine::schedule::{self, Auto, Period, Refused, SERVICE, TIMER, Times, WATCH};
use velatura_engine::store::DayNight;
use velatura_engine::targets::{INTERFACE_SCHEMA, SHELL, USER_THEMES};
use velatura_theme::editor::Draft;
use velatura_theme::{Appearance, Color, Resolved, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);
const USER_THEME_SCHEMA: &str = "org.gnome.shell.extensions.user-theme";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-companion-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        let h = Self { root, dirs };
        for base in [".config", ".local/share", ".local/state", ".local/bin"] {
            fs::create_dir_all(h.path(base)).unwrap();
        }
        fs::write(h.path(".local/bin/velatura"), "").unwrap();
        h
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dirs.home.join(rel)
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    fn user_files(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                // Velatura's own records, but not what it writes for the Shell.
                if rel.contains("/velatura/backups") || rel.contains(".local/state/velatura") {
                    continue;
                }
                if path.is_dir() {
                    out.insert(format!("{rel}/"), Vec::new());
                    walk(&path, base, out);
                } else {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.dirs.home, &self.dirs.home, &mut out);
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn no_env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| OsString::from("/nonexistent-velatura-test"))
}

fn settings() -> MemSettings {
    MemSettings::new(&[
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        (INTERFACE_SCHEMA, "accent-color", "blue"),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
        ("org.gnome.shell", "enabled-extensions", "@as []"),
        ("org.gnome.shell", "disable-user-extensions", "false"),
        (USER_THEME_SCHEMA, "name", ""),
    ])
}

fn enable(s: &MemSettings, uuids: &[&str]) {
    let list: Vec<String> = uuids.iter().map(|u| format!("'{u}'")).collect();
    s.write(
        "org.gnome.shell",
        "enabled-extensions",
        Some(&format!("[{}]", list.join(", "))),
    )
    .unwrap();
}

fn shell_only() -> ApplyOpts {
    ApplyOpts {
        only: vec![SHELL.to_owned(), "gnome:appearance".to_owned()],
        ..ApplyOpts::default()
    }
}

fn apply(host: &Host<'_>, index: usize) -> engine::Report {
    let themes = bundled::themes();
    let resolved = Resolved::new(&themes[index], None).unwrap();
    let plan = engine::plan_apply(host, &no_env, &resolved, &shell_only()).unwrap();
    engine::execute(host, &plan, &[]).unwrap()
}

#[test]
fn the_extension_is_installed_as_published_and_removed_without_a_trace() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let before = home.user_files();
    let dir = home.path(&format!(".local/share/gnome-shell/extensions/{UUID}"));

    let status = companion::status(&host, &no_env);
    assert!(status.installed.is_none() && !status.active());

    assert!(companion::install(&host).unwrap());
    for (name, text) in companion::FILES {
        assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), text);
    }
    let status = companion::status(&host, &no_env);
    assert_eq!(status.installed.as_deref(), Some(dir.as_path()));
    assert!(status.by_velatura && !status.outdated);
    // Installed is not on: turning it on is the user's to do.
    assert_eq!(status.enabled, Some(false));
    assert!(!status.active());
    assert!(!companion::install(&host).unwrap(), "already there");

    // An older copy of Velatura's own is brought up to date.
    fs::write(dir.join("extension.js"), "// an older version\n").unwrap();
    // ...but only if it is the copy Velatura wrote.
    assert!(matches!(
        companion::install(&host),
        Err(Refused::Foreign(_))
    ));
    assert!(companion::status(&host, &no_env).outdated);
    let kept = companion::remove(&host).unwrap().unwrap();
    assert_eq!(kept, [dir.join("extension.js")]);
    assert!(dir.join("extension.js").exists() && !dir.join("metadata.json").exists());
    fs::remove_dir_all(home.path(".local/share/gnome-shell")).unwrap();

    assert!(companion::install(&host).unwrap());
    assert_eq!(companion::remove(&host).unwrap(), Some(Vec::new()));
    assert_eq!(companion::remove(&host).unwrap(), None);
    assert_eq!(home.user_files(), before);

    // Someone else's folder of that name is left alone.
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("metadata.json"), "{}").unwrap();
    assert!(matches!(
        companion::install(&host),
        Err(Refused::Foreign(_))
    ));
    assert_eq!(fs::read_to_string(dir.join("metadata.json")).unwrap(), "{}");
    let status = companion::status(&host, &no_env);
    assert!(status.installed.is_some() && !status.by_velatura);
}

#[test]
fn what_is_published_is_two_files_that_say_what_they_are() {
    let (meta, js) = (companion::FILES[0].1, companion::FILES[1].1);
    let v: serde_json::Value = serde_json::from_str(meta).unwrap();
    assert_eq!(v["uuid"], UUID);
    for version in ["47", "48", "49", "50", "51"] {
        assert!(
            v["shell-version"]
                .as_array()
                .unwrap()
                .contains(&version.into())
        );
    }
    // The contract: the one file, read from the user's data folder.
    assert!(js.contains("['velatura', 'shell', 'gnome-shell.css']"));
    // What it must not do: start programs, reach the network, hold settings.
    for forbidden in [
        "Subprocess",
        "spawn",
        "Soup",
        "getSettings",
        "eval(",
        "imports.",
    ] {
        assert!(!js.contains(forbidden), "{forbidden}");
    }
    // Everything it connects or schedules in enable() it undoes in disable().
    assert!(js.contains("source_remove") && js.contains("this._monitor.cancel()"));
    assert!(js.contains("this._monitor.disconnect(this._changedId)"));
}

#[test]
fn with_the_extension_on_the_shell_gets_one_stylesheet_in_one_place() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let css = companion::stylesheet(&host);
    assert_eq!(
        css,
        home.path(".local/share/velatura/shell/gnome-shell.css")
    );

    // Neither way into the Shell: left out, with the reason.
    let report = apply(&host, 0);
    assert!(
        report
            .skipped
            .contains(&(SHELL.to_owned(), Skip::NeedsUserThemes))
    );
    engine::undo_last(&host).unwrap();

    companion::install(&host).unwrap();
    // Installed but off is still not a way in.
    assert!(apply(&host, 0).skipped.iter().any(|(i, _)| i == SHELL));
    engine::undo_last(&host).unwrap();
    enable(&s, &[UUID]);
    assert!(companion::status(&host, &no_env).active());
    let start = (home.user_files(), s.snapshot());

    let report = apply(&host, 1);
    assert!(!report.skipped.iter().any(|(i, _)| i == SHELL));
    let ember = fs::read_to_string(&css).unwrap();
    assert!(ember.contains("#panel"), "{ember}");
    // No folder per theme and no setting: the extension watches the file.
    assert!(!home.path(".local/share/themes").exists());
    assert_eq!(
        s.read(USER_THEME_SCHEMA, "name").unwrap().unwrap().user,
        None
    );

    // Another theme replaces the file in place.
    let report = apply(&host, 0);
    assert!(report.changes.iter().any(|c| matches!(
        c,
        Change::Owned { path, pre: Some(_), content: Some(_) } if *path == css
    )));
    assert_ne!(fs::read_to_string(&css).unwrap(), ember);
    assert!(apply(&host, 0).changes.is_empty(), "twice changes nothing");

    engine::undo_last(&host).unwrap();
    assert_eq!(fs::read_to_string(&css).unwrap(), ember);
    engine::undo_last(&host).unwrap();
    assert_eq!((home.user_files(), s.snapshot()), start);

    // A stylesheet there that Velatura did not write is not replaced.
    fs::create_dir_all(css.parent().unwrap()).unwrap();
    fs::write(&css, "/* mine */").unwrap();
    assert!(
        apply(&host, 0)
            .skipped
            .iter()
            .any(|(i, why)| i == SHELL && matches!(why, Skip::ForeignFile(_)))
    );
    assert_eq!(fs::read_to_string(&css).unwrap(), "/* mine */");
}

#[test]
fn moving_between_user_themes_and_the_extension_leaves_one_theme_behind() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    fs::create_dir_all(home.path(&format!(
        ".local/share/gnome-shell/extensions/{USER_THEMES}"
    )))
    .unwrap();
    enable(&s, &[USER_THEMES]);
    let css = companion::stylesheet(&host);
    let start = (home.user_files(), s.snapshot());

    apply(&host, 1);
    let name = s
        .read(USER_THEME_SCHEMA, "name")
        .unwrap()
        .unwrap()
        .user
        .unwrap();
    let folder = home.path(&format!(".local/share/themes/{name}"));
    assert!(folder.join("gnome-shell/gnome-shell.css").exists());
    let by_user_themes = (home.user_files(), s.snapshot());

    // The user installs and turns on Velatura's extension. The next apply
    // writes for it and clears what was made for User Themes.
    companion::install(&host).unwrap();
    enable(&s, &[USER_THEMES, UUID]);
    apply(&host, 0);
    assert!(css.exists());
    assert!(!folder.exists(), "the generated folder is cleared");
    assert_eq!(
        s.read(USER_THEME_SCHEMA, "name").unwrap().unwrap().user,
        None
    );

    // Turned off again: back to User Themes, and the stylesheet goes.
    enable(&s, &[USER_THEMES]);
    apply(&host, 1);
    assert!(!css.exists() && !css.parent().unwrap().exists());
    assert_eq!(
        s.read(USER_THEME_SCHEMA, "name")
            .unwrap()
            .unwrap()
            .user
            .as_deref(),
        Some(name.as_str())
    );

    // Undo walks back through both moves exactly.
    engine::undo_last(&host).unwrap();
    engine::undo_last(&host).unwrap();
    let mut now = home.user_files();
    now.retain(|k, _| !k.contains(UUID) && !k.ends_with("gnome-shell/extensions/"));
    let mut want = by_user_themes.0.clone();
    want.retain(|k, _| !k.ends_with("gnome-shell/extensions/"));
    assert_eq!(now, want);
    engine::undo_last(&host).unwrap();
    companion::remove(&host).unwrap();
    // All that is left is Velatura's own folder, which holds its backups.
    let mut now = home.user_files();
    now.remove(".local/share/velatura/");
    assert_eq!(now, start.0);
}

#[test]
fn exhaustive_crash_test_through_the_extension() {
    let mut crash_points = 0;
    for fail_at in 0.. {
        let home = Home::new();
        let s = settings();
        let clean = home.host(&StdFs, &s);
        companion::install(&clean).unwrap();
        enable(&s, &[UUID]);
        apply(&clean, 1);
        let before = (home.user_files(), s.snapshot());
        let themes = bundled::themes();
        let resolved = Resolved::new(&themes[0], None).unwrap();
        let plan = engine::plan_apply(&clean, &no_env, &resolved, &shell_only()).unwrap();
        let faults = Faults::new(&StdFs, &s, fail_at);
        let dying = home.host(&faults, &faults);
        if engine::execute(&dying, &plan, &[]).is_ok() {
            assert_ne!((home.user_files(), s.snapshot()), before);
            break;
        }
        crash_points += 1;
        if let Some(r) = engine::recover(&clean).unwrap() {
            assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
        }
        assert_eq!((home.user_files(), s.snapshot()), before, "step {fail_at}");
    }
    assert!(crash_points >= 3, "only {crash_points} crash points");
}

/// A light theme of the user's, to pair with a dark one.
fn light_theme(host: &Host<'_>) {
    let c = |hex| Color::parse(hex).unwrap();
    let draft = Draft::new(
        "paper",
        "Paper",
        Appearance::Light,
        [c("#fafafa"), c("#202020"), c("#3584e4")],
    );
    library::save(host, &draft.theme, None, false).unwrap();
}

fn choose(host: &Host<'_>, day: &str, night: &str) {
    DayNight {
        day: Some(day.to_owned()),
        night: Some(night.to_owned()),
    }
    .save(host)
    .unwrap();
}

#[test]
fn following_the_style_needs_a_light_and_a_dark_theme_and_replaces_the_clock() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let sd = MemSystemd::default();
    let exe = home.path(".local/bin/velatura");
    let units = home.path(".config/systemd/user");
    let before = home.user_files();

    // Two dark themes are not a pair, whichever way round.
    choose(&host, "ember", "aetherius");
    assert!(matches!(
        schedule::enable_follow(&host, &sd, &exe, false),
        Err(Refused::NotAPair)
    ));
    light_theme(&host);
    choose(&host, "ember", "paper");
    assert!(matches!(
        schedule::enable_follow(&host, &sd, &exe, false),
        Err(Refused::NotAPair)
    ));
    assert!(sd.calls.borrow().is_empty() && !units.exists());

    choose(&host, "paper", "ember");
    schedule::enable_follow(&host, &sd, &exe, false).unwrap();
    let watch = fs::read_to_string(units.join(WATCH)).unwrap();
    assert!(
        watch.contains(&format!(
            "PathChanged={}\nUnit={SERVICE}\n",
            home.path(".config/dconf/user").display()
        )),
        "{watch}"
    );
    let service = fs::read_to_string(units.join(SERVICE)).unwrap();
    assert!(service.contains("switch auto") && service.contains("WantedBy=default.target"));
    // Without this a burst of setting changes stops the watching for good:
    // seen on a real session, 2026-10-13.
    assert!(
        service.contains("[Unit]\nStartLimitIntervalSec=0\n"),
        "{service}"
    );
    assert_eq!(
        *sd.calls.borrow(),
        [
            "daemon-reload".to_owned(),
            format!("enable {WATCH} {SERVICE}"),
            format!("restart {WATCH}"),
        ]
    );
    let status = schedule::status(&host);
    assert!(status.on && status.follows_style && !status.damaged);
    assert!(schedule::follows_style(&host));

    // The clock takes over: the watching unit goes, the timer comes.
    schedule::enable(&host, &sd, &exe, Times::default(), false).unwrap();
    assert!(!units.join(WATCH).exists() && units.join(TIMER).exists());
    assert!(
        sd.calls
            .borrow()
            .contains(&format!("disable --now {WATCH} {SERVICE}"))
    );
    assert!(!schedule::status(&host).follows_style);
    // And back.
    schedule::enable_follow(&host, &sd, &exe, false).unwrap();
    assert!(units.join(WATCH).exists() && !units.join(TIMER).exists());

    schedule::disable(&host, &sd).unwrap();
    assert!(!schedule::status(&host).on && !units.exists());
    let mut after = home.user_files();
    after.retain(|k, _| !k.contains("velatura"));
    let mut want = before;
    want.retain(|k, _| !k.contains("velatura"));
    assert_eq!(after, want);

    // A home folder with a space in it is written so systemd reads it right.
    let (_, watch) =
        schedule::follow_units(&exe, Path::new("/home/a b/.config/dconf/user 100%")).unwrap();
    assert!(watch.contains("PathChanged=/home/a\\x20b/.config/dconf/user\\x20100%%\n"));
}

#[test]
fn the_theme_follows_the_style_and_never_sets_it() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let sd = MemSystemd::default();
    light_theme(&host);
    choose(&host, "paper", "ember");
    schedule::enable_follow(&host, &sd, &home.path(".local/bin/velatura"), false).unwrap();
    let auto = |minute| schedule::run_auto(&host, &no_env, minute, false).unwrap();
    let applied = || engine::status(&host).unwrap().applied.unwrap().0;
    let style = || {
        s.read(INTERFACE_SCHEMA, "color-scheme")
            .unwrap()
            .unwrap()
            .user
    };

    // Light style, whatever the hour: the daytime theme.
    assert!(matches!(
        auto(23 * 60),
        Auto::Done {
            period: Period::Day,
            ..
        }
    ));
    assert_eq!(applied(), "paper/default");
    assert_eq!(style(), None, "the style was not written");
    // Any other setting changing starts the command again: nothing to do.
    assert!(matches!(auto(23 * 60 + 5), Auto::NotDue(Period::Day)));

    // The user turns the dark style on in GNOME Settings.
    s.write(INTERFACE_SCHEMA, "color-scheme", Some("prefer-dark"))
        .unwrap();
    assert!(matches!(
        auto(12 * 60),
        Auto::Done {
            period: Period::Night,
            ..
        }
    ));
    assert_eq!(applied(), "ember/default");
    assert_eq!(style().as_deref(), Some("prefer-dark"));

    // A theme applied by hand keeps the style too, and stays until the
    // style changes again: the light theme under the dark style, as asked.
    let themes = library::load(&host).0;
    let paper = themes.iter().find(|e| e.theme.id == "paper").unwrap();
    let resolved = Resolved::new(&paper.theme, None).unwrap();
    let only = ApplyOpts {
        only: vec!["gnome:appearance".to_owned()],
        ..ApplyOpts::default()
    };
    let plan = engine::plan_apply(&host, &no_env, &resolved, &only).unwrap();
    assert!(!plan.changes().any(|c| matches!(
        c,
        Change::Setting { key, .. } if key == "color-scheme"
    )));
    engine::execute(&host, &plan, &[]).unwrap();
    assert_eq!(style().as_deref(), Some("prefer-dark"));
    assert!(matches!(auto(12 * 60 + 1), Auto::NotDue(Period::Night)));
    assert_eq!(applied(), "paper/default");

    s.write(INTERFACE_SCHEMA, "color-scheme", Some("default"))
        .unwrap();
    assert!(matches!(
        auto(3 * 60),
        Auto::Done {
            period: Period::Day,
            ..
        }
    ));

    // With following off, applying a theme sets the style again.
    schedule::disable(&host, &sd).unwrap();
    let ember = &bundled::themes()[1];
    let resolved = Resolved::new(ember, None).unwrap();
    let plan = engine::plan_apply(&host, &no_env, &resolved, &only).unwrap();
    engine::execute(&host, &plan, &[]).unwrap();
    assert_eq!(style().as_deref(), Some("prefer-dark"));
}
