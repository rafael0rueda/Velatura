//! Transaction tests for recolored icons, added in phase 3d. The icon
//! themes are small fixtures; every test runs in a throw-away home folder
//! with settings held in memory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::look::{self, Kind};
use velatura_engine::plan::{Change, Note, Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::targets::{ICONS, INTERFACE_SCHEMA};
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The fixture icon themes stand where the system's would.
fn env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| fixtures().into_os_string())
}

fn settings(icons: &str) -> MemSettings {
    let s = MemSettings::new(&[
        (INTERFACE_SCHEMA, "icon-theme", "Adwaita"),
        (INTERFACE_SCHEMA, "color-scheme", "default"),
        (INTERFACE_SCHEMA, "accent-color", "blue"),
    ]);
    s.write(INTERFACE_SCHEMA, "icon-theme", Some(icons))
        .unwrap();
    s
}

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-icon-sets-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        for dir in [".local/state", ".local/share"] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        Self { root, dirs }
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    fn icons(&self) -> PathBuf {
        self.dirs.data.join("icons")
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
                if rel.starts_with(".local/state") || rel.starts_with(".local/share/velatura") {
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

fn opts(unattended: bool) -> ApplyOpts {
    ApplyOpts {
        only: vec![ICONS.to_owned()],
        unattended,
        ..ApplyOpts::default()
    }
}

fn plan_as(host: &Host<'_>, theme: &Theme, unattended: bool) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &env, &resolved, &opts(unattended)).unwrap()
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    plan_as(host, theme, false)
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan(host, theme), &[]).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

fn icon_theme(s: &MemSettings) -> String {
    s.read(INTERFACE_SCHEMA, "icon-theme")
        .unwrap()
        .unwrap()
        .effective
}

#[test]
fn the_marked_icons_are_recolored_in_a_set_of_their_own_and_undone() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Mini");
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();

    // Off until turned on.
    let resolved = Resolved::new(&t[1], None).unwrap();
    let all = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    assert_eq!(target(&all, ICONS).skipped, Some(Skip::NotSelected));

    let p = plan(&host, &t[1]);
    assert!(
        p.needing_consent().is_empty(),
        "turning it on is the agreement"
    );
    apply(&host, &t[1]);
    let name = icon_theme(&s);
    assert!(name.starts_with("Velatura-mini-"), "{name}");
    let set = home.icons().join(&name);
    let mut files: Vec<String> = walk_files(&set);
    files.sort();
    // The folders with the accent; not the grey bin, not the app, not the
    // symbolic folder.
    assert_eq!(
        files,
        [
            "16/places/folder.svg",
            "index.theme",
            "scalable/places/folder-music.svg",
            "scalable/places/folder.svg",
        ]
    );
    let folder = fs::read_to_string(set.join("scalable/places/folder.svg")).unwrap();
    assert!(
        folder.contains("fill=\"#e0914f\""),
        "Ember's accent: {folder}"
    );
    assert!(folder.contains("fill=\"#ffffff\""), "white stays white");
    assert!(!folder.to_lowercase().contains("#7e57c2"));
    let index = fs::read_to_string(set.join("index.theme")).unwrap();
    assert!(index.contains("Inherits=Mini\nHidden=true\n"), "{index}");
    assert!(
        index.contains("Directories=scalable/places,16/places\n"),
        "{index}"
    );

    // The same colours again: nothing.
    assert!(plan(&host, &t[1]).is_empty());

    // Another theme: a set of its own, and the first one taken away.
    apply(&host, &t[0]);
    let second = icon_theme(&s);
    assert_ne!(second, name);
    assert!(home.icons().join(&second).join("index.theme").exists());
    assert!(!set.exists(), "the earlier set is cleared");

    undo(&host);
    assert_eq!(icon_theme(&s), name);
    assert_eq!(walk_files(&set).len(), 4, "put back from the backup");
    assert!(!home.icons().join(&second).exists());
    undo(&host);
    assert_eq!(home.snapshot(), start);
    assert_eq!(icon_theme(&s), "Mini");
}

fn walk_files(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).into_iter().flatten() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn exhaustive_crash_test_over_a_set() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::new();
            let s = settings("Mini");
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
            if engine::execute(&dying, &p, &[]).is_ok() {
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
            crash_points >= 6,
            "only {crash_points} crash points exercised"
        );
    }
}

#[test]
fn a_switch_with_no_one_present_never_takes_a_set_away() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Mini");
    let host = home.host(&StdFs, &s);
    apply(&host, &t[0]);
    let first = home.icons().join(icon_theme(&s));
    engine::execute(&host, &plan_as(&host, &t[1], true), &[]).unwrap();
    let second = home.icons().join(icon_theme(&s));
    assert!(first.exists() && second.exists());
    // Back to the first colours, still unattended: that set is there and
    // is used again as it is.
    let back = plan_as(&host, &t[0], true);
    let changes: Vec<&Change> = back.changes().collect();
    assert!(
        matches!(changes.as_slice(), [Change::Setting { key, .. }] if key == "icon-theme"),
        "{changes:?}"
    );
    // The next apply with someone present clears what is not in use.
    apply(&host, &t[0]);
    assert!(first.exists() && !second.exists());
}

#[test]
fn an_icon_edited_in_a_set_is_kept_and_its_folder_with_it() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Mini");
    let host = home.host(&StdFs, &s);
    apply(&host, &t[0]);
    let set = home.icons().join(icon_theme(&s));
    fs::write(set.join("16/places/folder.svg"), "<svg/>").unwrap();
    let report = apply(&host, &t[1]);
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KeptFile { .. }))
    );
    assert_eq!(walk_files(&set), ["16/places/folder.svg"]);
}

#[test]
fn themes_that_cannot_be_recolored_are_left_out_with_the_reason() {
    let t = bundled::themes();
    for (theme, why) in [
        // Its folder is not a picture this reads.
        ("Pics", Skip::IconsNotRecolorable("Pics".to_owned())),
        (
            "NotInstalledAnywhere",
            Skip::IconsNotRecolorable("NotInstalledAnywhere".to_owned()),
        ),
        (
            "../../etc",
            Skip::IconsNotRecolorable("../../etc".to_owned()),
        ),
    ] {
        let home = Home::new();
        let s = settings(theme);
        let p = plan(&home.host(&StdFs, &s), &t[0]);
        assert_eq!(target(&p, ICONS).skipped, Some(why), "{theme}");
    }
    // High Contrast has its own icons.
    let home = Home::new();
    let s = MemSettings::new(&[
        (INTERFACE_SCHEMA, "icon-theme", "Mini"),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "true"),
    ]);
    let p = plan(&home.host(&StdFs, &s), &t[0]);
    assert_eq!(target(&p, ICONS).skipped, Some(Skip::HighContrast));
}

#[test]
fn a_folder_of_that_name_that_is_not_velaturas_is_not_written_into() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Mini");
    let host = home.host(&StdFs, &s);
    let p = plan(&host, &t[0]);
    let name = target(&p, ICONS)
        .changes
        .iter()
        .find_map(|c| match c {
            Change::Setting { value, .. } => value.clone(),
            _ => None,
        })
        .unwrap();
    fs::create_dir_all(home.icons().join(&name)).unwrap();
    let p = plan(&host, &t[0]);
    assert_eq!(
        target(&p, ICONS).skipped,
        Some(Skip::ForeignFile(home.icons().join(&name)))
    );
}

#[test]
fn a_set_is_not_offered_as_an_icon_theme_and_is_made_from_its_base_again() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Mini");
    let host = home.host(&StdFs, &s);
    apply(&host, &t[0]);
    let choices: Vec<String> = look::available(&host, &env, Kind::Icons)
        .into_iter()
        .map(|c| c.value)
        .collect();
    assert!(choices.contains(&"Mini".to_owned()), "{choices:?}");
    assert!(
        !choices.iter().any(|c| c.starts_with("Velatura-")),
        "{choices:?}"
    );
    // In use now is the set; the next theme is still made from Mini.
    apply(&host, &t[1]);
    assert!(icon_theme(&s).starts_with("Velatura-mini-"));

    // Choosing Mini itself gives the original icons back.
    let back = look::plan_select(&host, &env, Kind::Icons, Some("Mini")).unwrap();
    engine::execute(&host, &back, &[]).unwrap();
    assert_eq!(icon_theme(&s), "Mini");
}

/// An icon theme made of pictures (phase 4d): the pictures that carry its
/// folders' colour are recoloured pixel by pixel; links become copies;
/// grey pictures and app icons are left to the original.
#[test]
fn pictures_are_recolored_pixel_by_pixel_and_undone() {
    use velatura_theme::png;
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Frosty");
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();
    let report = apply(&host, &t[1]);
    let name = icon_theme(&s);
    assert!(name.starts_with("Velatura-frosty-"), "{name}");
    let set = home.icons().join(&name);
    let mut files = walk_files(&set);
    files.sort();
    assert_eq!(
        files,
        [
            "48/mimetypes/audio-x-generic.png",
            "48/places/folder.png",
            "48/places/user-home.png",
            "index.theme"
        ]
    );
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    // GTK reads what was written; the folder's body is Ember's accent.
    let folder = png::decode(&fs::read(set.join("48/places/folder.png")).unwrap()).unwrap();
    let original =
        png::decode(&fs::read(fixtures().join("icons/Frosty/48/places/folder.png")).unwrap())
            .unwrap();
    assert_eq!((folder.width, folder.height), (48, 48));
    let at = |img: &png::Image, x: usize, y: usize| {
        let i = (y * 48 + x) * 4;
        img.rgba[i..i + 4].to_vec()
    };
    let accent = Resolved::new(&t[1], None).unwrap().colors.ui.accent;
    let body = at(&folder, 10, 12);
    for (got, want) in body.iter().zip([accent.r, accent.g, accent.b]) {
        assert!(got.abs_diff(want) <= 3, "{body:?} against {}", accent.hex());
    }
    // The grey label, the alpha and the transparent edge are as they were.
    assert_eq!(at(&folder, 20, 25), at(&original, 20, 25));
    assert_eq!(at(&folder, 0, 0), at(&original, 0, 0));
    assert_eq!(at(&folder, 1, 10)[3], 128);
    // Another theme makes another set, and two undos are the start.
    apply(&host, &t[0]);
    assert_ne!(icon_theme(&s), name);
    undo(&host);
    undo(&host);
    assert_eq!(home.snapshot(), start);
    assert_eq!(icon_theme(&s), "Frosty");
}
