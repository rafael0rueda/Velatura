//! Transaction tests: every one runs in a throw-away home folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Error, Health};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, NoSettings, Settings, StdFs};
use velatura_engine::plan::{Note, Plan, Skip};
use velatura_engine::store;
use velatura_engine::targets::{APPEARANCE, INTERFACE_SCHEMA, KITTY};
use velatura_engine::txn::Failure;
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A home folder under the system temp dir, removed on drop.
struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-txn-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        // The base folders every desktop session already has.
        for base in [".config", ".local/share", ".local/state"] {
            fs::create_dir_all(home.join(base)).unwrap();
        }
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        Self { root, dirs }
    }

    /// A home with a Kitty settings file holding `conf`.
    fn with_kitty(conf: &[u8]) -> Self {
        let h = Self::new();
        fs::create_dir_all(h.kitty()).unwrap();
        fs::write(h.kitty().join("kitty.conf"), conf).unwrap();
        h
    }

    fn kitty(&self) -> PathBuf {
        self.dirs.config.join("kitty")
    }

    fn conf(&self) -> PathBuf {
        self.kitty().join("kitty.conf")
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    /// Every file under the home folder with its content, and every folder
    /// that is not Velatura's own. Leaves out what a rolled-back run may
    /// legitimately leave behind: stored content, the lock, the cache tag.
    fn snapshot(&self) -> BTreeMap<String, Option<Vec<u8>>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    if rel.ends_with("backups/blobs") {
                        continue;
                    }
                    if !rel.contains("velatura") {
                        out.insert(format!("{rel}/"), None);
                    }
                    walk(&path, base, out);
                } else if meta.file_type().is_symlink() {
                    let to = fs::read_link(&path).unwrap();
                    out.insert(rel, Some(to.to_string_lossy().into_owned().into_bytes()));
                } else if !rel.ends_with("velatura/lock") && !rel.ends_with("CACHEDIR.TAG") {
                    out.insert(rel, Some(fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.dirs.home, &self.dirs.home, &mut out);
        out
    }

    /// The snapshot without anything of Velatura's own: what the user sees.
    fn user_files(&self) -> BTreeMap<String, Option<Vec<u8>>> {
        let mut s = self.snapshot();
        s.retain(|k, _| !k.contains("/velatura/"));
        s
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        // A test may have made a folder read-only.
        let _ = fs::set_permissions(self.kitty(), fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// An environment with nothing in it, and with the system's shared data
/// out of sight: what is installed on the machine running the tests (an
/// extension, a GTK theme) must not change their outcome.
fn no_env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| OsString::from("/nonexistent-velatura-test"))
}

fn themes() -> Vec<Theme> {
    bundled::themes()
}

/// The two targets of phase 1a, which most tests below are written around.
fn two() -> ApplyOpts {
    ApplyOpts {
        only: vec!["kitty".to_owned(), "gnome:appearance".to_owned()],
        ..ApplyOpts::default()
    }
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    let mut p = engine::plan_apply(host, &no_env, &resolved, &two()).unwrap();
    p.targets.retain(|t| t.skipped != Some(Skip::NotSelected));
    p
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a velatura_engine::plan::TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

fn yes() -> Vec<String> {
    vec![KITTY.to_owned()]
}

fn apply(host: &Host<'_>, theme: &Theme) -> engine::Report {
    engine::execute(host, &plan(host, theme), &yes()).unwrap()
}

fn undo(host: &Host<'_>) -> engine::Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

const CONFS: [&[u8]; 6] = [
    b"font_size 12\nbackground_opacity 0.9\n",
    b"font_size 12",
    b"",
    b"font_size 12\r\ninclude other.conf\r\n",
    b"# only a comment",
    b"\xff\xfe not utf-8 \xe9\n",
];

#[test]
fn apply_then_undo_equals_the_start() {
    let t = themes();
    for conf in CONFS {
        let home = Home::with_kitty(conf);
        let settings = MemSettings::gnome();
        let host = home.host(&StdFs, &settings);
        let before = home.user_files();

        let report = apply(&host, &t[0]);
        assert_eq!(report.backup, Some(1));
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        let conf_now = fs::read(home.conf()).unwrap();
        assert!(conf_now.starts_with(conf));
        assert!(String::from_utf8_lossy(&conf_now).contains("include velatura.conf"));
        assert!(home.kitty().join("velatura.conf").exists());
        let read = |key| {
            settings
                .read(INTERFACE_SCHEMA, key)
                .unwrap()
                .unwrap()
                .effective
        };
        assert_eq!(read("color-scheme"), "prefer-dark");
        assert_eq!(read("accent-color"), "blue");
        assert_eq!(
            engine::status(&host).unwrap().applied.unwrap().0,
            "aetherius/default"
        );

        undo(&host);
        assert_eq!(home.user_files(), before);
        assert!(settings.snapshot().is_empty(), "settings back at default");
        assert!(engine::status(&host).unwrap().applied.is_none());
        assert!(matches!(
            engine::plan_undo(&host),
            Err(Error::NothingToUndo)
        ));
    }
}

#[test]
fn applying_twice_changes_nothing_the_second_time() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[1]);
    let after = home.snapshot();
    let writes = settings.writes.get();

    let again = plan(&host, &t[1]);
    assert!(again.is_empty());
    assert!(again.needing_consent().is_empty());
    let report = engine::execute(&host, &again, &yes()).unwrap();
    assert_eq!(report.backup, None);
    assert_eq!(home.snapshot(), after, "no new backup, nothing rewritten");
    assert_eq!(settings.writes.get(), writes, "no setting written");
}

#[test]
fn a_b_undo_undo_equals_the_start() {
    let t = themes();
    let home = Home::with_kitty(CONFS[1]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();

    apply(&host, &t[0]);
    let after_a = (home.user_files(), settings.snapshot());
    let report = apply(&host, &t[1]);
    assert_eq!(report.backup, Some(2));
    assert_eq!(
        settings
            .read(INTERFACE_SCHEMA, "accent-color")
            .unwrap()
            .unwrap()
            .effective,
        "orange"
    );
    // The second apply touches the colour file and the accent, not the block.
    assert_eq!(report.changes.len(), 2);

    undo(&host);
    assert_eq!((home.user_files(), settings.snapshot()), after_a);
    assert_eq!(
        engine::status(&host).unwrap().applied.unwrap().0,
        "aetherius/default"
    );
    undo(&host);
    assert_eq!(home.user_files(), start);
    assert!(settings.snapshot().is_empty());
}

#[test]
fn undo_keeps_what_the_user_edited_since() {
    let t = themes();
    let home = Home::with_kitty(b"font_size 12");
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[1]);

    // The user edits around the block and changes the accent in Settings.
    let mut conf = b"# added at the top\n".to_vec();
    conf.extend(fs::read(home.conf()).unwrap());
    conf.extend(b"map f1 new_window\n");
    fs::write(home.conf(), conf).unwrap();
    settings
        .write(INTERFACE_SCHEMA, "accent-color", Some("green"))
        .unwrap();

    let plan = engine::plan_undo(&host).unwrap();
    let report = engine::execute(&host, &plan, &[]).unwrap();
    assert_eq!(
        fs::read(home.conf()).unwrap(),
        b"# added at the top\nfont_size 12\nmap f1 new_window\n"
    );
    assert!(!home.kitty().join("velatura.conf").exists());
    let now = settings.snapshot();
    assert_eq!(
        now.len(),
        1,
        "the dark style was put back, the accent left alone"
    );
    assert_eq!(now.values().next().unwrap(), "green");
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KeptSetting { .. }))
    );
}

#[test]
fn undo_leaves_an_owned_file_the_user_changed() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    let owned = home.kitty().join("velatura.conf");
    fs::write(&owned, b"background #000000\n").unwrap();
    assert_eq!(engine::status(&host).unwrap().files[0].1, Health::Changed);

    // A new apply does not overwrite it either, unless told to.
    let skipped = plan(&host, &t[1]);
    assert!(matches!(
        skipped.targets[0].skipped,
        Some(Skip::ChangedOutside(_))
    ));
    let resolved = Resolved::new(&t[1], None).unwrap();
    let forced = ApplyOpts {
        replace_changed: true,
        ..two()
    };
    let replace = engine::plan_apply(&host, &no_env, &resolved, &forced).unwrap();
    assert!(replace.targets[0].skipped.is_none());

    let report = undo(&host);
    assert_eq!(fs::read(&owned).unwrap(), b"background #000000\n");
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KeptFile { .. }))
    );
    assert_eq!(fs::read(home.conf()).unwrap(), CONFS[0]);
}

#[test]
fn creates_the_settings_file_and_folder_when_kitty_has_none() {
    let t = themes();
    let home = Home::new();
    let bin = home.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("kitty"), b"").unwrap();
    let env = |k: &str| (k == "PATH").then(|| OsString::from(&bin));
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();

    // Without Kitty anywhere, it is reported as not installed.
    let resolved = Resolved::new(&t[0], None).unwrap();
    let none = engine::plan_apply(&host, &no_env, &resolved, &ApplyOpts::default()).unwrap();
    assert_eq!(none.targets[0].skipped, Some(Skip::NotInstalled));

    let plan = engine::plan_apply(&host, &env, &resolved, &ApplyOpts::default()).unwrap();
    engine::execute(&host, &plan, &yes()).unwrap();
    assert_eq!(
        fs::read_to_string(home.conf()).unwrap(),
        "# >>> velatura kitty 1 >>>\ninclude velatura.conf\n# <<< velatura kitty 1 <<<\n"
    );
    undo(&host);
    assert_eq!(home.user_files(), start, "file and folder removed again");
}

#[test]
fn without_agreement_the_users_file_is_not_touched() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();

    let p = plan(&host, &t[0]);
    assert_eq!(p.needing_consent(), [KITTY]);
    let report = engine::execute(&host, &p, &[]).unwrap();
    assert_eq!(report.skipped, [(KITTY.to_owned(), Skip::NoConsent)]);
    assert_eq!(home.user_files(), start);
    assert_eq!(
        settings.snapshot().len(),
        1,
        "the settings were still applied"
    );

    // Once agreed, later themes do not ask again.
    apply(&host, &t[0]);
    assert!(plan(&host, &t[1]).needing_consent().is_empty());
}

#[test]
fn only_and_except_select_targets() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let resolved = Resolved::new(&t[0], None).unwrap();
    let only = ApplyOpts {
        only: vec!["gnome".to_owned()],
        ..ApplyOpts::default()
    };
    let p = engine::plan_apply(&host, &no_env, &resolved, &only).unwrap();
    assert_eq!(target(&p, KITTY).skipped, Some(Skip::NotSelected));
    assert!(target(&p, APPEARANCE).skipped.is_none());
    let except = ApplyOpts {
        except: vec![APPEARANCE.to_owned()],
        ..ApplyOpts::default()
    };
    let p = engine::plan_apply(&host, &no_env, &resolved, &except).unwrap();
    assert!(target(&p, KITTY).skipped.is_none());
    assert_eq!(target(&p, APPEARANCE).skipped, Some(Skip::NotSelected));
}

/// Stop an apply at every step in turn, as if the process had died there,
/// then run recovery and compare with the state before the apply.
#[test]
fn exhaustive_crash_test() {
    let t = themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::with_kitty(b"font_size 12");
            let settings = MemSettings::gnome();
            let clean = home.host(&StdFs, &settings);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                // The store's own folders exist from an earlier, unrelated run.
                store::ensure_store(&clean).unwrap();
            }
            let before = (home.snapshot(), settings.snapshot());
            let p = plan(&clean, &t[1]);

            let faults = Faults::new(&StdFs, &settings, fail_at);
            let dying = home.host(&faults, &faults);
            let result = engine::execute(&dying, &p, &yes());
            if result.is_ok() {
                assert_ne!((home.snapshot(), settings.snapshot()), before);
                break;
            }
            crash_points += 1;

            // Next start: a healthy process finds the journal and rolls back.
            let recovered = engine::recover(&clean).unwrap();
            if let Some(r) = &recovered {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                (home.snapshot(), settings.snapshot()),
                before,
                "state differs after a crash at step {fail_at} (already applied: {start_applied})"
            );
            assert!(engine::recover(&clean).unwrap().is_none(), "journal gone");

            // And the machine is fully usable afterwards.
            apply(&clean, &t[1]);
            undo(&clean);
            assert_eq!((home.user_files(), settings.snapshot().len()), {
                let mut b = before.0.clone();
                b.retain(|k, _| !k.contains("/velatura/"));
                (b, before.1.len())
            });
        }
        assert!(
            crash_points >= 7,
            "only {crash_points} crash points exercised"
        );
    }
}

/// The same for undo: an interrupted undo leaves the applied state intact.
#[test]
fn exhaustive_crash_test_for_undo() {
    let t = themes();
    let mut crash_points = 0;
    for fail_at in 0.. {
        let home = Home::with_kitty(CONFS[0]);
        let settings = MemSettings::gnome();
        let clean = home.host(&StdFs, &settings);
        apply(&clean, &t[0]);
        apply(&clean, &t[1]);
        let before = (home.snapshot(), settings.snapshot());
        let p = engine::plan_undo(&clean).unwrap();

        let faults = Faults::new(&StdFs, &settings, fail_at);
        let dying = home.host(&faults, &faults);
        if engine::execute(&dying, &p, &[]).is_ok() {
            break;
        }
        crash_points += 1;
        engine::recover(&clean).unwrap();
        assert_eq!(
            (home.snapshot(), settings.snapshot()),
            before,
            "crash at step {fail_at}"
        );
        undo(&clean);
        undo(&clean);
        assert_eq!(fs::read(home.conf()).unwrap(), CONFS[0]);
    }
    assert!(
        crash_points >= 4,
        "only {crash_points} crash points exercised"
    );
}

#[test]
fn recovery_keeps_work_done_after_the_crash() {
    let t = themes();
    let home = Home::with_kitty(b"font_size 12\n");
    let settings = MemSettings::gnome();
    let clean = home.host(&StdFs, &settings);
    store::ensure_store(&clean).unwrap();
    let p = plan(&clean, &t[1]);

    // Die on the last step: everything is written, the journal remains.
    let total = {
        let probe_home = Home::with_kitty(b"font_size 12\n");
        let s = MemSettings::gnome();
        let counting = Faults::new(&StdFs, &s, u32::MAX);
        let h = probe_home.host(&counting, &counting);
        store::ensure_store(&h).unwrap();
        let base = counting.count.get();
        engine::execute(&h, &plan(&h, &t[1]), &yes()).unwrap();
        counting.count.get() - base
    };
    let faults = Faults::new(&StdFs, &settings, total - 1);
    let dying = home.host(&faults, &faults);
    assert!(engine::execute(&dying, &p, &yes()).is_err());

    // Before the next start the user edits the file and changes a setting.
    let mut conf = fs::read(home.conf()).unwrap();
    assert!(String::from_utf8_lossy(&conf).contains("include velatura.conf"));
    conf.extend(b"map f2 next_tab\n");
    fs::write(home.conf(), &conf).unwrap();
    settings
        .write(INTERFACE_SCHEMA, "accent-color", Some("pink"))
        .unwrap();
    // And an interrupted atomic write left its temporary file behind.
    let stray = home.kitty().join(".kitty.conf.velatura-tmp-1-1");
    fs::write(&stray, b"partial").unwrap();

    let r = engine::recover(&clean).unwrap().unwrap();
    assert!(r.is_clean());
    assert_eq!(r.kept.len(), 2, "{:?}", r.kept);
    assert_eq!(fs::read(home.conf()).unwrap(), conf, "newer work is kept");
    assert_eq!(
        settings.snapshot().len(),
        1,
        "dark style rolled back, accent kept"
    );
    assert!(
        !home.kitty().join("velatura.conf").exists(),
        "untouched file rolled back"
    );
    assert!(!stray.exists(), "temporary file swept");
}

#[test]
fn a_change_between_preview_and_apply_stops_the_apply() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let p = plan(&host, &t[0]);
    fs::write(home.conf(), b"font_size 14\n").unwrap();
    let before = home.user_files();
    match engine::execute(&host, &p, &yes()) {
        Err(Error::Failed(Failure::Stale(what))) => assert!(what.ends_with("kitty.conf")),
        other => panic!("expected stale, got {other:?}"),
    }
    assert_eq!(home.user_files(), before);
    assert!(settings.snapshot().is_empty());
}

#[test]
fn a_failing_write_rolls_everything_back() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let before = home.user_files();
    let p = plan(&host, &t[0]);
    // A folder that cannot be written to: disk full behaves the same way.
    fs::set_permissions(home.kitty(), fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(home.kitty().join("probe"), b"").is_ok() {
        // Running as root (a CI container): permissions do not stop writes.
        // The exhaustive crash test covers the same path by injection.
        eprintln!("SKIPPED: file permissions are not enforced for this user");
        return;
    }
    match engine::execute(&host, &p, &yes()) {
        Err(Error::Failed(Failure::Step { rollback, .. })) => {
            assert!(
                rollback.is_clean() && rollback.kept.is_empty(),
                "{rollback:?}"
            );
        }
        other => panic!("expected a failed step, got {other:?}"),
    }
    fs::set_permissions(home.kitty(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(home.user_files(), before);
    assert!(settings.snapshot().is_empty());
    assert!(
        engine::history(&host).unwrap().0.is_empty(),
        "no backup left listed"
    );
}

#[test]
fn a_linked_settings_file_is_written_through_and_the_link_kept() {
    let t = themes();
    let home = Home::new();
    let dotfiles = home.dirs.home.join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    fs::create_dir_all(home.kitty()).unwrap();
    fs::write(dotfiles.join("kitty.conf"), b"font_size 12\n").unwrap();
    symlink(dotfiles.join("kitty.conf"), home.conf()).unwrap();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.user_files();

    apply(&host, &t[0]);
    assert!(
        fs::symlink_metadata(home.conf())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_to_string(dotfiles.join("kitty.conf"))
            .unwrap()
            .contains("velatura")
    );
    undo(&host);
    assert_eq!(home.user_files(), start);
}

#[test]
fn unsafe_or_damaged_places_are_skipped_not_forced() {
    let t = themes();
    let settings = MemSettings::gnome();
    let skipped = |home: &Home| {
        let host = home.host(&StdFs, &settings);
        let p = plan(&host, &t[0]);
        let before = home.user_files();
        let report = engine::execute(&host, &p, &yes()).unwrap();
        assert_eq!(home.user_files(), before, "nothing of the user's changed");
        report.skipped.into_iter().next().map(|(_, why)| why)
    };

    // A link that leaves the home folder.
    let home = Home::new();
    let outside = home.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(home.kitty()).unwrap();
    fs::write(outside.join("kitty.conf"), b"x\n").unwrap();
    symlink(outside.join("kitty.conf"), home.conf()).unwrap();
    assert!(matches!(skipped(&home), Some(Skip::OutsideHome(_))));

    // The whole folder is a link that leaves the home folder.
    let home = Home::new();
    let outside = home.root.join("outside-dir");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, home.kitty()).unwrap();
    assert!(matches!(skipped(&home), Some(Skip::OutsideHome(_))));
    assert!(fs::read_dir(&outside).unwrap().next().is_none());

    // A dangling link, and a looping one.
    let home = Home::new();
    fs::create_dir_all(home.kitty()).unwrap();
    symlink(home.kitty().join("missing"), home.conf()).unwrap();
    assert!(matches!(skipped(&home), Some(Skip::NotAFile(_))));
    let home = Home::new();
    fs::create_dir_all(home.kitty()).unwrap();
    symlink(home.conf(), home.conf()).unwrap();
    assert!(matches!(skipped(&home), Some(Skip::NotAFile(_))));

    // A folder where the file should be.
    let home = Home::new();
    fs::create_dir_all(home.conf()).unwrap();
    assert!(matches!(skipped(&home), Some(Skip::NotAFile(_))));

    // Half a marker, and a duplicated block.
    let home = Home::with_kitty(b"# >>> velatura kitty 1 >>>\ninclude velatura.conf\n");
    assert!(matches!(skipped(&home), Some(Skip::MalformedBlock(_))));
    let block = "# >>> velatura kitty 1 >>>\nx\n# <<< velatura kitty 1 <<<\n";
    let home = Home::with_kitty(format!("{block}{block}").as_bytes());
    assert!(matches!(skipped(&home), Some(Skip::MalformedBlock(_))));

    // A file with Velatura's name that Velatura did not make.
    let home = Home::with_kitty(CONFS[0]);
    fs::write(home.kitty().join("velatura.conf"), b"mine\n").unwrap();
    assert!(matches!(skipped(&home), Some(Skip::ForeignFile(_))));
}

#[test]
fn settings_problems_do_not_stop_the_rest() {
    let t = themes();

    // No settings store at all: Kitty is still themed.
    let home = Home::with_kitty(CONFS[0]);
    let host = home.host(&StdFs, &NoSettings);
    let report = apply(&host, &t[0]);
    assert!(matches!(report.skipped[0].1, Skip::SettingsUnavailable(_)));
    assert!(home.kitty().join("velatura.conf").exists());

    // An older GNOME without the accent key, and a locked key.
    let home = Home::with_kitty(CONFS[0]);
    let old = MemSettings::new(&[(INTERFACE_SCHEMA, "color-scheme", "default")]);
    let report = apply(&home.host(&StdFs, &old), &t[0]);
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| *n == Note::AccentUnsupported)
    );
    assert_eq!(old.snapshot().len(), 1);

    let home = Home::with_kitty(CONFS[0]);
    let locked = MemSettings::gnome();
    locked.lock(INTERFACE_SCHEMA, "color-scheme");
    let report = apply(&home.host(&StdFs, &locked), &t[1]);
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::SettingLocked { .. }))
    );
    assert_eq!(locked.snapshot().len(), 1, "the accent was still set");

    // A value the user had set is restored as set, not reset to default.
    let home = Home::with_kitty(CONFS[0]);
    let set = MemSettings::gnome();
    set.write(INTERFACE_SCHEMA, "accent-color", Some("teal"))
        .unwrap();
    set.write(INTERFACE_SCHEMA, "color-scheme", Some("prefer-dark"))
        .unwrap();
    let before = set.snapshot();
    let host = home.host(&StdFs, &set);
    let report = apply(&host, &t[1]);
    assert_eq!(report.changes.len(), 3, "dark style already as wanted");
    undo(&host);
    assert_eq!(set.snapshot(), before);
}

#[test]
fn reports_what_overrides_the_theme() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    fs::write(
        home.kitty().join("dark-theme.auto.conf"),
        b"background #000\n",
    )
    .unwrap();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let report = apply(&host, &t[0]);
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KittyAutoTheme { .. }))
    );
    assert!(report.notes.iter().any(|(_, n)| *n == Note::KittyReload));

    // The user later adds colour lines below the block.
    let mut conf = fs::read(home.conf()).unwrap();
    conf.extend(b"\nfont_size 13\nbackground #101010\ninclude extra.conf\n");
    fs::write(home.conf(), conf).unwrap();
    let p = plan(&host, &t[1]);
    let lines = p.targets[0].notes.iter().find_map(|n| match n {
        Note::Override { lines, .. } => Some(lines.clone()),
        _ => None,
    });
    assert_eq!(lines, Some(vec![8, 9]));
}

#[test]
fn restore_original_removes_everything_and_can_itself_be_undone() {
    let t = themes();
    let home = Home::with_kitty(b"font_size 12");
    let settings = MemSettings::gnome();
    settings
        .write(INTERFACE_SCHEMA, "accent-color", Some("teal"))
        .unwrap();
    let host = home.host(&StdFs, &settings);
    let start = (home.user_files(), settings.snapshot());

    // More applies than backups are kept, so undo alone could not get back.
    for i in 0..(store::KEEP + 5) {
        apply(&host, &t[i % 2]);
    }
    let (kept, damaged) = engine::history(&host).unwrap();
    assert_eq!(kept.len(), store::KEEP);
    assert!(damaged.is_empty());
    let applied = (home.user_files(), settings.snapshot());

    let p = engine::plan_restore_original(&host).unwrap();
    engine::execute(&host, &p, &[]).unwrap();
    assert_eq!((home.user_files(), settings.snapshot()), start);
    assert!(engine::plan_restore_original(&host).unwrap().is_empty());

    undo(&host);
    assert_eq!((home.user_files(), settings.snapshot()), applied);
}

#[test]
fn a_damaged_backup_is_reported_not_trusted() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    apply(&host, &t[1]);
    // Corrupt every stored blob.
    let blobs = host.backups_dir().join("blobs");
    for entry in fs::read_dir(&blobs).unwrap() {
        fs::write(entry.unwrap().path(), b"garbage").unwrap();
    }
    let before = home.user_files();
    assert!(matches!(engine::plan_undo(&host), Err(Error::Io(_))));
    assert_eq!(home.user_files(), before);

    // An unreadable backup file is listed as damaged, and the rest still work.
    fs::write(host.backups_dir().join("000002.json"), b"{not json").unwrap();
    let (good, bad) = engine::history(&host).unwrap();
    assert_eq!((good.len(), bad.len()), (1, 1));

    // A state record from a newer version makes this build refuse to act.
    fs::write(store::State::path(&host), b"{\"version\": 99}").unwrap();
    assert!(matches!(
        engine::status(&host),
        Err(Error::Store(store::StoreError::Newer(_)))
    ));
}

#[test]
fn a_backup_another_home_wrote_into_these_backups_is_not_undone() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let start = home.user_files();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);

    // A run with a home of its own but this home's data folder, as a
    // sandbox that kept XDG_DATA_HOME has: its backup lands among these.
    let other = Home::with_kitty(CONFS[1]);
    let mixed = Dirs {
        data: home.dirs.data.clone(),
        ..other.dirs.clone()
    };
    let other_settings = MemSettings::gnome();
    let other_host = Host {
        dirs: &mixed,
        ..other.host(&StdFs, &other_settings)
    };
    apply(&other_host, &t[1]);
    let stray = store::Manifest::path(&host, 2);
    assert!(stray.exists());

    // Undoing it would have put the other run's state record in place of
    // this home's: refused, with nothing changed.
    let before = (home.snapshot(), settings.snapshot());
    match engine::plan_undo(&host) {
        Err(Error::ForeignBackup { backup, names }) => {
            assert_eq!(backup, stray);
            assert!(names.starts_with(&other.dirs.home), "{names:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        engine::undo_last(&host),
        Err(Error::ForeignBackup { .. })
    ));
    assert_eq!((home.snapshot(), settings.snapshot()), before);

    // Moved out of the way, this home's own change is undone as ever.
    fs::remove_file(&stray).unwrap();
    undo(&host);
    assert_eq!(home.user_files(), start);
}

#[test]
fn a_newer_backup_that_cannot_be_read_stops_the_undo() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    apply(&host, &t[1]);
    let blobs = || -> Vec<_> {
        let mut names: Vec<_> = fs::read_dir(host.backups_dir().join("blobs"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let stored = blobs();

    // Passing over it would undo the first theme while the second is on.
    for broken in [&b"{not json"[..], b"{\"version\": 99}"] {
        let newest = store::Manifest::path(&host, 2);
        let kept = fs::read(&newest).unwrap();
        fs::write(&newest, broken).unwrap();
        let before = (home.snapshot(), settings.snapshot());
        match engine::plan_undo(&host) {
            Err(Error::UnreadableBackup(path)) => assert_eq!(path, newest),
            other => panic!("{other:?}"),
        }
        assert!(engine::undo_last(&host).is_err());
        assert!(engine::restore_to(&host, 1).is_err());
        assert_eq!((home.snapshot(), settings.snapshot()), before);

        // Nor does tidying up after an apply drop what it may need.
        apply(&host, &t[0]);
        assert!(stored.iter().all(|b| blobs().contains(b)));
        undo(&host);
        fs::write(&newest, kept).unwrap();
    }
    undo(&host);
    undo(&host);
    assert!(matches!(
        engine::plan_undo(&host),
        Err(Error::NothingToUndo)
    ));
}

#[test]
fn an_undo_preview_that_no_longer_holds_is_not_carried_out() {
    let t = themes();
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    apply(&host, &t[1]);

    // Another apply after the preview: the preview undoes the wrong one.
    let preview = engine::plan_undo(&host).unwrap();
    apply(&host, &t[0]);
    let before = (home.snapshot(), settings.snapshot());
    assert!(matches!(
        engine::execute(&host, &preview, &[]),
        Err(Error::Failed(Failure::Stale(_)))
    ));
    assert_eq!((home.snapshot(), settings.snapshot()), before);

    // Another undo after the preview: it would undo a second time.
    let preview = engine::plan_undo(&host).unwrap();
    undo(&host);
    let before = (home.snapshot(), settings.snapshot());
    assert!(matches!(
        engine::execute(&host, &preview, &[]),
        Err(Error::Failed(Failure::Stale(_)))
    ));
    assert_eq!((home.snapshot(), settings.snapshot()), before);

    // A fresh preview still works.
    undo(&host);
    assert_eq!(engine::plan_undo(&host).unwrap().undo_of, Some(1));

    // A preview with nothing left to write has no file to find changed:
    // carried out after another apply, it marked the wrong backup undone
    // and put back the state record from before it.
    let home = Home::with_kitty(CONFS[0]);
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let kitty = |theme: &Theme, replace_changed: bool| {
        let opts = ApplyOpts {
            only: vec![KITTY.to_owned()],
            replace_changed,
            ..ApplyOpts::default()
        };
        let resolved = Resolved::new(theme, None).unwrap();
        let plan = engine::plan_apply(&host, &no_env, &resolved, &opts).unwrap();
        engine::execute(&host, &plan, &yes()).unwrap();
    };
    kitty(&t[0], false);
    kitty(&t[1], false);
    fs::write(home.kitty().join("velatura.conf"), b"# mine\n").unwrap();
    let preview = engine::plan_undo(&host).unwrap();
    assert!(preview.targets.iter().all(|t| t.changes.is_empty()));
    kitty(&t[0], true);
    let before = (home.snapshot(), settings.snapshot());
    assert!(matches!(
        engine::execute(&host, &preview, &[]),
        Err(Error::Failed(Failure::Stale(_)))
    ));
    assert_eq!((home.snapshot(), settings.snapshot()), before);
    assert_eq!(engine::plan_undo(&host).unwrap().undo_of, Some(3));
}
