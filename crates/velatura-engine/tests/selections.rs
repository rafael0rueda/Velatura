//! Icons, cursor, wallpaper and fonts, and the Flatpak overrides: the same
//! transaction tests as every other target, in a throw-away home folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Health};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::look::{self, BACKGROUND_SCHEMA, Kind, Rejected};
use velatura_engine::plan::{Change, Note, Plan, Skip};
use velatura_engine::targets::{FLATPAK, INTERFACE_SCHEMA};
use velatura_theme::{Resolved, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Home {
    root: PathBuf,
    dirs: Dirs,
    /// Stands in for /usr/share.
    system: PathBuf,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-select-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        let h = Self {
            system: root.join("usr-share"),
            root,
            dirs,
        };
        for base in [".config", ".local/share", ".local/state"] {
            fs::create_dir_all(h.dirs.home.join(base)).unwrap();
        }
        h
    }

    fn write(&self, path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    fn env(&self) -> impl Fn(&str) -> Option<OsString> + '_ {
        move |key| (key == "XDG_DATA_DIRS").then(|| OsString::from(&self.system))
    }

    fn overrides(&self) -> PathBuf {
        self.dirs.data.join("flatpak/overrides/global")
    }

    /// Every file and folder that is not Velatura's own record keeping.
    fn user_files(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if rel.contains("/velatura/") || rel.ends_with("/velatura") {
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

fn settings() -> MemSettings {
    MemSettings::new(&[
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        (INTERFACE_SCHEMA, "accent-color", "blue"),
        (INTERFACE_SCHEMA, "gtk-theme", "Adwaita"),
        (INTERFACE_SCHEMA, "icon-theme", "Adwaita"),
        (INTERFACE_SCHEMA, "cursor-theme", "Adwaita"),
        (INTERFACE_SCHEMA, "font-name", "Adwaita Sans 11"),
        (INTERFACE_SCHEMA, "document-font-name", "Adwaita Sans 11"),
        (INTERFACE_SCHEMA, "monospace-font-name", "Adwaita Mono 11"),
        (
            BACKGROUND_SCHEMA,
            "picture-uri",
            "file:///usr/share/backgrounds/gnome/adwaita-l.jxl",
        ),
        (
            BACKGROUND_SCHEMA,
            "picture-uri-dark",
            "file:///usr/share/backgrounds/gnome/adwaita-d.jxl",
        ),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
    ])
}

/// Two icon themes, one of them the user's; a cursor theme; a theme that
/// is both; and wallpapers.
fn install(home: &Home) {
    let icons = home.system.join("icons");
    home.write(
        &icons.join("Adwaita/index.theme"),
        "[Icon Theme]\nName=Adwaita\nDirectories=16x16/apps\n",
    );
    home.write(&icons.join("Adwaita/cursors/left_ptr"), "");
    home.write(
        &icons.join("hicolor/index.theme"),
        "[Icon Theme]\nName=Hicolor\nDirectories=a\n",
    );
    home.write(
        &icons.join("default/index.theme"),
        "[Icon Theme]\nInherits=Adwaita\n",
    );
    home.write(&icons.join("Pointers/cursors/left_ptr"), "");
    home.write(
        &icons.join("Pointers/index.theme"),
        "[Icon Theme]\nName=Big Pointers\n",
    );
    home.write(
        &home.dirs.data.join("icons/Tela-purple/index.theme"),
        "[Icon Theme]\nName=Tela \u{1b}[31mPurple\nDirectories=scalable/apps\n",
    );
    home.write(&home.system.join("backgrounds/gnome/blobs-l.svg"), "<svg/>");
    home.write(
        &home.system.join("backgrounds/gnome/notes.txt"),
        "not a picture",
    );
    home.write(
        &home.system.join("backgrounds/f44/default/f44 day.png"),
        "png",
    );
}

fn run(host: &Host<'_>, plan: &Plan) -> engine::Report {
    let all: Vec<String> = plan
        .needing_consent()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    engine::execute(host, plan, &all).unwrap()
}

fn value(s: &MemSettings, schema: &str, key: &str) -> Option<String> {
    s.read(schema, key).unwrap().unwrap().user
}

#[test]
fn what_is_installed_is_listed_by_name_with_text_cleaned() {
    let home = Home::new();
    install(&home);
    let s = settings();
    let host = home.host(&StdFs, &s);
    let env = home.env();

    let icons = look::available(&host, &env, Kind::Icons);
    let shown: Vec<(&str, &str)> = icons
        .iter()
        .map(|c| (c.value.as_str(), c.name.as_str()))
        .collect();
    // No fallbacks, no cursor-only theme, and no escape sequence in a name.
    assert_eq!(
        shown,
        [("Adwaita", "Adwaita"), ("Tela-purple", "Tela [31mPurple")]
    );
    let cursors = look::available(&host, &env, Kind::Cursor);
    let shown: Vec<&str> = cursors.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(shown, ["Adwaita", "Pointers"]);
    assert_eq!(cursors[1].name, "Big Pointers");

    let walls = look::available(&host, &env, Kind::Wallpaper);
    let shown: Vec<(&str, &str)> = walls
        .iter()
        .map(|c| (c.name.as_str(), c.value.as_str()))
        .collect();
    let base = look::file_uri(&home.system);
    assert_eq!(
        shown,
        [
            (
                "blobs-l (gnome)",
                format!("{base}/backgrounds/gnome/blobs-l.svg").as_str()
            ),
            (
                "f44 day (default)",
                format!("{base}/backgrounds/f44/default/f44%20day.png").as_str()
            ),
        ]
    );
    assert!(look::available(&host, &env, Kind::Font).is_empty());
    assert_eq!(
        look::current(&host, Kind::Icons).as_deref(),
        Some("Adwaita")
    );
    assert!(look::installed(&host, &env, Kind::Icons, "Tela-purple"));
    assert!(!look::installed(&host, &env, Kind::Icons, "Pointers"));
    assert_eq!(
        look::file_uri(Path::new("/a b/ñ#?.png")),
        "file:///a%20b/%C3%B1%23%3F.png"
    );
}

#[test]
fn a_choice_is_applied_backed_up_undone_and_keeps_the_theme() {
    let home = Home::new();
    install(&home);
    let s = settings();
    let host = home.host(&StdFs, &s);
    let env = home.env();
    let themes = bundled::themes();
    let ember = Resolved::new(&themes[1], None).unwrap();
    let only = ApplyOpts {
        only: vec!["gnome:appearance".to_owned()],
        ..ApplyOpts::default()
    };
    run(
        &host,
        &engine::plan_apply(&host, &env, &ember, &only).unwrap(),
    );
    let themed = s.snapshot();

    let plan = look::plan_select(&host, &env, Kind::Icons, Some("Tela-purple")).unwrap();
    assert_eq!(plan.changes().count(), 1);
    assert_eq!(s.snapshot(), themed, "planning changes nothing");
    let report = run(&host, &plan);
    assert!(report.backup.is_some());
    assert_eq!(
        value(&s, INTERFACE_SCHEMA, "icon-theme").as_deref(),
        Some("Tela-purple")
    );
    // The theme in use is still the theme in use.
    assert_eq!(
        engine::status(&host).unwrap().applied.unwrap().0,
        "ember/default"
    );
    let (history, _) = engine::history(&host).unwrap();
    assert_eq!((history[0].cause.as_str(), history.len()), ("select", 2));
    // The same choice again is nothing to do.
    assert!(
        look::plan_select(&host, &env, Kind::Icons, Some("Tela-purple"))
            .unwrap()
            .is_empty()
    );

    // One picture goes to both wallpaper settings; chosen by the name shown.
    let plan = look::plan_select(&host, &env, Kind::Wallpaper, Some("f44 day (default)")).unwrap();
    assert_eq!(plan.changes().count(), 2);
    run(&host, &plan);
    let uri = value(&s, BACKGROUND_SCHEMA, "picture-uri").unwrap();
    assert!(
        uri.ends_with("/backgrounds/f44/default/f44%20day.png"),
        "{uri}"
    );
    assert_eq!(
        value(&s, BACKGROUND_SCHEMA, "picture-uri-dark").as_deref(),
        Some(uri.as_str())
    );

    run(
        &host,
        &look::plan_select(&host, &env, Kind::MonospaceFont, Some("Fira Code 12")).unwrap(),
    );
    assert_eq!(
        value(&s, INTERFACE_SCHEMA, "monospace-font-name").as_deref(),
        Some("Fira Code 12")
    );

    // Back to the desktop's default, which is "not set", not a value.
    run(
        &host,
        &look::plan_select(&host, &env, Kind::Icons, None).unwrap(),
    );
    assert_eq!(value(&s, INTERFACE_SCHEMA, "icon-theme"), None);

    // Applying another theme leaves the choices alone.
    let aetherius = Resolved::new(&themes[0], None).unwrap();
    run(
        &host,
        &engine::plan_apply(&host, &env, &aetherius, &only).unwrap(),
    );
    assert_eq!(
        value(&s, INTERFACE_SCHEMA, "monospace-font-name").as_deref(),
        Some("Fira Code 12")
    );

    // Undo walks back through all of it, and removing everything puts the
    // choices back with the rest.
    engine::undo_last(&host).unwrap();
    engine::undo_last(&host).unwrap();
    assert_eq!(
        value(&s, INTERFACE_SCHEMA, "icon-theme").as_deref(),
        Some("Tela-purple")
    );
    run(&host, &engine::plan_restore_original(&host).unwrap());
    assert!(s.snapshot().is_empty(), "{:?}", s.snapshot());
}

#[test]
fn only_what_is_installed_can_be_chosen() {
    let home = Home::new();
    install(&home);
    let s = settings();
    let host = home.host(&StdFs, &s);
    let env = home.env();
    let no = |kind, wanted: &str| look::plan_select(&host, &env, kind, Some(wanted));

    for bad in [
        "Missing",
        "../Adwaita",
        "/usr/share/icons/Adwaita",
        "",
        "hicolor",
        "Pointers",
    ] {
        assert!(
            matches!(no(Kind::Icons, bad), Err(Rejected::NotInstalled)),
            "{bad:?}"
        );
    }
    // A picture of the user's own, by its full path; nothing else.
    let mine = home.dirs.home.join("Pictures/mine.jpg");
    home.write(&mine, "jpg");
    home.write(&home.dirs.home.join("Pictures/notes.txt"), "x");
    let plan = no(Kind::Wallpaper, mine.to_str().unwrap()).unwrap();
    assert!(matches!(
        plan.changes().next(),
        Some(Change::Setting { value: Some(v), .. }) if v.ends_with("/Pictures/mine.jpg")
    ));
    let txt = home.dirs.home.join("Pictures/notes.txt");
    let via = home.dirs.home.join("Pictures/../Pictures/mine.jpg");
    for bad in [
        txt.to_str().unwrap(),
        via.to_str().unwrap(),
        "Pictures/mine.jpg",
        "/nonexistent.png",
    ] {
        assert!(
            matches!(no(Kind::Wallpaper, bad), Err(Rejected::NotInstalled)),
            "{bad:?}"
        );
    }
    for bad in [
        "",
        "Fira",
        "Fira Code",
        "Fira Code 0",
        "Fira\nCode 11",
        "11",
    ] {
        assert!(
            matches!(no(Kind::Font, bad), Err(Rejected::NotAFont)),
            "{bad:?}"
        );
    }
    assert!(no(Kind::Font, "Inter Display Medium 10.5").is_ok());
    assert_eq!(s.writes.get(), 0);

    // A locked setting is reported and not written; a missing one is no error.
    s.lock(INTERFACE_SCHEMA, "cursor-theme");
    let plan = no(Kind::Cursor, "Pointers").unwrap();
    assert!(plan.is_empty());
    assert!(matches!(
        plan.targets[0].notes[..],
        [Note::SettingLocked { .. }]
    ));
    let bare = MemSettings::new(&[(BACKGROUND_SCHEMA, "picture-uri", "")]);
    let host = home.host(&StdFs, &bare);
    let plan = look::plan_select(&host, &env, Kind::Wallpaper, Some("blobs-l (gnome)")).unwrap();
    assert_eq!(
        plan.changes().count(),
        1,
        "the desktop without a dark wallpaper key"
    );
}

const WANT: &str = "xdg-config/gtk-3.0:ro;xdg-config/gtk-4.0:ro;";

fn flatpak_plan(host: &Host<'_>, env: &dyn Fn(&str) -> Option<OsString>) -> Plan {
    let themes = bundled::themes();
    let ember = Resolved::new(&themes[1], None).unwrap();
    let only = ApplyOpts {
        only: vec!["flatpak".to_owned()],
        ..ApplyOpts::default()
    };
    let mut plan = engine::plan_apply(host, env, &ember, &only).unwrap();
    plan.targets
        .retain(|t| t.skipped != Some(Skip::NotSelected));
    plan
}

#[test]
fn flatpak_is_off_until_turned_on_and_skipped_where_flatpak_is_not_used() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let env = home.env();
    let themes = bundled::themes();
    let ember = Resolved::new(&themes[1], None).unwrap();
    let state = |plan: &Plan| {
        plan.targets
            .iter()
            .find(|t| t.instance == FLATPAK)
            .unwrap()
            .skipped
            .clone()
    };

    let all = engine::plan_apply(&host, &env, &ember, &ApplyOpts::default()).unwrap();
    assert_eq!(state(&all), Some(Skip::NotSelected));
    assert!(
        engine::excluded(&host)
            .unwrap()
            .contains(&FLATPAK.to_owned())
    );
    engine::set_included(&host, FLATPAK, true).unwrap();
    assert!(
        !engine::excluded(&host)
            .unwrap()
            .contains(&FLATPAK.to_owned())
    );
    let all = engine::plan_apply(&host, &env, &ember, &ApplyOpts::default()).unwrap();
    // This test must not depend on the machine it runs on.
    if !Path::new("/var/lib/flatpak").exists() {
        assert_eq!(state(&all), Some(Skip::NotInstalled));
    }
    fs::create_dir_all(home.dirs.data.join("flatpak")).unwrap();
    let all = engine::plan_apply(&host, &env, &ember, &ApplyOpts::default()).unwrap();
    assert_eq!(state(&all), None);
    engine::set_included(&host, FLATPAK, false).unwrap();
    let all = engine::plan_apply(&host, &env, &ember, &ApplyOpts::default()).unwrap();
    assert_eq!(state(&all), Some(Skip::NotSelected));
}

#[test]
fn flatpak_apply_and_undo_give_back_every_kind_of_file_exactly() {
    let files: [Option<&str>; 6] = [
        None,
        Some(""),
        Some("[Context]\nfilesystems=home;\n"),
        Some("[Context]\nfilesystems=xdg-config/gtk-4.0:ro;\n\n[Environment]\nA=1"),
        Some("[Environment]\r\nA=1\r\n"),
        Some("[Context]\nsockets=x11;"),
    ];
    for (i, file) in files.into_iter().enumerate() {
        let home = Home::new();
        let s = settings();
        let host = home.host(&StdFs, &s);
        let env = home.env();
        // The first case has no Flatpak folder at all in the home folder.
        if i > 0 {
            fs::create_dir_all(home.dirs.data.join("flatpak")).unwrap();
        } else {
            fs::create_dir_all(home.system.join("bin")).unwrap();
        }
        if let Some(text) = file {
            home.write(&home.overrides(), text);
        }
        let before = home.user_files();
        let path_env = |key: &str| match key {
            "PATH" => Some(OsString::from(home.system.join("bin"))),
            other => home.env()(other),
        };
        home.write(&home.system.join("bin/flatpak"), "");
        let plan = flatpak_plan(&host, &path_env);
        let target = &plan.targets[0];
        assert_eq!(target.skipped, None, "{file:?}");
        // Agreement is for a file that was already there.
        assert_eq!(target.needs_consent, file.is_some(), "{file:?}");
        run(&host, &plan);
        let now = fs::read_to_string(home.overrides()).unwrap();
        assert!(now.contains("xdg-config/gtk-3.0:ro"), "{now:?}");
        assert_eq!(now.matches("gtk-4.0").count(), 1, "{now:?}");
        let status = engine::status(&host).unwrap();
        assert!(status.blocks.iter().all(|(_, h)| *h == Health::Ok));
        assert!(
            flatpak_plan(&host, &env).is_empty(),
            "twice changes nothing"
        );

        engine::undo_last(&host).unwrap();
        assert_eq!(home.user_files(), before, "{file:?}");
        // And the same through removing everything.
        run(&host, &flatpak_plan(&host, &path_env));
        run(&host, &engine::plan_restore_original(&host).unwrap());
        assert_eq!(home.user_files(), before, "{file:?}");
        let _ = env;
    }
}

#[test]
fn flatpak_entries_that_were_there_and_later_edits_are_the_users() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s);
    let env = home.env();

    // Both already granted, as on a machine themed by hand: nothing to do,
    // nothing claimed, and removing everything leaves the file alone.
    let granted = format!("[Context]\nfilesystems={WANT}\n");
    home.write(&home.overrides(), &granted);
    assert!(flatpak_plan(&host, &env).is_empty());
    // Denied by the user: also theirs.
    home.write(
        &home.overrides(),
        "[Context]\nfilesystems=!xdg-config/gtk-3.0;xdg-config/gtk-4.0;\n",
    );
    assert!(flatpak_plan(&host, &env).is_empty());

    home.write(&home.overrides(), "[Context]\nfilesystems=home;\n");
    run(&host, &flatpak_plan(&host, &env));
    // Flatseal, or the user, rewrites the file around what was added.
    let edited = format!(
        "[Context]\nfilesystems=home;xdg-music;{WANT}\nsockets=x11;\n\n[Environment]\nA=1\n"
    );
    home.write(&home.overrides(), &edited);
    engine::undo_last(&host).unwrap();
    assert_eq!(
        fs::read_to_string(home.overrides()).unwrap(),
        "[Context]\nfilesystems=home;xdg-music;\nsockets=x11;\n\n[Environment]\nA=1\n"
    );

    // Taken out by hand after an apply: reported as changed, and neither
    // put back behind the user's back nor a problem for removing the rest.
    run(&host, &flatpak_plan(&host, &env));
    home.write(&home.overrides(), "[Context]\nfilesystems=home;\n");
    let status = engine::status(&host).unwrap();
    assert_eq!(status.blocks[0].1, Health::Changed);
    let plan = flatpak_plan(&host, &env);
    assert!(plan.is_empty());
    assert!(matches!(plan.targets[0].notes[..], [Note::KeptFile { .. }]));
    run(&host, &engine::plan_restore_original(&host).unwrap());
    assert_eq!(
        fs::read_to_string(home.overrides()).unwrap(),
        "[Context]\nfilesystems=home;\n"
    );

    // Not text, a folder in the way, a hard link: left alone with a reason.
    fs::write(home.overrides(), b"\xff\xfe").unwrap();
    assert!(matches!(
        flatpak_plan(&host, &env).targets[0].skipped,
        Some(Skip::Unreadable(_))
    ));
    fs::write(home.overrides(), "[Context]\n").unwrap();
    fs::hard_link(home.overrides(), home.root.join("other-name")).unwrap();
    assert!(matches!(
        flatpak_plan(&host, &env).targets[0].skipped,
        Some(Skip::HardLinked(_))
    ));
    fs::remove_file(home.overrides()).unwrap();
    fs::create_dir(home.overrides()).unwrap();
    assert!(matches!(
        flatpak_plan(&host, &env).targets[0].skipped,
        Some(Skip::NotAFile(_))
    ));
}

/// Stop the Flatpak edit and a choice of icons at every step, recover, and
/// compare with the state before.
#[test]
fn exhaustive_crash_test_for_flatpak_and_a_choice() {
    for which in ["flatpak", "icons"] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::new();
            install(&home);
            let s = settings();
            let clean = home.host(&StdFs, &s);
            let env = home.env();
            home.write(&home.overrides(), "[Context]\nfilesystems=home;\n");
            let before = (home.user_files(), s.snapshot());
            let plan = if which == "flatpak" {
                flatpak_plan(&clean, &env)
            } else {
                look::plan_select(&clean, &env, Kind::Icons, Some("Tela-purple")).unwrap()
            };
            let faults = Faults::new(&StdFs, &s, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &plan, &[FLATPAK.to_owned()]).is_ok() {
                assert_ne!((home.user_files(), s.snapshot()), before);
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(
                    r.is_clean() && r.kept.is_empty(),
                    "{which} step {fail_at}: {r:?}"
                );
            }
            assert_eq!(
                (home.user_files(), s.snapshot()),
                before,
                "{which} step {fail_at}"
            );
        }
        assert!(
            crash_points >= 3,
            "{which}: only {crash_points} crash points"
        );
    }
}
