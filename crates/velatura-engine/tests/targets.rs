//! Transaction tests for the targets added in phase 1b, and for all five
//! together. Every one runs in a throw-away home folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Note, Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::targets::{
    self, FLATPAK, GHOSTTY, GTK3, GTK4, INSTANCES, INTERFACE_SCHEMA, KITTY, OtherTool, SHELL,
};
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const GTK_CSS: &str = "@define-color window_bg_color #101010;\nwindow { border-radius: 0; }\n";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    /// A home where every app is present: Kitty, Ghostty, GTK settings
    /// files, and the adw-gtk3 theme in the user's themes folder.
    fn full() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-tgt-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        let h = Self { root, dirs };
        for dir in [".local/state", ".local/share/themes/adw-gtk3-dark/gtk-3.0"] {
            fs::create_dir_all(h.dirs.home.join(dir)).unwrap();
        }
        fs::create_dir_all(h.dirs.home.join(".local/share/themes/adw-gtk3/gtk-3.0")).unwrap();
        for variant in ["adw-gtk3", "adw-gtk3-dark"] {
            h.write(
                &format!(".local/share/themes/{variant}/gtk-3.0/gtk.css"),
                "",
            );
        }
        // User Themes as a distribution package installs it: no schema of
        // its own. Here it lives in the user's folder to stay in the sandbox.
        h.write(
            &format!(
                ".local/share/gnome-shell/extensions/{}/metadata.json",
                targets::USER_THEMES
            ),
            "{}",
        );
        h.write(".config/kitty/kitty.conf", "font_size 12\n");
        h.write(
            ".config/ghostty/config",
            "theme = aetherius\nfont-size = 12",
        );
        h.write(".config/gtk-4.0/gtk.css", GTK_CSS);
        h.write(".config/gtk-3.0/gtk.css", GTK_CSS);
        h
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dirs.home.join(rel)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    /// Every file with its content, leaving out stored backup content, the
    /// lock and the cache tag, which a rolled-back run may leave behind.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if path.is_dir() {
                    if !rel.ends_with("backups/blobs") {
                        if !rel.contains("velatura") {
                            out.insert(format!("{rel}/"), Vec::new());
                        }
                        walk(&path, base, out);
                    }
                } else if !rel.ends_with("velatura/lock") && !rel.ends_with("CACHEDIR.TAG") {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.dirs.home, &self.dirs.home, &mut out);
        out
    }

    fn user_files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut s = self.snapshot();
        s.retain(|k, _| !k.contains("/velatura/"));
        s
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// An environment with nothing in it, and with the system's shared data
/// out of sight: what is installed on the machine running the tests (an
/// extension, a GTK theme) must not change their outcome.
fn no_env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| OsString::from("/nonexistent-velatura-test"))
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &no_env, &resolved, &ApplyOpts::default()).unwrap()
}

fn everyone() -> Vec<String> {
    INSTANCES.iter().map(|s| (*s).to_owned()).collect()
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan(host, theme), &everyone()).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

#[test]
fn all_five_apply_and_undo_to_the_exact_start() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();

    let p = plan(&host, &t[1]);
    assert_eq!(p.needing_consent(), [KITTY, GHOSTTY, GTK4, GTK3]);
    let report = engine::execute(&host, &p, &everyone()).unwrap();
    // Firefox, Flatpak Apps, Recolored Icons and Recolored Cursor are off until turned on, and the
    // command-line apps are
    // not in this home (their tests are in cli_apps.rs); nothing else
    // is left out.
    let left_out: Vec<_> = report
        .skipped
        .iter()
        .filter(|(_, why)| *why != Skip::NotInstalled)
        .collect();
    assert_eq!(
        left_out,
        [
            &(targets::FIREFOX.to_owned(), Skip::NotSelected),
            &(FLATPAK.to_owned(), Skip::NotSelected),
            &(targets::TOP_BAR.to_owned(), Skip::NotSelected),
            &(targets::UPS.to_owned(), Skip::NotSelected),
            &(targets::OUTLINE.to_owned(), Skip::NotSelected),
            &(targets::ICONS.to_owned(), Skip::NotSelected),
            &(targets::CURSORS.to_owned(), Skip::NotSelected)
        ]
    );

    assert!(
        home.read(".config/ghostty/velatura.conf")
            .contains("background = #191316")
    );
    assert!(home.read(".config/ghostty/config").ends_with(
        "font-size = 12\n# >>> velatura ghostty 1 >>>\nconfig-file = velatura.conf\n# <<< velatura ghostty 1 <<<\n"
    ));
    for gtk in ["gtk-4.0", "gtk-3.0"] {
        let n = &gtk[4..5];
        assert!(
            home.read(&format!(".config/{gtk}/velatura.css"))
                .contains("@define-color window_bg_color #191316;")
        );
        assert_eq!(
            home.read(&format!(".config/{gtk}/gtk.css")),
            format!(
                "{GTK_CSS}/* >>> velatura gtk{n} 1 >>> */\n@import url(\"velatura.css\");\n/* <<< velatura gtk{n} 1 <<< */\n"
            )
        );
    }
    let theme_now = settings
        .read(INTERFACE_SCHEMA, "gtk-theme")
        .unwrap()
        .unwrap();
    assert_eq!(theme_now.effective, "adw-gtk3-dark");
    assert!(
        report
            .notes
            .iter()
            .any(|(i, n)| i == GTK4 && *n == Note::GtkRestart)
    );
    assert!(
        report
            .notes
            .iter()
            .any(|(i, n)| i == GHOSTTY && *n == Note::GhosttyReload)
    );

    // A second theme only rewrites the four colour files and the accent.
    let second = apply(&host, &t[0]);
    // Plus the Shell: a new theme folder in, the old one out, the name set.
    assert_eq!(second.changes.len(), 5 + 7, "{:?}", second.changes);
    assert!(plan(&host, &t[0]).is_empty());

    undo(&host);
    undo(&host);
    assert_eq!(home.user_files(), start);
    assert!(settings.snapshot().is_empty());
}

#[test]
fn exhaustive_crash_test_over_all_targets() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::full();
            let settings = MemSettings::gnome();
            let clean = home.host(&StdFs, &settings);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
            }
            let before = (home.snapshot(), settings.snapshot());
            let p = plan(&clean, &t[1]);

            let faults = Faults::new(&StdFs, &settings, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &p, &everyone()).is_ok() {
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                (home.snapshot(), settings.snapshot()),
                before,
                "state differs after a crash at step {fail_at} (already applied: {start_applied})"
            );
        }
        assert!(
            crash_points >= 9,
            "only {crash_points} crash points exercised"
        );
    }
}

#[test]
fn ghostty_uses_whichever_settings_file_exists() {
    let t = bundled::themes();
    let home = Home::full();
    home.write(".config/ghostty/config.ghostty", "font-size = 11\n");
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    assert!(
        home.read(".config/ghostty/config.ghostty")
            .contains("velatura ghostty")
    );
    assert!(!home.read(".config/ghostty/config").contains("velatura"));

    // A later config-file line is the one thing that can override.
    let mut conf = home.read(".config/ghostty/config.ghostty");
    conf.push_str("background = #000000\nconfig-file = mine.conf\n");
    home.write(".config/ghostty/config.ghostty", &conf);
    let p = plan(&host, &t[1]);
    let lines = target(&p, GHOSTTY).notes.iter().find_map(|n| match n {
        Note::Override { lines, .. } => Some(lines.clone()),
        _ => None,
    });
    assert_eq!(lines, Some(vec![6]));
}

#[test]
fn gtk_reports_definitions_after_the_block_and_is_skipped_in_high_contrast() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);

    let mut css = home.read(".config/gtk-4.0/gtk.css");
    css.push_str("@define-color my_own_color #fff;\n@define-color accent_bg_color #f00;\n");
    home.write(".config/gtk-4.0/gtk.css", &css);
    let p = plan(&host, &t[1]);
    let lines = target(&p, GTK4).notes.iter().find_map(|n| match n {
        Note::Override { lines, .. } => Some(lines.clone()),
        _ => None,
    });
    assert_eq!(
        lines,
        Some(vec![7]),
        "only the colour the theme also defines"
    );

    let hc = MemSettings::new(&[
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "true"),
    ]);
    let p = plan(&home.host(&StdFs, &hc), &t[1]);
    assert_eq!(target(&p, GTK4).skipped, Some(Skip::HighContrast));
    assert_eq!(target(&p, GTK3).skipped, Some(Skip::HighContrast));
    assert!(
        target(&p, KITTY).skipped.is_none(),
        "terminals are not affected"
    );
}

#[test]
fn gtk3_needs_adw_gtk3_and_finds_it_in_any_theme_folder() {
    let t = bundled::themes();
    let home = Home::full();
    fs::remove_dir_all(home.path(".local/share/themes")).unwrap();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let before = home.read(".config/gtk-3.0/gtk.css");

    let report = apply(&host, &t[0]);
    assert!(
        report
            .skipped
            .contains(&(GTK3.to_owned(), Skip::NeedsAdwGtk3))
    );
    assert_eq!(home.read(".config/gtk-3.0/gtk.css"), before);
    assert!(
        settings
            .read(INTERFACE_SCHEMA, "gtk-theme")
            .unwrap()
            .unwrap()
            .user
            .is_none()
    );

    // Installed system-wide, found through the data folders.
    let system = home.root.join("usr-share");
    fs::create_dir_all(system.join("themes/adw-gtk3-dark/gtk-3.0")).unwrap();
    fs::write(system.join("themes/adw-gtk3-dark/gtk-3.0/gtk.css"), "").unwrap();
    let env = |k: &str| (k == "XDG_DATA_DIRS").then(|| OsString::from(&system));
    let resolved = Resolved::new(&t[0], None).unwrap();
    let p = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    assert!(target(&p, GTK3).skipped.is_none());

    // A light theme needs the light variant, which is not there.
    let light = bundled::SOURCES[0]
        .1
        .replace("appearance = \"dark\"", "appearance = \"light\"");
    let light = velatura_theme::parse_theme(&light).unwrap().theme;
    let resolved = Resolved::new(&light, None).unwrap();
    let p = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    assert_eq!(target(&p, GTK3).skipped, Some(Skip::NeedsAdwGtk3));
}

#[test]
fn a_light_theme_reports_the_legacy_dark_preference() {
    let home = Home::full();
    home.write(
        ".config/gtk-3.0/settings.ini",
        "[Settings]\ngtk-application-prefer-dark-theme = 1\n",
    );
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let light = bundled::SOURCES[0]
        .1
        .replace("appearance = \"dark\"", "appearance = \"light\"");
    let light = velatura_theme::parse_theme(&light).unwrap().theme;
    let p = plan(&host, &light);
    assert!(
        target(&p, GTK3)
            .notes
            .iter()
            .any(|n| matches!(n, Note::Gtk3DarkForced { .. }))
    );
    assert!(!plan(&host, &bundled::themes()[0]).targets.iter().any(|t| {
        t.notes
            .iter()
            .any(|n| matches!(n, Note::Gtk3DarkForced { .. }))
    }));
}

#[test]
fn a_hard_linked_file_is_left_alone_and_a_link_is_reported() {
    let t = bundled::themes();
    let home = Home::full();
    fs::hard_link(
        home.path(".config/gtk-4.0/gtk.css"),
        home.path("other-name.css"),
    )
    .unwrap();
    // Ghostty's file is managed stow-style: a link into a dotfiles folder.
    home.write("dotfiles/ghostty-config", "font-size = 12\n");
    fs::remove_file(home.path(".config/ghostty/config")).unwrap();
    std::os::unix::fs::symlink(
        home.path("dotfiles/ghostty-config"),
        home.path(".config/ghostty/config"),
    )
    .unwrap();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);

    let p = plan(&host, &t[0]);
    assert!(matches!(
        target(&p, GTK4).skipped,
        Some(Skip::HardLinked(_))
    ));
    let real = target(&p, GHOSTTY).notes.iter().find_map(|n| match n {
        Note::LinkedFile { real, .. } => Some(real.clone()),
        _ => None,
    });
    assert!(real.unwrap().ends_with("dotfiles/ghostty-config"));

    engine::execute(&host, &p, &everyone()).unwrap();
    assert_eq!(
        home.read("other-name.css"),
        GTK_CSS,
        "both names still one file"
    );
    assert_eq!(StdFs.links(&home.path("other-name.css")).unwrap(), 2);
    assert!(
        home.read("dotfiles/ghostty-config")
            .contains("velatura ghostty")
    );
}

#[test]
fn other_theming_tools_are_found_by_what_they_leave_behind() {
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    assert!(targets::other_tools(&host).is_empty());

    home.write(
        ".config/systemd/user/timers.target.wants/theme-auto.timer",
        "",
    );
    home.write(".config/systemd/user/timers.target.wants/backup.timer", "");
    home.write(
        ".config/systemd/user/timers.target.wants/velatura-switch.timer",
        "",
    );
    home.write(".cache/wal/colors", "");
    home.write(".local/share/chezmoi/dot_bashrc", "");
    let found = targets::other_tools(&host);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(matches!(&found[0], OtherTool::Known { name: "pywal", .. }));
    assert!(matches!(&found[1], OtherTool::Timer { unit } if unit == "theme-auto.timer"));
    assert!(matches!(
        &found[2],
        OtherTool::Dotfiles {
            name: "chezmoi",
            ..
        }
    ));
}

#[test]
fn day_and_night_choices_are_kept_in_a_file_a_person_can_edit() {
    use velatura_engine::store::{DayNight, is_theme_spec};
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    assert_eq!(DayNight::load(&host), DayNight::default());

    home.write(".config/velatura/config.toml", "# mine\nretention = 20\n");
    let chosen = DayNight {
        day: Some("aetherius".to_owned()),
        night: Some("ember/default".to_owned()),
    };
    chosen.save(&host).unwrap();
    assert_eq!(
        home.read(".config/velatura/config.toml"),
        "day = \"aetherius\"\nnight = \"ember/default\"\n# mine\nretention = 20\n"
    );
    assert_eq!(DayNight::load(&host), chosen);
    DayNight {
        day: None,
        ..chosen.clone()
    }
    .save(&host)
    .unwrap();
    assert_eq!(DayNight::load(&host).day, None);

    // Anything that is not a plain theme name is ignored, not trusted.
    home.write(
        ".config/velatura/config.toml",
        "day = \"../../etc\"\nnight = 7\n",
    );
    assert_eq!(DayNight::load(&host), DayNight::default());
    for bad in ["", "a/b/c", "A", "a b", "a/", "/a", "a\nb"] {
        assert!(!is_theme_spec(bad), "{bad:?}");
    }
    assert!(is_theme_spec("base16-ocean/dark"));
}

const SHELL_SCHEMA: &str = "org.gnome.shell.extensions.user-theme";

#[test]
fn the_shell_theme_gets_a_new_folder_per_theme_and_the_old_one_is_cleared() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();
    let name = |s: &MemSettings| s.read(SHELL_SCHEMA, "name").unwrap().unwrap().effective;
    let folders = |home: &Home| -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(home.path(".local/share/themes"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("Velatura-"))
            .collect();
        v.sort();
        v
    };

    apply(&host, &t[0]);
    let first = name(&settings);
    assert!(first.starts_with("Velatura-"));
    assert_eq!(folders(&home), std::slice::from_ref(&first));
    let css = home.read(&format!(
        ".local/share/themes/{first}/gnome-shell/gnome-shell.css"
    ));
    assert!(css.contains("#panel { background-color: #0d101a; color: #d3d9ee; }"));

    // A new theme means a new name, or the Shell would not reload.
    apply(&host, &t[1]);
    let second = name(&settings);
    assert_ne!(first, second);
    assert_eq!(
        folders(&home),
        std::slice::from_ref(&second),
        "the earlier folder is gone"
    );
    assert!(plan(&host, &t[1]).is_empty());

    undo(&host);
    assert_eq!(name(&settings), first);
    assert_eq!(folders(&home), [first], "and comes back with undo");
    undo(&host);
    assert_eq!(home.user_files(), start);
    assert!(settings.snapshot().is_empty());
}

#[test]
fn the_shell_needs_user_themes_and_reports_what_hides_the_theme() {
    let t = bundled::themes();
    let home = Home::full();
    let ext = format!(
        ".local/share/gnome-shell/extensions/{}",
        targets::USER_THEMES
    );

    // Installed for the user, the extension's own schema folder is used.
    home.write(&format!("{ext}/schemas/gschemas.compiled"), "");
    let own = format!(
        "{SHELL_SCHEMA}@{}",
        home.path(&format!("{ext}/schemas")).display()
    );
    // One that colours the top bar itself; one that colours only its own
    // widgets, and the panel only under a class of its own (an option).
    home.write(
        ".local/share/gnome-shell/extensions/styled@test/stylesheet.css",
        "/* panel */\n#panel.styled-panel .panel-button { color: #d3d9ee; }\n",
    );
    home.write(
        ".local/share/gnome-shell/extensions/widgets@test/stylesheet.css",
        ".widgets-label { color: red; }\n.widgets-option #panel { background-color: transparent; }\n#panelish { color: red; }\n",
    );
    let settings = MemSettings::new(&[
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        (&own, "name", "Dracula"),
        (
            "org.gnome.shell",
            "enabled-extensions",
            "['styled@test', 'widgets@test', 'plain@test']",
        ),
    ]);
    let host = home.host(&StdFs, &settings);
    let p = plan(&host, &t[0]);
    let shell = target(&p, SHELL);
    assert!(shell.skipped.is_none());
    assert!(shell.notes.contains(&Note::UserThemesOff));
    assert!(shell.notes.contains(&Note::ShellExtensionStyles {
        names: vec!["styled@test".to_owned()]
    }));
    engine::execute(&host, &p, &everyone()).unwrap();
    assert!(
        settings
            .read(&own, "name")
            .unwrap()
            .unwrap()
            .effective
            .starts_with("Velatura-")
    );
    undo(&host);
    assert!(
        settings.snapshot().is_empty(),
        "the leftover default name is back"
    );

    // Without the extension the Shell is left out; with High Contrast too.
    fs::remove_dir_all(home.path(&ext)).unwrap();
    let p = plan(&home.host(&StdFs, &MemSettings::gnome()), &t[0]);
    assert_eq!(target(&p, SHELL).skipped, Some(Skip::NeedsUserThemes));
}

#[test]
fn a_target_turned_off_is_left_out_until_named_or_turned_on() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    engine::set_included(&host, GHOSTTY, false).unwrap();
    assert_eq!(
        engine::excluded(&host).unwrap(),
        [
            GHOSTTY,
            targets::FIREFOX,
            FLATPAK,
            targets::TOP_BAR,
            targets::UPS,
            targets::OUTLINE,
            targets::ICONS,
            targets::CURSORS
        ]
    );
    let p = plan(&host, &t[0]);
    assert_eq!(target(&p, GHOSTTY).skipped, Some(Skip::NotSelected));
    assert!(target(&p, KITTY).skipped.is_none());

    let resolved = Resolved::new(&t[0], None).unwrap();
    let named = ApplyOpts {
        only: vec!["ghostty".to_owned()],
        ..ApplyOpts::default()
    };
    let p = engine::plan_apply(&host, &no_env, &resolved, &named).unwrap();
    assert!(target(&p, GHOSTTY).skipped.is_none(), "naming it overrides");

    engine::set_included(&host, GHOSTTY, true).unwrap();
    assert!(target(&plan(&host, &t[0]), GHOSTTY).skipped.is_none());
    engine::save_window_size(&host, 900, 640).unwrap();
    assert_eq!(engine::window_size(&host), Some((900, 640)));
    assert_eq!(
        engine::excluded(&host).unwrap(),
        [
            targets::FIREFOX,
            FLATPAK,
            targets::TOP_BAR,
            targets::UPS,
            targets::OUTLINE,
            targets::ICONS,
            targets::CURSORS
        ]
    );
}

#[test]
fn restore_to_goes_back_to_before_the_named_backup() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();
    apply(&host, &t[0]);
    let after_first = home.user_files();
    apply(&host, &t[1]);
    apply(&host, &t[0]);
    assert_eq!(engine::restore_to(&host, 2).unwrap().len(), 2);
    assert_eq!(home.user_files(), after_first);
    assert!(engine::restore_to(&host, 2).is_err(), "already undone");
    assert_eq!(engine::restore_to(&host, 1).unwrap().len(), 1);
    assert_eq!(home.user_files(), start);
}

/// Backups written by the first released format must stay restorable by
/// every later build. The fixture was written by phase 1b and is not
/// regenerated: if this test fails, the format changed incompatibly.
#[test]
fn a_first_version_backup_can_still_be_undone() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store-v1");
    if std::env::var_os("VELATURA_WRITE_FIXTURE").is_some() {
        write_fixture(&fixture);
    }
    let home = Home::full();
    let home_str = home.dirs.home.to_string_lossy().into_owned();
    fn copy(from: &Path, to: &Path, home: &str) {
        for entry in fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let dest = to.join(path.file_name().unwrap());
            if path.is_dir() {
                fs::create_dir_all(&dest).unwrap();
                copy(&path, &dest, home);
            } else {
                let text = fs::read(&path).unwrap();
                let text = String::from_utf8_lossy(&text).replace("@HOME@", home);
                fs::write(&dest, text).unwrap();
            }
        }
    }
    for rel in [
        ".config/kitty",
        ".local/share/velatura",
        ".local/state/velatura",
    ] {
        let _ = fs::remove_dir_all(home.path(rel));
        fs::create_dir_all(home.path(rel)).unwrap();
        copy(&fixture.join(rel), &home.path(rel), &home_str);
    }
    let settings = MemSettings::gnome();
    settings
        .write(INTERFACE_SCHEMA, "color-scheme", Some("prefer-dark"))
        .unwrap();
    settings
        .write(INTERFACE_SCHEMA, "accent-color", Some("orange"))
        .unwrap();
    let host = home.host(&StdFs, &settings);

    let status = engine::status(&host).unwrap();
    assert_eq!(status.applied.unwrap().0, "ember/default");
    let (history, damaged) = engine::history(&host).unwrap();
    assert_eq!((history.len(), damaged.len()), (1, 0));

    undo(&host);
    assert_eq!(home.read(".config/kitty/kitty.conf"), "font_size 12");
    assert!(!home.path(".config/kitty/velatura.conf").exists());
    assert!(settings.snapshot().is_empty());
}

/// Produce the fixture for the test above from a real apply.
fn write_fixture(fixture: &Path) {
    let home = Home::full();
    home.write(".config/kitty/kitty.conf", "font_size 12");
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let resolved_theme = bundled::themes();
    let resolved = Resolved::new(&resolved_theme[1], None).unwrap();
    let only = ApplyOpts {
        only: vec!["kitty".to_owned(), "gnome:appearance".to_owned()],
        ..ApplyOpts::default()
    };
    let p = engine::plan_apply(&host, &no_env, &resolved, &only).unwrap();
    engine::execute(&host, &p, &everyone()).unwrap();
    let home_str = home.dirs.home.to_string_lossy().into_owned();
    let _ = fs::remove_dir_all(fixture);
    for (rel, bytes) in home.snapshot() {
        let keep = [
            ".config/kitty/",
            ".local/share/velatura/",
            ".local/state/velatura/",
        ]
        .iter()
        .any(|p| rel.starts_with(p));
        if !keep || rel.ends_with('/') {
            continue;
        }
        let dest = fixture.join(&rel);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(
            dest,
            String::from_utf8_lossy(&bytes).replace(&home_str, "@HOME@"),
        )
        .unwrap();
    }
    let blobs = ".local/share/velatura/backups/blobs";
    fs::create_dir_all(fixture.join(blobs)).unwrap();
    for entry in fs::read_dir(home.path(blobs)).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, fixture.join(blobs).join(path.file_name().unwrap())).unwrap();
    }
}
