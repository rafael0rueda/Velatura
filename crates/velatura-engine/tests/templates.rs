//! Transaction tests for templates of the user's, added in phase 4c. Every
//! one runs in a throw-away home folder, with settings held in memory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::templates::{self, Invalid, Refused};
use velatura_theme::template::Problem;
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const FOOT: &str = "schema = 1\nname = \"Foot\"\noutput = \"~/.config/foot/colors.ini\"\ntemplate = '''\n[colors]\nbackground={{bg|bare}}\nforeground={{fg|bare}}\nregular1={{red|bare}}\n'''\n";
const OUTPUT: &str = ".config/foot/colors.ini";
const INSTANCE: &str = "template:foot";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-templates-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        for dir in [".local/state", ".local/share", ".config"] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        Self { root, dirs }
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
                if rel.starts_with(".local/share/velatura")
                    || rel.starts_with(".local/state/velatura")
                {
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

fn only_templates() -> ApplyOpts {
    ApplyOpts {
        only: vec!["template".to_owned()],
        ..ApplyOpts::default()
    }
}

fn plan_with(host: &Host<'_>, theme: &Theme, opts: &ApplyOpts) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, &no_env, &resolved, opts).unwrap()
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    let p = plan_with(host, theme, &only_templates());
    let agreed: Vec<String> = p
        .needing_consent()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    engine::execute(host, &p, &agreed).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

#[test]
fn a_template_applies_twice_and_undoes_to_the_exact_start() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    templates::add(&host, "foot", FOOT, false).unwrap();
    let start = home.snapshot();

    // It takes part without being turned on, and its first write is
    // agreed to: with no one there, nothing.
    let everything = plan_with(&host, &t[1], &ApplyOpts::default());
    assert!(target(&everything, INSTANCE).skipped.is_none());
    let p = plan_with(&host, &t[1], &only_templates());
    assert_eq!(p.needing_consent(), [INSTANCE]);
    let alone = engine::execute(&host, &p, &[]).unwrap();
    assert!(alone.changes.is_empty());
    assert!(
        alone
            .skipped
            .contains(&(INSTANCE.to_owned(), Skip::NoConsent))
    );
    assert_eq!(home.snapshot(), start);

    apply(&host, &t[1]);
    let ui = &Resolved::new(&t[1], None).unwrap().colors.ui;
    assert_eq!(
        fs::read_to_string(home.path(OUTPUT)).unwrap(),
        format!(
            "[colors]\nbackground={}\nforeground={}\nregular1={}\n",
            ui.bg.hex_noprefix(),
            ui.fg.hex_noprefix(),
            Resolved::new(&t[1], None).unwrap().colors.ansi[1].hex_noprefix()
        )
    );
    // The same theme again changes nothing; another rewrites the file,
    // and is not asked again.
    assert!(plan_with(&host, &t[1], &only_templates()).is_empty());
    let p = plan_with(&host, &t[0], &only_templates());
    assert!(p.needing_consent().is_empty());
    engine::execute(&host, &p, &[]).unwrap();
    assert!(
        fs::read_to_string(home.path(OUTPUT)).unwrap().contains(
            &Resolved::new(&t[0], None)
                .unwrap()
                .colors
                .ui
                .bg
                .hex_noprefix()
        )
    );
    undo(&host);
    undo(&host);
    assert_eq!(home.snapshot(), start);
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
            templates::add(&clean, "foot", FOOT, false).unwrap();
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
            }
            let before = home.snapshot();
            let p = plan_with(&clean, &t[1], &only_templates());
            let faults = Faults::new(&StdFs, &s, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &p, &[INSTANCE.to_owned()]).is_ok() {
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                home.snapshot(),
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
fn a_template_turned_off_or_removed_is_left_out_and_its_file_goes_with_undo() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    templates::add(&host, "foot", FOOT, false).unwrap();
    let start = home.snapshot();
    engine::set_included(&host, INSTANCE, false).unwrap();
    assert!(
        engine::excluded(&host)
            .unwrap()
            .contains(&INSTANCE.to_owned())
    );
    let p = plan_with(&host, &t[0], &ApplyOpts::default());
    assert_eq!(target(&p, INSTANCE).skipped, Some(Skip::NotSelected));
    engine::set_included(&host, INSTANCE, true).unwrap();
    apply(&host, &t[0]);

    // Removing the template keeps a copy, and leaves the file it wrote
    // for Undo or Remove Everything.
    let kept = templates::remove(&host, "foot").unwrap();
    assert_eq!(fs::read_to_string(&kept).unwrap(), FOOT);
    assert!(home.path(OUTPUT).exists());
    let p = plan_with(&host, &t[1], &ApplyOpts::default());
    assert!(!p.targets.iter().any(|t| t.instance == INSTANCE));
    let restore = engine::plan_restore_original(&host).unwrap();
    engine::execute(&host, &restore, &[]).unwrap();
    assert_eq!(home.snapshot(), start);
    assert_eq!(
        templates::remove(&host, "foot"),
        Err(Refused::NotFound("foot".to_owned()))
    );
}

#[test]
fn a_file_that_is_not_velaturas_is_never_written_over() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    fs::create_dir_all(home.path(".config/foot")).unwrap();
    fs::write(home.path(OUTPUT), "[colors]\nbackground=000000\n").unwrap();
    templates::add(&host, "foot", FOOT, false).unwrap();
    let p = plan_with(&host, &t[0], &only_templates());
    assert_eq!(
        target(&p, INSTANCE).skipped,
        Some(Skip::ForeignFile(home.path(OUTPUT)))
    );
}

#[test]
fn outputs_go_only_where_templates_may_write() {
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    let with_output = |output: &str| FOOT.replace("~/.config/foot/colors.ini", output);
    for (output, ok) in [
        ("~/.config/foot/colors.ini", true),
        ("~/.local/share/foot/colors.ini", true),
        ("~/.config/a/b/c/colors.conf", true),
        ("~/.bashrc", false),
        ("~/.config/../.bashrc", false),
        ("~/.config/./x", false),
        ("~/.config/", false),
        ("~/.config/foot/", false),
        ("/etc/foot.ini", false),
        ("~/.local/bin/foot", false),
        ("~/.config/autostart/x.desktop", false),
        ("~/.config/systemd/user/x.service", false),
        ("~/.config/environment.d/x.conf", false),
        ("~/.local/share/applications/x.desktop", false),
        ("~/.local/share/dbus-1/services/x.service", false),
        (
            "~/.local/share/gnome-shell/extensions/x/extension.js",
            false,
        ),
        ("~/.local/share/nautilus/scripts/x", false),
        ("~/.config/velatura/prefs.json", false),
        ("~/.config/kitty/velatura.conf", false),
        ("~/.config/Velatura-x/y", false),
        ("~/.config/foot/a\\nb", false),
    ] {
        let got = templates::read(&host, "foot", &with_output(output));
        assert_eq!(got.is_ok(), ok, "{output:?}: {got:?}");
        if !ok {
            assert!(
                matches!(
                    got,
                    Err(Invalid::OutputPlace(_) | Invalid::OutputForbidden(_))
                ),
                "{output:?}"
            );
        }
    }
    // The settings folder is where XDG_CONFIG_HOME says, under any name.
    assert_eq!(
        templates::output_path(&host, "~/.config/foot/colors.ini").unwrap(),
        host.dirs.config.join("foot/colors.ini")
    );
}

#[test]
fn templates_that_cannot_be_used_are_said_and_left_out() {
    let t = bundled::themes();
    let home = Home::new();
    let s = MemSettings::gnome();
    let host = home.host(&StdFs, &s);
    let folder = templates::dir(&host);
    store::ensure_dir(&host, &folder).unwrap();
    fs::write(
        folder.join("typo.toml"),
        FOOT.replace("{{fg|bare}}", "{{fgg}}")
            .replace("foot/", "typo/"),
    )
    .unwrap();
    fs::write(
        folder.join("newer.toml"),
        FOOT.replace("schema = 1", "schema = 2"),
    )
    .unwrap();
    fs::write(folder.join("broken.toml"), "output = ").unwrap();
    fs::write(folder.join("Not A Slug.toml"), FOOT).unwrap();
    fs::write(folder.join("a.toml"), FOOT).unwrap();
    fs::write(folder.join("b.toml"), FOOT).unwrap();
    let p = plan_with(&host, &t[0], &ApplyOpts::default());
    let why = |id: &str| target(&p, &format!("template:{id}")).skipped.clone();
    assert_eq!(
        why("typo"),
        Some(Skip::TemplateInvalid(Invalid::Template(
            Problem::UnknownName {
                line: 3,
                name: "fgg".to_owned()
            }
        )))
    );
    assert_eq!(
        why("newer"),
        Some(Skip::TemplateInvalid(Invalid::NewerSchema))
    );
    assert!(matches!(
        why("broken"),
        Some(Skip::TemplateInvalid(Invalid::NotToml(_)))
    ));
    assert!(why("a").is_none());
    // Two that write one file: the second one is refused.
    assert!(matches!(
        why("b"),
        Some(Skip::TemplateInvalid(Invalid::OutputTaken(_)))
    ));
    assert!(!p.targets.iter().any(|t| t.instance.contains("Not A Slug")));

    // Adding checks the same, and keeps out a second writer of one file.
    assert!(matches!(
        templates::add(&host, "c", FOOT, false),
        Err(Refused::Invalid(Invalid::OutputTaken(_)))
    ));
    assert_eq!(
        templates::add(&host, "a", FOOT, false),
        Err(Refused::Exists("a".to_owned()))
    );
    assert!(templates::add(&host, "a", FOOT, true).is_ok());
    assert_eq!(
        templates::add(&host, "../x", FOOT, false),
        Err(Refused::BadId("../x".to_owned()))
    );
}
