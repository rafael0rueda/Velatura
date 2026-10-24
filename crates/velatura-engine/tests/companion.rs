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
use velatura_engine::plan::{Change, Note, Skip};
use velatura_engine::schedule::{self, Auto, Period, Refused, SERVICE, TIMER, Times, WATCH};
use velatura_engine::store::DayNight;
use velatura_engine::targets::{INTERFACE_SCHEMA, SHELL, TOP_BAR, UPS, USER_THEMES};
use velatura_theme::editor::Draft;
use velatura_theme::shell::{BarStyle, TopBar};
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
    assert!(status.by_velatura && !status.differs && !status.outdated);
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
    let status = companion::status(&host, &no_env);
    assert!(status.differs && !status.outdated, "the same version");
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

/// Right after an update the Shell still runs the copy it loaded when the
/// user signed in. That is said, and the parts are written all the same,
/// so they show once the new copy runs.
#[test]
fn an_update_waits_for_signing_out_and_in() {
    let home = Home::new();
    let running = format!("extension:{UUID}");
    let meta: serde_json::Value = serde_json::from_str(companion::FILES[0].1).unwrap();
    let ours = meta["version-name"].as_str().unwrap().to_owned();
    let earlier = (ours.parse::<u64>().unwrap() - 1).to_string();
    let s = MemSettings::new(&[
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        (INTERFACE_SCHEMA, "accent-color", "blue"),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
        ("org.gnome.shell", "enabled-extensions", "@as []"),
        ("org.gnome.shell", "disable-user-extensions", "false"),
        (USER_THEME_SCHEMA, "name", ""),
        (&running, "version-name", &earlier),
    ]);
    let host = home.host(&StdFs, &s);
    companion::install(&host).unwrap();
    enable(&s, &[UUID]);
    let only = ApplyOpts {
        only: vec![TOP_BAR.to_owned()],
        ..ApplyOpts::default()
    };
    let top_bar = |host: &Host<'_>| {
        top_bar_plan(host, &only)
            .targets
            .into_iter()
            .find(|t| t.instance == TOP_BAR)
            .unwrap()
    };

    let status = companion::status(&host, &no_env);
    assert!(
        status.active() && status.waiting && !status.outdated,
        "{status:?}"
    );
    let part = top_bar(&host);
    assert_eq!(part.skipped, None);
    assert!(!part.changes.is_empty());
    assert!(
        part.notes.contains(&Note::ExtensionWaiting),
        "{:?}",
        part.notes
    );

    // Once the Shell runs it, nothing is left to wait for.
    s.write(&running, "version-name", Some(&ours)).unwrap();
    let status = companion::status(&host, &no_env);
    assert!(status.active() && !status.waiting);
    assert!(!top_bar(&host).notes.contains(&Note::ExtensionWaiting));
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
    // The contract: four files read from the user's data folder, and the
    // outline's and the UPS indicator's style classes.
    assert!(js.contains("['velatura', 'shell', 'gnome-shell.css']"));
    assert!(js.contains("['velatura', 'shell', 'top-bar.css']"));
    assert!(js.contains("['velatura', 'shell', 'ups.css']"));
    assert!(js.contains(&format!("'{}'", velatura_theme::shell::UPS_CLASS)));
    // The one thing it connects to: NUT's server, on this computer.
    assert!(js.contains("const NUT_HOST = 'localhost';"));
    assert_eq!(js.matches("connect_to_host_async(").count(), 1);
    assert!(js.contains("connect_to_host_async(NUT_HOST, NUT_PORT"));
    assert!(js.contains("['velatura', 'shell', 'window-outline.css']"));
    assert!(js.contains(&format!("'{}'", velatura_theme::shell::OUTLINE_CLASS)));
    // What it must not do: start programs, reach beyond this computer,
    // hold settings.
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
    assert!(js.contains("object.disconnect(id)") && js.contains("this._widget.destroy()"));
    assert!(js.contains("this._cancel.cancel()") && js.contains("this._ups?.destroy()"));
}

/// Every version of the extension as published, with the SHA-256 of its
/// `extension.js`. A line is never changed once its version is out: a
/// change to `extension.js` raises `version-name` in `metadata.json` and
/// adds a line here.
const RELEASED: [(u64, &str); 6] = [
    (
        1,
        "5fe303db4bb508c20caad93f7e9abe00b709713ac0bcaecd9dbf7d6b89f043c0",
    ),
    (
        2,
        "7c9fc2616dec37fded596dd1b29e30f5a5b471942a41c94d9f09e3495387be5f",
    ),
    (
        3,
        "34e478712f943d8ae7e9d54559f254e187abfaa7ecd465e8f8c64f26b51aaca8",
    ),
    (
        4,
        "9c04e8bd6790a3e8829626d1089010565960c7c7f0accc01c15442c085e59914",
    ),
    (
        5,
        "81ea761b0387496ea9790dc0f777065dba669c87452889a8f0e0ce649a410206",
    ),
    (
        6,
        "d9e9da3bc96bd7dec3b3747e28ac2a00b5422fec95bbf4289f397bd57dbc30f2",
    ),
];

/// Whether an installed copy is older is read from its `version-name`, so
/// a change to what the extension does must come with a new one, or an
/// earlier copy would count as up to date and be written for.
#[test]
fn a_changed_extension_has_a_new_version() {
    let meta: serde_json::Value = serde_json::from_str(companion::FILES[0].1).unwrap();
    let version: u64 = meta["version-name"].as_str().unwrap().parse().unwrap();
    let js = velatura_theme::sha256_hex(companion::FILES[1].1.as_bytes());
    for pair in RELEASED.windows(2) {
        assert!(pair[0].0 < pair[1].0, "versions in order, each once");
        assert!(
            RELEASED.iter().filter(|(_, sha)| *sha == pair[1].1).count() == 1,
            "each version's extension.js is its own"
        );
    }
    assert_eq!(
        RELEASED.last(),
        Some(&(version, js.as_str())),
        "extension.js or version-name changed: raise version-name in metadata.json to {} and add ({}, \"{js}\") as a new last line of RELEASED",
        RELEASED[RELEASED.len() - 1].0 + 1,
        RELEASED[RELEASED.len() - 1].0 + 1,
    );
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

    // A home folder with a space in it is written so systemd reads it right:
    // as it is, since systemd decodes no escapes there, only `%`.
    let (_, watch) =
        schedule::follow_units(&exe, Path::new("/home/a b/.config/dconf/user 100%")).unwrap();
    assert!(watch.contains("PathChanged=/home/a b/.config/dconf/user 100%%\n"));
    // What systemd would cut off or join to the next line is refused.
    for bad in [
        "/home/a/user ",
        "/home/a/user\\",
        "/home/a/us\ner",
        "home/a/user",
    ] {
        assert!(
            schedule::follow_units(&exe, Path::new(bad)).is_none(),
            "{bad:?}"
        );
    }
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

/// On a GNOME Shell that was not checked, the top bar is left alone with
/// the reason, unless it is asked for by name.
#[test]
fn an_unchecked_shell_version_is_left_alone_unless_asked_for() {
    let themes = bundled::themes();
    let resolved = Resolved::new(&themes[0], None).unwrap();
    for (version, left_alone) in [
        ("46.2", true),
        ("47.0", false),
        ("51.1", false),
        ("52.alpha", true),
        ("nonsense", false),
    ] {
        let home = Home::new();
        let s = MemSettings::new(&[
            (INTERFACE_SCHEMA, "color-scheme", "default"),
            (INTERFACE_SCHEMA, "accent-color", "blue"),
            ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
            ("org.gnome.shell", "enabled-extensions", "@as []"),
            ("org.gnome.shell", "disable-user-extensions", "false"),
            ("dbus:org.gnome.Shell", "ShellVersion", version),
        ]);
        let host = home.host(&StdFs, &s);
        companion::install(&host).unwrap();
        enable(&s, &[UUID]);
        let plan = engine::plan_apply(&host, &no_env, &resolved, &shell_only()).unwrap();
        let shell = plan.targets.iter().find(|t| t.instance == SHELL).unwrap();
        assert_eq!(
            shell.skipped == Some(Skip::UntestedShell(version.to_owned())),
            left_alone,
            "{version}: {:?}",
            shell.skipped
        );
        let asked = ApplyOpts {
            untested_shell: true,
            ..shell_only()
        };
        let plan = engine::plan_apply(&host, &no_env, &resolved, &asked).unwrap();
        let shell = plan.targets.iter().find(|t| t.instance == SHELL).unwrap();
        assert_eq!(shell.skipped, None, "{version}");
    }
}

fn top_bar_plan(host: &Host<'_>, opts: &ApplyOpts) -> velatura_engine::plan::Plan {
    let themes = bundled::themes();
    let resolved = Resolved::new(&themes[0], None).unwrap();
    engine::plan_apply(host, &no_env, &resolved, opts).unwrap()
}

#[test]
fn the_top_bar_is_drawn_as_chosen_and_taken_away_once_turned_off() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let css = companion::top_bar_stylesheet(&host);
    assert_eq!(css, home.path(".local/share/velatura/shell/top-bar.css"));
    let only = ApplyOpts {
        only: vec![TOP_BAR.to_owned()],
        ..ApplyOpts::default()
    };
    let part = |plan: &velatura_engine::plan::Plan| {
        plan.targets
            .iter()
            .find(|t| t.instance == TOP_BAR)
            .unwrap()
            .clone()
    };

    // Off until turned on, and only Velatura's extension can show it.
    assert_eq!(
        part(&top_bar_plan(&host, &ApplyOpts::default())).skipped,
        Some(Skip::NotSelected)
    );
    assert_eq!(
        part(&top_bar_plan(&host, &only)).skipped,
        Some(Skip::NeedsVelaturaExtension)
    );
    companion::install(&host).unwrap();
    enable(&s, &[UUID]);

    // Turned on: pills at 60 %, the defaults.
    assert_eq!(engine::top_bar(&host).unwrap(), TopBar::default());
    engine::set_included(&host, TOP_BAR, true).unwrap();
    let plan = top_bar_plan(&host, &ApplyOpts::default());
    assert!(part(&plan).skipped.is_none());
    engine::execute(&host, &plan, &[]).unwrap();
    let pills = fs::read_to_string(&css).unwrap();
    assert!(pills.contains("top bar pills 60"));
    assert!(pills.contains(".panel-button"));
    assert!(
        part(&top_bar_plan(&host, &ApplyOpts::default()))
            .changes
            .is_empty()
    );

    // A new choice is a change for the next apply, and is kept.
    let bar = TopBar {
        style: BarStyle::Bar,
        opacity: 200,
    };
    engine::set_top_bar(&host, bar).unwrap();
    assert_eq!(
        engine::top_bar(&host).unwrap(),
        TopBar {
            style: BarStyle::Bar,
            opacity: 100
        }
    );
    let plan = top_bar_plan(&host, &ApplyOpts::default());
    assert!(!part(&plan).changes.is_empty());
    engine::execute(&host, &plan, &[]).unwrap();
    let full = fs::read_to_string(&css).unwrap();
    assert!(full.contains("top bar bar 100") && !full.contains(".panel-button"));

    // Undo brings the pills' file back; the choice stays as it was made.
    let undo = engine::plan_undo(&host).unwrap();
    engine::execute(&host, &undo, &[]).unwrap();
    assert_eq!(fs::read_to_string(&css).unwrap(), pills);
    assert_eq!(engine::top_bar(&host).unwrap().style, BarStyle::Bar);

    // Turned off: a scheduled switch leaves the file, an apply by hand
    // takes it away.
    engine::set_included(&host, TOP_BAR, false).unwrap();
    let scheduled = ApplyOpts {
        unattended: true,
        ..ApplyOpts::default()
    };
    assert_eq!(
        part(&top_bar_plan(&host, &scheduled)).skipped,
        Some(Skip::NotSelected)
    );
    let plan = top_bar_plan(&host, &ApplyOpts::default());
    assert!(part(&plan).changes.iter().any(|c| matches!(
        c,
        Change::Owned { path, content: None, .. } if *path == css
    )));
    engine::execute(&host, &plan, &[]).unwrap();
    assert!(!css.exists());
    assert_eq!(
        part(&top_bar_plan(&host, &ApplyOpts::default())).skipped,
        Some(Skip::NotSelected)
    );
}

#[test]
fn an_older_extension_cannot_show_the_top_bar() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    companion::install(&host).unwrap();
    enable(&s, &[UUID]);
    let js = home.path(&format!(
        ".local/share/gnome-shell/extensions/{UUID}/extension.js"
    ));
    let metadata = js.with_file_name("metadata.json");
    let ours = fs::read_to_string(&metadata).unwrap();
    let only = ApplyOpts {
        only: vec![TOP_BAR.to_owned()],
        ..ApplyOpts::default()
    };
    let skipped = |host: &velatura_engine::host::Host<'_>| {
        let plan = top_bar_plan(host, &only);
        plan.targets
            .iter()
            .find(|t| t.instance == TOP_BAR)
            .unwrap()
            .skipped
            .clone()
    };

    // The same version with another description, as after 0.2.0's
    // metadata was reworded: up to date enough, and offered to update.
    fs::write(&metadata, ours.replace("Loads the colors", "Loads colors")).unwrap();
    let status = companion::status(&host, &no_env);
    assert!(status.differs && !status.outdated);
    assert_ne!(skipped(&host), Some(Skip::VelaturaExtensionOutdated));

    // An earlier version: one line less, and a lower version number.
    let older = fs::read_to_string(&js)
        .unwrap()
        .replace("        new Sheet(TOP_BAR_SHEET),\n", "");
    assert_ne!(older, fs::read_to_string(&js).unwrap());
    fs::write(&js, older).unwrap();
    let version = |v: &str| format!("\"version-name\": \"{v}\"");
    let now = ours
        .lines()
        .find_map(|l| l.trim().strip_prefix("\"version-name\": \""))
        .and_then(|v| v.strip_suffix('"'))
        .unwrap()
        .to_owned();
    let earlier = (now.parse::<u64>().unwrap() - 1).to_string();
    fs::write(&metadata, ours.replace(&version(&now), &version(&earlier))).unwrap();
    assert!(companion::status(&host, &no_env).outdated);
    assert_eq!(skipped(&host), Some(Skip::VelaturaExtensionOutdated));
}

#[test]
fn the_ups_indicator_has_its_stylesheet_while_on() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let css = companion::ups_stylesheet(&host);
    assert_eq!(css, home.path(".local/share/velatura/shell/ups.css"));
    companion::install(&host).unwrap();
    enable(&s, &[UUID]);
    let part = |plan: &velatura_engine::plan::Plan| {
        plan.targets
            .iter()
            .find(|t| t.instance == UPS)
            .unwrap()
            .clone()
    };
    assert_eq!(
        part(&top_bar_plan(&host, &ApplyOpts::default())).skipped,
        Some(Skip::NotSelected)
    );
    engine::set_included(&host, UPS, true).unwrap();
    let plan = top_bar_plan(&host, &ApplyOpts::default());
    engine::execute(&host, &plan, &[]).unwrap();
    let written = fs::read_to_string(&css).unwrap();
    assert!(written.contains(".velatura-ups.on-battery"));

    // Turned off, the file goes, and with it the indicator.
    engine::set_included(&host, UPS, false).unwrap();
    let plan = top_bar_plan(&host, &ApplyOpts::default());
    engine::execute(&host, &plan, &[]).unwrap();
    assert!(!css.exists());
}

/// A copy of a later version, as a later Velatura installs, is not
/// replaced by this build's earlier one, nor offered to be.
#[test]
fn a_later_extension_is_kept() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    companion::install(&host).unwrap();
    enable(&s, &[UUID]);
    let js = home.path(&format!(
        ".local/share/gnome-shell/extensions/{UUID}/extension.js"
    ));
    let metadata = js.with_file_name("metadata.json");
    let ours = fs::read_to_string(&metadata).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&ours).unwrap();
    let now = meta["version-name"].as_str().unwrap();
    let later = (now.parse::<u64>().unwrap() + 1).to_string();
    let later_meta = ours.replace(
        &format!("\"version-name\": \"{now}\""),
        &format!("\"version-name\": \"{later}\""),
    );
    assert_ne!(later_meta, ours);
    fs::write(&metadata, &later_meta).unwrap();
    fs::write(&js, "// what a later version does\n").unwrap();

    let status = companion::status(&host, &no_env);
    assert!(status.by_velatura && status.active());
    assert!(!status.differs && !status.outdated, "{status:?}");
    assert!(!companion::install(&host).unwrap(), "kept");
    assert_eq!(fs::read_to_string(&metadata).unwrap(), later_meta);
    assert_eq!(
        fs::read_to_string(&js).unwrap(),
        "// what a later version does\n"
    );
    // And the parts that need the extension are written for it.
    let only = ApplyOpts {
        only: vec![TOP_BAR.to_owned()],
        ..ApplyOpts::default()
    };
    let plan = top_bar_plan(&host, &only);
    let top_bar = plan.targets.iter().find(|t| t.instance == TOP_BAR).unwrap();
    assert_ne!(top_bar.skipped, Some(Skip::VelaturaExtensionOutdated));
    // Removing it is still Velatura's to do.
    assert!(companion::remove(&host).unwrap().is_some());
}
