//! Transaction tests for Firefox, added in phase 4a. Every one runs in a
//! throw-away home folder holding a Firefox profile folder as Firefox
//! leaves it, with settings held in memory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::firefox::{self, FIREFOX};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Note, Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const ROOT: &str = ".mozilla/firefox";
const PROFILE: &str = ".mozilla/firefox/w6hrzc8i.default-release";
const SHEET: &str = ".mozilla/firefox/w6hrzc8i.default-release/chrome/velatura.css";
const USER_CHROME: &str = ".mozilla/firefox/w6hrzc8i.default-release/chrome/userChrome.css";
const USER_JS: &str = ".mozilla/firefox/w6hrzc8i.default-release/user.js";

/// What Firefox writes on its first start: two profiles, as it makes when
/// an older one was there, and the one it opens named in installs.ini.
const PROFILES_INI: &str = "[Profile1]\nName=default\nIsRelative=1\nPath=uysw3pf6.default\nDefault=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=w6hrzc8i.default-release\n\n[General]\nStartWithLastProfile=1\nVersion=2\n\n[Install11457493C5A56847]\nDefault=w6hrzc8i.default-release\nLocked=1\n";
const INSTALLS_INI: &str = "[11457493C5A56847]\nDefault=w6hrzc8i.default-release\nLocked=1\n";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-firefox-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        for dir in [
            ".local/state",
            ".local/share",
            ".config",
            PROFILE,
            &format!("{ROOT}/uysw3pf6.default"),
        ] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        fs::write(home.join(ROOT).join("profiles.ini"), PROFILES_INI).unwrap();
        fs::write(home.join(ROOT).join("installs.ini"), INSTALLS_INI).unwrap();
        fs::write(
            home.join(PROFILE).join("prefs.js"),
            "user_pref(\"a\", 1);\n",
        )
        .unwrap();
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

fn named() -> ApplyOpts {
    ApplyOpts {
        only: vec![FIREFOX.to_owned()],
        ..ApplyOpts::default()
    }
}

fn plan_with(host: &Host<'_>, theme: &Theme, opts: &ApplyOpts) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &no_env, &resolved, opts).unwrap()
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    plan_with(host, theme, &named())
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan(host, theme), &[FIREFOX.to_owned()]).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

#[test]
fn off_until_turned_on_and_asked_the_first_time() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();

    // Left out of an ordinary apply until turned on.
    let p = plan_with(&host, &t[1], &ApplyOpts::default());
    assert_eq!(target(&p, FIREFOX).skipped, Some(Skip::NotSelected));
    engine::set_included(&host, FIREFOX, true).unwrap();
    let p = plan_with(&host, &t[1], &ApplyOpts::default());
    assert!(target(&p, FIREFOX).skipped.is_none());

    // The profile is the user's: the first change is agreed to, so a run
    // with no one there, as a scheduled switch is, changes nothing.
    let alone = ApplyOpts {
        unattended: true,
        ..named()
    };
    let p = plan_with(&host, &t[1], &alone);
    assert_eq!(p.needing_consent(), [FIREFOX]);
    let report = engine::execute(&host, &p, &[]).unwrap();
    assert!(report.changes.is_empty(), "{:?}", report.changes);
    assert!(
        report
            .skipped
            .contains(&(FIREFOX.to_owned(), Skip::NoConsent))
    );
    assert_eq!(home.snapshot(), start);

    // Agreed once, and not asked again, even with no one there.
    apply(&host, &t[1]);
    let p = plan_with(&host, &t[0], &alone);
    assert!(p.needing_consent().is_empty());
    assert!(!engine::execute(&host, &p, &[]).unwrap().changes.is_empty());
}

#[test]
fn apply_twice_and_undo_to_the_exact_start() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();

    let report = apply(&host, &t[1]);
    let sheet = home.read(SHEET);
    assert!(sheet.contains("  --toolbar-background-color: #191316 !important;\n"));
    assert_eq!(
        home.read(USER_CHROME),
        "/* >>> velatura firefox 1 >>> */\n@import url(\"velatura.css\");\n/* <<< velatura firefox 1 <<< */\n"
    );
    assert_eq!(
        home.read(USER_JS),
        format!(
            "// >>> velatura firefox-prefs 1 >>>\n{}\n// <<< velatura firefox-prefs 1 <<<\n",
            firefox::pref_line()
        )
    );
    // Firefox's own settings file is never touched.
    assert_eq!(
        home.read(&format!("{PROFILE}/prefs.js")),
        "user_pref(\"a\", 1);\n"
    );
    // The other profile is left alone, and that is said.
    assert!(
        !home
            .path(&format!("{ROOT}/uysw3pf6.default/chrome"))
            .exists()
    );
    for note in [Note::FirefoxRestart, Note::FirefoxOtherProfiles] {
        assert!(report.notes.contains(&(FIREFOX.to_owned(), note)));
    }

    // The same theme again changes nothing.
    assert!(plan(&host, &t[1]).is_empty());

    // Another theme rewrites the stylesheet only.
    let second = apply(&host, &t[0]);
    assert_eq!(second.changes.len(), 1, "{:?}", second.changes);
    assert!(home.read(SHEET).contains("#141826"));

    undo(&host);
    let first = undo(&host);
    assert_eq!(home.snapshot(), start);
    // Taking the line out does not turn the preference off in Firefox.
    assert!(
        first
            .notes
            .contains(&(String::new(), Note::FirefoxPrefStays))
    );
}

#[test]
fn files_of_the_users_keep_what_is_theirs() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    fs::create_dir_all(home.path(PROFILE).join("chrome")).unwrap();
    let own_sheet = "@charset \"UTF-8\";\n#nav-bar { border: none !important; }\n";
    let own_prefs = "user_pref(\"browser.tabs.warnOnClose\", false);";
    fs::write(home.path(USER_CHROME), own_sheet).unwrap();
    fs::write(home.path(USER_JS), own_prefs).unwrap();
    let start = home.snapshot();

    apply(&host, &t[0]);
    // The import goes before every rule; the preference at the end.
    assert!(
        home.read(USER_CHROME)
            .starts_with("/* >>> velatura firefox 1 >>> */\n")
    );
    assert!(home.read(USER_CHROME).ends_with(own_sheet));
    assert!(home.read(USER_JS).starts_with(own_prefs));

    // Edits made since, around the blocks, stay through undo.
    let mut sheet = home.read(USER_CHROME);
    sheet.push_str("#urlbar { font-size: 14px; }\n");
    fs::write(home.path(USER_CHROME), &sheet).unwrap();
    let mut prefs = home.read(USER_JS);
    prefs.insert_str(0, "user_pref(\"b\", 2);\n");
    fs::write(home.path(USER_JS), &prefs).unwrap();
    undo(&host);
    assert_eq!(
        home.read(USER_CHROME),
        format!("{own_sheet}#urlbar {{ font-size: 14px; }}\n")
    );
    assert_eq!(
        home.read(USER_JS),
        format!("user_pref(\"b\", 2);\n{own_prefs}")
    );
    assert!(!home.path(SHEET).exists());

    // Restore Original Setup does the same from an applied state.
    fs::write(home.path(USER_CHROME), own_sheet).unwrap();
    fs::write(home.path(USER_JS), own_prefs).unwrap();
    apply(&host, &t[1]);
    let p = engine::plan_restore_original(&host).unwrap();
    let report = engine::execute(&host, &p, &[]).unwrap();
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| *n == Note::FirefoxPrefStays)
    );
    assert_eq!(home.snapshot(), start);
}

#[test]
fn rules_and_lines_that_win_over_the_theme_are_said() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    fs::create_dir_all(home.path(PROFILE).join("chrome")).unwrap();
    fs::write(
        home.path(USER_CHROME),
        ":root {\n  --toolbar-background-color: red !important;\n  --toolbar-background-color-x: red;\n}\n",
    )
    .unwrap();
    fs::write(
        home.path(USER_JS),
        format!("user_pref(\"{}\", false);\n", firefox::PREF),
    )
    .unwrap();
    // Before the first apply the user's lines come before where the
    // block goes in user.js, and after it in userChrome.css.
    let p = plan(&host, &t[0]);
    let notes = &target(&p, FIREFOX).notes;
    assert!(notes.contains(&Note::Override {
        path: home.path(USER_CHROME),
        lines: vec![5],
    }));
    assert!(
        !notes
            .iter()
            .any(|n| matches!(n, Note::Override { path, .. } if *path == home.path(USER_JS)))
    );

    // A line after the block that turns the preference off again.
    apply(&host, &t[0]);
    let mut prefs = home.read(USER_JS);
    prefs.push_str(&format!("user_pref(\"{}\", false);\n", firefox::PREF));
    fs::write(home.path(USER_JS), prefs).unwrap();
    let p = plan(&host, &t[0]);
    assert!(target(&p, FIREFOX).notes.contains(&Note::Override {
        path: home.path(USER_JS),
        lines: vec![5],
    }));
}

#[test]
fn exhaustive_crash_test() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::new();
            let s = MemSettings::gnome();
            let clean = home.host(&StdFs, &s);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
                fs::create_dir_all(home.path(PROFILE).join("chrome")).unwrap();
                fs::write(home.path(USER_CHROME), "#nav-bar { border: none; }\n").unwrap();
            }
            let before = home.snapshot();
            let p = plan(&clean, &t[1]);

            let faults = Faults::new(&StdFs, &s, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &p, &[FIREFOX.to_owned()]).is_ok() {
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                home.snapshot(),
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
fn where_firefox_keeps_its_profiles() {
    let t = bundled::themes();
    let s = MemSettings::gnome();

    // Not here at all.
    let home = Home::new();
    fs::remove_dir_all(home.path(".mozilla")).unwrap();
    let p = plan(&home.host(&StdFs, &s), &t[0]);
    assert_eq!(target(&p, FIREFOX).skipped, Some(Skip::NotInstalled));

    // A list of profiles that names none Firefox opens, or one that is
    // not there, or one outside its folder.
    for listed in [
        "[General]\nVersion=2\n",
        "[Profile0]\nName=x\nIsRelative=1\nPath=gone.default\nDefault=1\n",
        "[Profile0]\nName=x\nIsRelative=1\nPath=../../.ssh\nDefault=1\n",
    ] {
        let home = Home::new();
        fs::remove_file(home.path(ROOT).join("installs.ini")).unwrap();
        fs::write(home.path(ROOT).join("profiles.ini"), listed).unwrap();
        let p = plan(&home.host(&StdFs, &s), &t[0]);
        assert_eq!(
            target(&p, FIREFOX).skipped,
            Some(Skip::FirefoxNoProfile),
            "{listed:?}"
        );
    }

    // An older Firefox, with only Default=1 to go by; and one profile.
    let home = Home::new();
    fs::remove_file(home.path(ROOT).join("installs.ini")).unwrap();
    fs::write(
        home.path(ROOT).join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=w6hrzc8i.default-release\nDefault=1\n",
    )
    .unwrap();
    let host = home.host(&StdFs, &s);
    let report = apply(&host, &t[0]);
    assert!(home.path(SHEET).exists());
    assert!(
        !report
            .notes
            .iter()
            .any(|(_, n)| *n == Note::FirefoxOtherProfiles)
    );

    // The Flatpak keeps its profiles in its own folder.
    let home = Home::new();
    let flatpak = ".var/app/org.mozilla.firefox/.mozilla";
    fs::create_dir_all(home.path(flatpak)).unwrap();
    fs::rename(
        home.path(".mozilla/firefox"),
        home.path(flatpak).join("firefox"),
    )
    .unwrap();
    let host = home.host(&StdFs, &s);
    apply(&host, &t[0]);
    assert!(
        home.path(flatpak)
            .join("firefox/w6hrzc8i.default-release/chrome/velatura.css")
            .exists()
    );

    // A profile kept elsewhere by its whole path, as Firefox writes it
    // for one made in another folder.
    let home = Home::new();
    let elsewhere = home.path("profiles/mine");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::remove_file(home.path(ROOT).join("installs.ini")).unwrap();
    fs::write(
        home.path(ROOT).join("profiles.ini"),
        format!(
            "[Profile0]\nName=mine\nIsRelative=0\nPath={}\nDefault=1\n",
            elsewhere.display()
        ),
    )
    .unwrap();
    apply(&home.host(&StdFs, &s), &t[0]);
    assert!(elsewhere.join("chrome/velatura.css").exists());
}

#[test]
fn a_file_of_that_name_that_is_not_velaturas_is_not_replaced() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    fs::create_dir_all(home.path(PROFILE).join("chrome")).unwrap();
    fs::write(home.path(SHEET), ":root { --mine: red; }\n").unwrap();
    let p = plan(&home.host(&StdFs, &s), &t[0]);
    assert_eq!(
        target(&p, FIREFOX).skipped,
        Some(Skip::ForeignFile(home.path(SHEET)))
    );
}
