//! Transaction tests for recolored cursors, added in phase 4d. The cursor
//! theme is a small fixture; every test runs in a throw-away home folder
//! with settings held in memory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::targets::{CURSORS, ICONS, INTERFACE_SCHEMA};
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The fixture themes stand where the system's would.
fn env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| fixtures().into_os_string())
}

fn settings(cursor: &str) -> MemSettings {
    let s = MemSettings::new(&[
        (INTERFACE_SCHEMA, "cursor-theme", "Adwaita"),
        (INTERFACE_SCHEMA, "icon-theme", "Adwaita"),
    ]);
    s.write(INTERFACE_SCHEMA, "cursor-theme", Some(cursor))
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
            "velatura-cursor-sets-{}-{}",
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

fn opts(only: &str, unattended: bool) -> ApplyOpts {
    ApplyOpts {
        only: vec![only.to_owned()],
        unattended,
        ..ApplyOpts::default()
    }
}

fn plan_as(host: &Host<'_>, theme: &Theme, unattended: bool) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &env, &resolved, &opts(CURSORS, unattended)).unwrap()
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan_as(host, theme, false), &[]).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

fn cursor_theme(s: &MemSettings) -> String {
    s.read(INTERFACE_SCHEMA, "cursor-theme")
        .unwrap()
        .unwrap()
        .effective
}

fn files(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir.join("cursors"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn the_cursors_are_recolored_in_a_set_of_their_own_and_undone() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Pointy");
    let host = home.host(&StdFs, &s);
    let start = home.snapshot();

    // Off until turned on; turning it on is the agreement.
    let resolved = Resolved::new(&t[1], None).unwrap();
    let p = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    assert_eq!(target(&p, CURSORS).skipped, Some(Skip::NotSelected));
    engine::set_included(&host, CURSORS, true).unwrap();
    let p = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    assert!(target(&p, CURSORS).skipped.is_none());
    assert!(p.needing_consent().is_empty());

    apply(&host, &t[1]);
    let name = cursor_theme(&s);
    assert!(name.starts_with("Velatura-cursors-pointy-"), "{name}");
    let set = home.icons().join(&name);
    // The alias is a copy; what is not a cursor stays out.
    assert_eq!(files(&set), ["default", "left_ptr", "watch"]);
    let read = |n: &str| fs::read(set.join("cursors").join(n)).unwrap();
    assert_eq!(read("default"), read("left_ptr"));
    let index = fs::read_to_string(set.join("index.theme")).unwrap();
    assert!(index.contains("Inherits=Pointy\nHidden=true\n"));
    // The arrow's black body is Ember's background now, its outline
    // Ember's text: the first row is outline, the second's middle body.
    let ui = &resolved.colors.ui;
    let arrow = read("left_ptr");
    let pixel = |at: usize| (arrow[at + 2], arrow[at + 1], arrow[at], arrow[at + 3]);
    let first = 16 + 2 * 12 + 36;
    assert_eq!(pixel(first), (ui.fg.r, ui.fg.g, ui.fg.b, 255));
    assert_eq!(
        pixel(first + (2 * 24 + 1) * 4),
        (ui.bg.r, ui.bg.g, ui.bg.b, 255)
    );
    assert_eq!(
        arrow.len(),
        fs::read(fixtures().join("icons/Pointy/cursors/left_ptr"))
            .unwrap()
            .len()
    );

    // The same again changes nothing; another theme makes another set and
    // clears this one; two undos are the start.
    assert!(plan_as(&host, &t[1], false).is_empty());
    apply(&host, &t[0]);
    assert_ne!(cursor_theme(&s), name);
    assert!(!set.exists());
    undo(&host);
    assert!(set.exists());
    undo(&host);
    assert_eq!(home.snapshot(), start);
    assert_eq!(cursor_theme(&s), "Pointy");
}

#[test]
fn exhaustive_crash_test() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::new();
            let s = settings("Pointy");
            let clean = home.host(&StdFs, &s);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
            }
            let before = (home.snapshot(), s.snapshot());
            let p = plan_as(&clean, &t[1], false);
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
                "crash at step {fail_at} (applied: {start_applied})"
            );
        }
        assert!(
            crash_points >= 3,
            "only {crash_points} crash points exercised"
        );
    }
}

#[test]
fn a_switch_with_no_one_present_keeps_earlier_sets_and_icons_leave_cursors_alone() {
    let t = bundled::themes();
    let home = Home::new();
    let s = settings("Pointy");
    s.write(INTERFACE_SCHEMA, "icon-theme", Some("Frosty"))
        .unwrap();
    let host = home.host(&StdFs, &s);
    apply(&host, &t[0]);
    let first = cursor_theme(&s);
    // A scheduled switch makes the new set and keeps the old one.
    let p = plan_as(&host, &t[1], true);
    engine::execute(&host, &p, &[]).unwrap();
    assert!(home.icons().join(&first).exists());
    // Recolouring icons, with someone present, clears earlier icon sets
    // only: the cursor sets are not its to take away.
    let resolved = Resolved::new(&t[1], None).unwrap();
    let p = engine::plan_apply(&host, &env, &resolved, &opts(ICONS, false)).unwrap();
    engine::execute(&host, &p, &[]).unwrap();
    let names: Vec<String> = fs::read_dir(home.icons())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("Velatura-cursors-"))
            .count(),
        2
    );
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("Velatura-frosty-"))
            .count(),
        1
    );
}

#[test]
fn cursor_themes_that_cannot_be_recolored_are_left_out() {
    let t = bundled::themes();
    for name in ["NotInstalledAnywhere", "Mini", "../../etc"] {
        let home = Home::new();
        let s = settings(name);
        let p = plan_as(&home.host(&StdFs, &s), &t[0], false);
        assert_eq!(
            target(&p, CURSORS).skipped,
            Some(Skip::CursorsNotRecolorable(name.to_owned())),
            "{name}"
        );
    }
}

/// How the parts stand, as the app shows them, is worked out without
/// making a set: the cursor part is ready, and the plan that says so holds
/// no cursor to write.
#[test]
fn the_parts_list_does_not_make_a_set_to_say_one_would_be_made() {
    use velatura_engine::engine::PartState;
    let home = Home::new();
    let s = settings("Pointy");
    let host = home.host(&StdFs, &s);
    let (themes, _) = velatura_engine::library::load(&host);
    let parts = engine::parts(&host, &env, &themes);
    let cursors = parts.iter().find(|p| p.instance == CURSORS).unwrap();
    assert_eq!(cursors.state, PartState::Ready);
    let t = bundled::themes();
    let resolved = Resolved::new(&t[0], None).unwrap();
    let probe = ApplyOpts {
        only: vec![CURSORS.to_owned()],
        probe: true,
        ..ApplyOpts::default()
    };
    let p = engine::plan_apply(&host, &env, &resolved, &probe).unwrap();
    assert!(
        !p.changes()
            .any(|c| matches!(c, velatura_engine::plan::Change::Owned { .. }))
    );
    assert!(!home.icons().exists(), "nothing was written");
}
