//! Transaction tests for the targets added in phase 3c: Ptyxis and GNOME
//! Text Editor. Every one runs in a throw-away home folder, with settings
//! held in memory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Note, Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::targets::{self, PTYXIS, TEXT_EDITOR};
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const PROFILE: &str = "65c6fbc3c355f4d3b8b70a1f68b3104a";
const PALETTE_FILE: &str = ".local/share/org.gnome.Ptyxis/palettes/velatura.palette";
const SCHEME_FILE: &str = ".local/share/gtksourceview-5/styles/velatura.xml";

fn profile(uuid: &str) -> String {
    targets::ptyxis_profile(uuid).unwrap()
}

/// The settings of a desktop where both apps are installed, Ptyxis with
/// the profiles given, the first of them the default.
fn settings(profiles: &[&str]) -> MemSettings {
    let listed = format!(
        "[{}]",
        profiles
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let paths: Vec<String> = profiles.iter().map(|p| profile(p)).collect();
    let mut keys = vec![
        (
            "org.gnome.Ptyxis",
            "default-profile-uuid",
            profiles.first().copied().unwrap_or(""),
        ),
        ("org.gnome.Ptyxis", "profile-uuids", listed.as_str()),
        ("org.gnome.TextEditor", "style-scheme", "Adwaita"),
        ("org.gnome.desktop.interface", "color-scheme", "default"),
        ("org.gnome.desktop.interface", "accent-color", "blue"),
    ];
    for path in &paths {
        keys.push((path.as_str(), "palette", "gnome"));
    }
    MemSettings::new(&keys)
}

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-gnome-apps-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        // Ptyxis makes its folder for palettes when it first starts.
        for dir in [
            ".local/state",
            ".local/share/org.gnome.Ptyxis/palettes",
            ".config",
        ] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        Self { root, dirs }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dirs.home.join(rel)
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

    /// Every file and folder outside Velatura's own store.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if rel.contains("/velatura") && !rel.contains("velatura.") {
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

fn no_env(_: &str) -> Option<OsString> {
    None
}

const APPS: [&str; 2] = [PTYXIS, TEXT_EDITOR];

fn opts() -> ApplyOpts {
    ApplyOpts {
        only: APPS.iter().map(|s| (*s).to_owned()).collect(),
        ..ApplyOpts::default()
    }
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &no_env, &resolved, &opts()).unwrap()
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan(host, theme), &opts().only).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

#[test]
fn both_apply_twice_and_undo_to_the_exact_start() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings(&[PROFILE]);
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();

    let p = plan(&host, &t[1]);
    // Neither edits a file of the user's, but the first change to an app
    // is agreed to: with no one there, as in a scheduled switch, nothing.
    assert_eq!(p.needing_consent(), APPS);
    let alone = engine::execute(&host, &p, &[]).unwrap();
    assert!(alone.changes.is_empty(), "{:?}", alone.changes);
    assert_eq!(home.snapshot(), start);
    assert!(s.snapshot().is_empty());

    let report = engine::execute(&host, &p, &opts().only).unwrap();
    let palette = home.read(PALETTE_FILE);
    assert!(palette.contains("[Palette]\nName=Velatura\n"), "{palette}");
    // Both sets are the theme's, so it shows under either style.
    assert_eq!(palette.matches("Background=#191316\n").count(), 2);
    assert_eq!(palette.matches("Color15=#dbc7b0\n").count(), 2);
    assert!(
        home.read(SCHEME_FILE)
            .contains("<style name=\"text\" foreground=\"#dbc7b0\" background=\"#191316\"/>")
    );
    let set = |schema: &str, key: &str| s.read(schema, key).unwrap().unwrap().effective;
    assert_eq!(set(&profile(PROFILE), "palette"), "velatura");
    assert_eq!(set("org.gnome.TextEditor", "style-scheme"), "velatura");
    assert!(
        report
            .notes
            .contains(&(PTYXIS.to_owned(), Note::PtyxisReopen))
    );
    assert!(
        report
            .notes
            .contains(&(TEXT_EDITOR.to_owned(), Note::GtkRestart))
    );
    assert!(!report.notes.iter().any(|(_, n)| *n == Note::PtyxisChoose));

    // The same theme again changes nothing and writes no setting.
    let writes = s.writes.get();
    assert!(plan(&host, &t[1]).is_empty());
    apply(&host, &t[1]);
    assert_eq!(s.writes.get(), writes);

    // Another theme rewrites the two files; the settings already point
    // at them, and nothing more is asked.
    let p = plan(&host, &t[0]);
    assert!(p.needing_consent().is_empty());
    let second = engine::execute(&host, &p, &[]).unwrap();
    assert_eq!(second.changes.len(), 2, "{:?}", second.changes);
    assert!(home.read(PALETTE_FILE).contains("Background=#141826\n"));

    undo(&host);
    undo(&host);
    assert_eq!(home.snapshot(), start);
    assert!(
        s.snapshot().is_empty(),
        "both settings are at their defaults again"
    );
}

#[test]
fn exhaustive_crash_test_over_both() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::new();
            let s = settings(&[PROFILE]);
            let clean = home.host(&StdFs, &s);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
            }
            let before = (home.snapshot(), s.snapshot());
            let p = plan(&clean, &t[1]);

            let faults = Faults::new(&StdFs, &s, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &p, &opts().only).is_ok() {
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                (home.snapshot(), s.snapshot()),
                before,
                "state differs after a crash at step {fail_at} (already applied: {start_applied})"
            );
        }
        assert!(
            crash_points >= 3,
            "only {crash_points} crash points exercised"
        );
    }
}

#[test]
fn a_choice_the_user_made_since_is_kept_by_undo() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings(&[PROFILE]);
    let host = home.host(&StdFs, &s);
    // A palette and a style of the user's own choosing, before Velatura.
    s.write(&profile(PROFILE), "palette", Some("dracula"))
        .unwrap();
    s.write("org.gnome.TextEditor", "style-scheme", Some("kate-dark"))
        .unwrap();
    apply(&host, &t[0]);
    undo(&host);
    let set = |schema: &str, key: &str| s.read(schema, key).unwrap().unwrap().user;
    assert_eq!(
        set(&profile(PROFILE), "palette").as_deref(),
        Some("dracula")
    );
    assert_eq!(
        set("org.gnome.TextEditor", "style-scheme").as_deref(),
        Some("kate-dark")
    );

    // Changed again after applying: that choice is the newer one.
    apply(&host, &t[0]);
    s.write(&profile(PROFILE), "palette", Some("nord")).unwrap();
    let report = undo(&host);
    assert_eq!(set(&profile(PROFILE), "palette").as_deref(), Some("nord"));
    assert_eq!(
        set("org.gnome.TextEditor", "style-scheme").as_deref(),
        Some("kate-dark")
    );
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KeptSetting { .. }))
    );
    assert!(!home.path(PALETTE_FILE).exists());
}

#[test]
fn only_the_default_profile_is_set_and_the_rest_is_said() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings(&[PROFILE, "second-profile"]);
    let host = home.host(&StdFs, &s);
    let report = apply(&host, &t[0]);
    let palette = |uuid: &str| {
        s.read(&profile(uuid), "palette")
            .unwrap()
            .unwrap()
            .effective
    };
    assert_eq!(palette(PROFILE), "velatura");
    assert_eq!(palette("second-profile"), "gnome");
    assert!(
        report
            .notes
            .contains(&(PTYXIS.to_owned(), Note::PtyxisChoose))
    );

    // No profile yet, as before Ptyxis is first started, or an id that
    // could not be part of a path: the palette is put there to choose.
    for default in ["", "../../../org/gnome/desktop/interface"] {
        let home = Home::new();
        let s = MemSettings::new(&[
            ("org.gnome.Ptyxis", "default-profile-uuid", default),
            ("org.gnome.Ptyxis", "profile-uuids", "[]"),
        ]);
        let host = home.host(&StdFs, &s);
        let p = plan(&host, &t[0]);
        assert!(target(&p, PTYXIS).skipped.is_none(), "{default:?}");
        assert!(target(&p, PTYXIS).notes.contains(&Note::PtyxisChoose));
        engine::execute(&host, &p, &opts().only).unwrap();
        assert!(home.path(PALETTE_FILE).exists());
        assert!(s.snapshot().is_empty(), "no setting was written");
    }
    assert_eq!(targets::ptyxis_profile("a/b"), None);
}

#[test]
fn apps_that_are_not_there_or_cannot_be_set_are_left_out() {
    let t = bundled::themes();
    let home = Home::new();
    // No settings of theirs on this system: not installed.
    let none = MemSettings::gnome();
    let p = plan(&home.host(&StdFs, &none), &t[0]);
    for app in APPS {
        assert_eq!(target(&p, app).skipped, Some(Skip::NotInstalled), "{app}");
    }
    // Ptyxis's settings without its folder or its program.
    let s = settings(&[PROFILE]);
    fs::remove_dir_all(home.path(".local/share/org.gnome.Ptyxis")).unwrap();
    let p = plan(&home.host(&StdFs, &s), &t[0]);
    assert_eq!(target(&p, PTYXIS).skipped, Some(Skip::NotInstalled));
    assert!(target(&p, TEXT_EDITOR).skipped.is_none());

    // A setting an administrator has locked: the file is written, the
    // setting is left, and that is said.
    let home = Home::new();
    let s = settings(&[PROFILE]);
    s.lock("org.gnome.TextEditor", "style-scheme");
    let host = home.host(&StdFs, &s);
    let report = apply(&host, &t[0]);
    assert!(home.path(SCHEME_FILE).exists());
    assert!(report.notes.contains(&(
        TEXT_EDITOR.to_owned(),
        Note::SettingLocked {
            key: "style-scheme".to_owned()
        }
    )));

    // A file of that name that is not Velatura's is not replaced.
    let home = Home::new();
    fs::write(home.path(PALETTE_FILE), "[Palette]\nName=Mine\n").unwrap();
    let s = settings(&[PROFILE]);
    let p = plan(&home.host(&StdFs, &s), &t[0]);
    assert_eq!(
        target(&p, PTYXIS).skipped,
        Some(Skip::ForeignFile(home.path(PALETTE_FILE)))
    );
}
