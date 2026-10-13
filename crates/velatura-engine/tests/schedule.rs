//! The schedule: the timer's files, the switch with no one present, and
//! the single backup its switches share. Every test runs in a throw-away
//! home folder with a systemd that only records what it is asked.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, MemSystemd, Settings, StdFs};
use velatura_engine::schedule::{
    self, Auto, Period, Problem, Refused, SERVICE, TIMER, Times, format_time, parse_time,
};
use velatura_engine::store::{self, DayNight};
use velatura_engine::targets::{GHOSTTY, KITTY, SHELL, USER_THEMES};
use velatura_theme::{Resolved, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const SHELL_SCHEMA: &str = "org.gnome.shell.extensions.user-theme";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    /// Kitty, Ghostty, GTK 4 and a per-user User Themes: every kind of
    /// change a switch can make.
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-schedule-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        let h = Self { root, dirs };
        fs::create_dir_all(h.path(".local/state")).unwrap();
        fs::create_dir_all(h.path(&format!(
            ".local/share/gnome-shell/extensions/{USER_THEMES}"
        )))
        .unwrap();
        h.write(".config/kitty/kitty.conf", "font_size 12\n");
        h.write(".config/ghostty/config", "font-size = 12\n");
        h.write(".config/gtk-4.0/gtk.css", "/* mine */\n");
        // The command the timer starts, installed where it will stay.
        h.write(".local/bin/velatura", "#!/bin/sh\n");
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

    fn exe(&self) -> PathBuf {
        self.path(".local/bin/velatura")
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings, now: u64) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now,
        }
    }

    /// Every file and folder outside Velatura's own records.
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

fn no_env(key: &str) -> Option<OsString> {
    (key == "XDG_DATA_DIRS").then(|| OsString::from("/nonexistent-velatura-test"))
}

fn settings() -> MemSettings {
    MemSettings::new(&[
        ("org.gnome.desktop.interface", "color-scheme", "default"),
        ("org.gnome.desktop.interface", "accent-color", "blue"),
        ("org.gnome.desktop.interface", "gtk-theme", "Adwaita"),
        ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
        (SHELL_SCHEMA, "name", ""),
    ])
}

fn choose(host: &Host<'_>) {
    DayNight {
        day: Some("ember".to_owned()),
        night: Some("aetherius".to_owned()),
    }
    .save(host)
    .unwrap();
}

/// A user's own apply of a bundled theme, agreeing to every added line.
fn apply(host: &Host<'_>, index: usize) {
    let themes = bundled::themes();
    let resolved = Resolved::new(&themes[index], None).unwrap();
    let plan = engine::plan_apply(host, &no_env, &resolved, &ApplyOpts::default()).unwrap();
    let everyone: Vec<String> = plan
        .needing_consent()
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    engine::execute(host, &plan, &everyone).unwrap();
}

/// Undo the way the app does: the user's last change, and with it
/// whatever the schedule did after.
fn undo(host: &Host<'_>) {
    engine::undo_last(host).unwrap();
}

const DAY: u64 = 86_400;
/// Midnight UTC of some day; tests treat UTC as the local time.
const T0: u64 = 1_800_000_000 / DAY * DAY;

fn at(day: u64, hour: u64, minute: u64) -> (u64, u16) {
    (
        T0 + day * DAY + hour * 3600 + minute * 60,
        (hour * 60 + minute) as u16,
    )
}

#[test]
fn times_are_read_and_written_as_clock_times() {
    assert_eq!(parse_time("07:00"), Some(420));
    assert_eq!(parse_time(" 7:05 "), Some(425));
    assert_eq!(parse_time("23:59"), Some(1439));
    for bad in [
        "", "7", "24:00", "07:60", "7:5", "007:00", "-1:00", "07:00:00", "a:bc", "07.00",
    ] {
        assert_eq!(parse_time(bad), None, "{bad:?}");
    }
    assert_eq!(format_time(425), "07:05");

    let day_first = Times {
        day: 420,
        night: 1200,
    };
    assert_eq!(day_first.period(419), Period::Night);
    assert_eq!(day_first.period(420), Period::Day);
    assert_eq!(day_first.period(1199), Period::Day);
    assert_eq!(day_first.period(1200), Period::Night);
    assert_eq!(day_first.period(0), Period::Night);
    // Someone who works nights: the daytime theme starts in the evening.
    let night_first = Times {
        day: 1320,
        night: 360,
    };
    assert_eq!(night_first.period(0), Period::Day);
    assert_eq!(night_first.period(359), Period::Day);
    assert_eq!(night_first.period(360), Period::Night);
    assert_eq!(night_first.period(1320), Period::Day);

    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    assert_eq!(Times::load(&host), Times::default());
    choose(&host);
    Times {
        day: 390,
        night: 1290,
    }
    .save(&host)
    .unwrap();
    assert_eq!(
        fs::read_to_string(home.path(".config/velatura/config.toml")).unwrap(),
        "day_at = \"06:30\"\nnight_at = \"21:30\"\nday = \"ember\"\nnight = \"aetherius\"\n"
    );
    assert_eq!(
        Times::load(&host),
        Times {
            day: 390,
            night: 1290
        }
    );
    assert_eq!(DayNight::load(&host).day.as_deref(), Some("ember"));
    // Nonsense in the file is the default, not an error.
    home.write(
        ".config/velatura/config.toml",
        "day_at = \"25:00\"\nnight_at = 7\n",
    );
    assert_eq!(Times::load(&host), Times::default());
}

#[test]
fn the_unit_files_hold_the_command_and_the_times_and_nothing_else() {
    let (service, timer) = schedule::units(
        Path::new("/home/u/.local/bin/velatura"),
        Times {
            day: 425,
            night: 1200,
        },
    )
    .unwrap();
    assert!(service.contains("ExecStart=\"/home/u/.local/bin/velatura\" switch auto\n"));
    assert!(service.contains("SuccessExitStatus=5\n"));
    assert!(timer.contains("OnCalendar=*-*-* 07:05:00\nOnCalendar=*-*-* 20:00:00\n"));
    assert!(timer.contains("Persistent=true\n") && timer.contains("WantedBy=timers.target\n"));

    // A path systemd would read something into is quoted so it does not.
    let (service, _) = schedule::units(
        Path::new("/home/a b/50% \"off\"/$x\\y/velatura"),
        Times::default(),
    )
    .unwrap();
    assert!(
        service
            .contains("ExecStart=\"/home/a b/50%% \\\"off\\\"/$$x\\\\y/velatura\" switch auto\n"),
        "{service}"
    );
    for bad in ["relative/velatura", "/home/u/line\nbreak/velatura"] {
        assert!(
            schedule::units(Path::new(bad), Times::default()).is_none(),
            "{bad:?}"
        );
    }
}

/// systemd itself must accept what is written, where it is there to ask.
#[test]
fn systemd_accepts_the_unit_files() {
    let have = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join("systemd-analyze").is_file())
    });
    if !have {
        eprintln!("SKIPPED: systemd-analyze is not installed");
        return;
    }
    let home = Home::new();
    let (service, timer) = schedule::units(Path::new("/usr/bin/true"), Times::default()).unwrap();
    home.write("units/velatura-switch.service", &service);
    home.write("units/velatura-switch.timer", &timer);
    let out = std::process::Command::new("systemd-analyze")
        .args(["verify", "--user"])
        .arg(home.path("units/velatura-switch.timer"))
        .env("SYSTEMD_UNIT_PATH", home.path("units"))
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    // It also comments on units of the machine; only ours count.
    let ours: Vec<&str> = said
        .lines()
        .filter(|l| l.contains("velatura-switch"))
        .collect();
    assert!(ours.is_empty(), "{said}");
    if !out.status.success() {
        eprintln!("INCONCLUSIVE: systemd-analyze failed for another reason: {said}");
    }
}

#[test]
fn turning_the_schedule_on_and_off() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    let sd = MemSystemd::default();
    let units = home.path(".config/systemd/user");
    let before = home.user_files();

    // Not before both themes are chosen.
    assert!(matches!(
        schedule::enable(&host, &sd, &home.exe(), Times::default(), false),
        Err(Refused::NoThemes)
    ));
    choose(&host);
    let chosen = home.user_files();
    // Not from a build folder, which the next build or clean takes away.
    for gone in ["target/debug/velatura", ".local/bin/missing"] {
        assert!(matches!(
            schedule::enable(&host, &sd, &home.path(gone), Times::default(), false),
            Err(Refused::NotInstalled(_))
        ));
    }
    home.write("build/.cargo-lock", "");
    home.write("build/velatura", "");
    assert!(!schedule::lasting(&host, &home.path("build/velatura")));
    fs::remove_dir_all(home.path("build")).unwrap();
    assert!(matches!(
        schedule::enable(&host, &sd, &home.exe(), Times { day: 60, night: 60 }, false),
        Err(Refused::SameTime)
    ));
    assert!(sd.calls.borrow().is_empty() && !units.exists());
    assert_eq!(home.user_files(), chosen);
    assert!(!schedule::status(&host).on);

    schedule::enable(
        &host,
        &sd,
        &home.exe(),
        Times {
            day: 390,
            night: 1290,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        *sd.calls.borrow(),
        [
            "daemon-reload",
            "enable velatura-switch.timer",
            "restart velatura-switch.timer"
        ]
    );
    let timer = fs::read_to_string(units.join(TIMER)).unwrap();
    assert!(timer.contains("OnCalendar=*-*-* 06:30:00"), "{timer}");
    assert!(
        fs::read_to_string(units.join(SERVICE))
            .unwrap()
            .contains("switch auto")
    );
    let status = schedule::status(&host);
    assert!(status.on && !status.damaged);
    assert_eq!(
        status.times,
        Times {
            day: 390,
            night: 1290
        }
    );

    // New times replace the files and restart the timer.
    schedule::enable(&host, &sd, &home.exe(), Times::default(), false).unwrap();
    assert!(
        fs::read_to_string(units.join(TIMER))
            .unwrap()
            .contains("07:00:00")
    );
    assert_eq!(sd.calls.borrow().len(), 6);
    assert!(!schedule::status(&host).damaged);

    // Changed by hand: reported, and left in place when turning off.
    fs::write(units.join(TIMER), "# mine now\n").unwrap();
    assert!(schedule::status(&host).damaged);
    assert!(matches!(
        schedule::enable(&host, &sd, &home.exe(), Times::default(), false),
        Err(Refused::Foreign(_))
    ));
    let kept = schedule::disable(&host, &sd).unwrap().unwrap();
    assert_eq!(kept, [units.join(TIMER)]);
    assert!(!units.join(SERVICE).exists() && units.join(TIMER).exists());
    assert!(!schedule::status(&host).on);
    assert_eq!(sd.calls.borrow()[6], "disable --now velatura-switch.timer");
    fs::remove_file(units.join(TIMER)).unwrap();

    fs::remove_dir_all(home.path(".config/systemd")).unwrap();

    // On and off again leaves nothing behind, not even the folders.
    schedule::enable(&host, &sd, &home.exe(), Times::default(), false).unwrap();
    assert_eq!(schedule::disable(&host, &sd).unwrap(), Some(Vec::new()));
    assert!(!home.path(".config/systemd").exists());
    assert_eq!(schedule::disable(&host, &sd).unwrap(), None);
    let mut after = home.user_files();
    after.remove(".config/velatura/config.toml");
    assert_eq!(after, before);
}

#[test]
fn a_schedule_systemd_refuses_leaves_nothing_behind() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    let before = home.user_files();
    for step in ["daemon-reload", "enable", "restart"] {
        let sd = MemSystemd::default();
        sd.fail.set(Some(step));
        assert!(matches!(
            schedule::enable(&host, &sd, &home.exe(), Times::default(), false),
            Err(Refused::Systemd(_))
        ));
        assert!(!schedule::status(&host).on, "{step}");
        let mut after = home.user_files();
        after.remove(".config/velatura/config.toml");
        let mut want = before.clone();
        want.remove(".config/velatura/config.toml");
        assert_eq!(after, want, "{step}");
    }
    // A timer that cannot be turned off stays on, files and all.
    let sd = MemSystemd::default();
    schedule::enable(&host, &sd, &home.exe(), Times::default(), false).unwrap();
    sd.fail.set(Some("disable"));
    assert!(matches!(
        schedule::disable(&host, &sd),
        Err(Refused::Systemd(_))
    ));
    assert!(schedule::status(&host).on && !schedule::status(&host).damaged);
}

#[test]
fn another_theme_timer_or_someone_elses_file_stops_it() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    let sd = MemSystemd::default();
    choose(&host);

    home.write(
        ".config/systemd/user/timers.target.wants/theme-auto.timer",
        "",
    );
    match schedule::enable(&host, &sd, &home.exe(), Times::default(), false) {
        Err(Refused::OtherTimers(units)) => assert_eq!(units, ["theme-auto.timer"]),
        other => panic!("{other:?}"),
    }
    assert!(sd.calls.borrow().is_empty());
    schedule::enable(&host, &sd, &home.exe(), Times::default(), true).unwrap();
    schedule::disable(&host, &sd).unwrap();
    // The folders were there before and are not Velatura's to remove.
    assert!(
        home.path(".config/systemd/user/timers.target.wants")
            .exists()
    );
    fs::remove_dir_all(home.path(".config/systemd")).unwrap();

    home.write(
        ".config/systemd/user/velatura-switch.service",
        "[Unit]\n# not ours\n",
    );
    assert!(matches!(
        schedule::enable(&host, &sd, &home.exe(), Times::default(), false),
        Err(Refused::Foreign(_))
    ));
    assert_eq!(
        fs::read_to_string(home.path(".config/systemd/user/velatura-switch.service")).unwrap(),
        "[Unit]\n# not ours\n"
    );
    assert!(
        !home
            .path(".config/systemd/user/velatura-switch.timer")
            .exists()
    );
}

fn auto(home: &Home, s: &MemSettings, (now, minute): (u64, u16)) -> Auto {
    schedule::run_auto(&home.host(&StdFs, s, now), &no_env, minute, false).unwrap()
}

fn theme_of(a: &Auto) -> Option<&str> {
    match a {
        Auto::Done { theme, .. } => Some(theme),
        _ => None,
    }
}

#[test]
fn the_switch_acts_once_per_period_and_makes_up_for_a_missed_one() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    apply(&host, 0); // Aetherius by hand, agreeing to the added lines.
    let applied = || engine::status(&host).unwrap().applied.unwrap().0;

    // First start, mid-morning: the daytime theme.
    assert_eq!(
        theme_of(&auto(&home, &s, at(0, 9, 30))),
        Some("ember/default")
    );
    assert_eq!(applied(), "ember/default");
    // Later the same day nothing happens, even after the user picks
    // another theme: that choice stands until the evening.
    apply(&host, 0);
    assert!(matches!(
        auto(&home, &s, at(0, 15, 0)),
        Auto::NotDue(Period::Day)
    ));
    assert_eq!(applied(), "aetherius/default");
    // Unless asked to act again.
    let again = schedule::run_auto(&home.host(&StdFs, &s, at(0, 15, 1).0), &no_env, 901, true);
    assert_eq!(theme_of(&again.unwrap()), Some("ember/default"));

    assert_eq!(
        theme_of(&auto(&home, &s, at(0, 20, 0))),
        Some("aetherius/default")
    );
    assert!(matches!(
        auto(&home, &s, at(0, 23, 0)),
        Auto::NotDue(Period::Night)
    ));
    assert!(matches!(
        auto(&home, &s, at(1, 3, 0)),
        Auto::NotDue(Period::Night)
    ));
    assert_eq!(
        theme_of(&auto(&home, &s, at(1, 7, 0))),
        Some("ember/default")
    );

    // Asleep through the evening and the next morning: the start after
    // that is in the same period as the last one seen to, a day later.
    assert_eq!(
        theme_of(&auto(&home, &s, at(1, 20, 0))),
        Some("aetherius/default")
    );
    apply(&host, 1);
    assert_eq!(
        theme_of(&auto(&home, &s, at(3, 1, 0))),
        Some("aetherius/default")
    );
    // And a clock that moved by an hour does not make it act twice.
    assert!(matches!(
        auto(&home, &s, at(3, 2, 0)),
        Auto::NotDue(Period::Night)
    ));
}

#[test]
fn with_no_one_present_nothing_is_asked_removed_or_forced() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    let conf = home.path(".config/kitty/kitty.conf");

    // Nothing was ever agreed to: the three files of the user's stay as
    // they are, the rest is themed, and what was left out is on record.
    let done = auto(&home, &s, at(0, 9, 0));
    let Auto::Done { report, .. } = &done else {
        panic!("{done:?}")
    };
    assert_eq!(fs::read_to_string(&conf).unwrap(), "font_size 12\n");
    assert!(!home.path(".config/kitty/velatura.conf").exists());
    assert_eq!(
        report
            .skipped
            .iter()
            .filter(|(_, w)| *w == engine_skip())
            .count(),
        3
    );
    let last = schedule::status(&host).last.unwrap();
    assert_eq!(last.left_out, [KITTY, GHOSTTY, "gtk4:default"]);
    assert_eq!((last.period, last.problem), (Period::Day, None));
    assert_eq!(last.theme.as_deref(), Some("ember/default"));
    let shell = |name: &str| home.path(&format!(".local/share/themes/{name}"));
    let first = s.read(SHELL_SCHEMA, "name").unwrap().unwrap().effective;
    assert!(shell(&first).join("gnome-shell/gnome-shell.css").exists());

    // The user applies by hand and agrees: the record is cleared.
    apply(&host, 1);
    assert!(schedule::status(&host).last.unwrap().left_out.is_empty());

    // The evening switch keeps the folder of the theme it replaces: a run
    // with no one present removes nothing.
    auto(&home, &s, at(0, 20, 0));
    let second = s.read(SHELL_SCHEMA, "name").unwrap().unwrap().effective;
    assert_ne!(first, second);
    assert!(shell(&first).exists() && shell(&second).exists());
    assert!(
        fs::read_to_string(&conf)
            .unwrap()
            .contains("include velatura.conf")
    );
    // The next apply by the user clears it away.
    apply(&host, 0);
    apply(&host, 1);
    assert!(!shell(&second).exists());

    // No theme chosen for the night: said once, and nothing changes.
    DayNight {
        day: Some("ember".to_owned()),
        night: None,
    }
    .save(&host)
    .unwrap();
    let before = home.user_files();
    assert!(matches!(
        auto(&home, &s, at(1, 21, 0)),
        Auto::NoTheme(Period::Night)
    ));
    assert_eq!(
        schedule::status(&host).last.unwrap().problem,
        Some(Problem::NoTheme)
    );
    assert!(matches!(auto(&home, &s, at(1, 22, 0)), Auto::NotDue(_)));
    assert_eq!(home.user_files(), before);
}

fn engine_skip() -> velatura_engine::plan::Skip {
    velatura_engine::plan::Skip::NoConsent
}

/// Days of switching are one backup, and Undo goes back past all of it to
/// before the change the user made.
#[test]
fn scheduled_switches_share_one_backup_and_undo_passes_through_them() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    let start = (home.user_files(), s.snapshot());

    apply(&host, 0);
    for day in 0..4 {
        auto(&home, &s, at(day, 7, 0));
        auto(&home, &s, at(day, 20, 0));
    }
    // However many days pass, the schedule holds one backup.
    let (history, _) = engine::history(&host).unwrap();
    assert_eq!(history.len(), 2, "{history:?}");
    assert_eq!(history[0].cause, "scheduled");

    auto(&home, &s, at(4, 7, 0));
    let (history, _) = engine::history(&host).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].theme.as_deref(), Some("ember/default"));
    assert_eq!(history[0].time, at(4, 7, 0).0);

    // One Undo: the schedule's switch and the apply before it.
    undo(&host);
    assert_eq!((home.user_files(), s.snapshot()), start);
    assert!(engine::status(&host).unwrap().applied.is_none());

    // A switch that only cancels the one before leaves no backup at all.
    // (Without the top bar, whose earlier folder a scheduled run keeps.)
    let (home, s) = (Home::new(), settings());
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    engine::set_included(&host, SHELL, false).unwrap();
    apply(&host, 0);
    apply(&host, 1);
    apply(&host, 0);
    let by_hand = (home.user_files(), s.snapshot());
    auto(&home, &s, at(0, 7, 0));
    assert_eq!(engine::history(&host).unwrap().0.len(), 4);
    auto(&home, &s, at(0, 20, 0));
    let (history, _) = engine::history(&host).unwrap();
    assert_eq!(history.len(), 3, "{history:?}");
    assert!(history.iter().all(|m| m.cause == "apply"));
    assert_eq!((home.user_files(), s.snapshot()), by_hand);
    assert_eq!(
        engine::status(&host).unwrap().applied.unwrap().0,
        "aetherius/default"
    );
}

/// The user changes things between two scheduled switches; undo gives back
/// exactly what they left, not what an earlier switch had found.
#[test]
fn what_the_user_changed_between_two_switches_survives_undo() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    apply(&host, 0);
    let conf = home.path(".config/kitty/kitty.conf");

    auto(&home, &s, at(0, 7, 0));
    // An edit to the settings file, and an accent picked in GNOME Settings.
    let edited = format!("{}# added by hand\n", fs::read_to_string(&conf).unwrap());
    fs::write(&conf, &edited).unwrap();
    s.write("org.gnome.desktop.interface", "accent-color", Some("teal"))
        .unwrap();
    auto(&home, &s, at(0, 20, 0));
    assert_eq!(engine::history(&host).unwrap().0.len(), 2);

    let plan = engine::plan_undo(&host).unwrap();
    assert!(plan.scheduled);
    engine::execute(&host, &plan, &[]).unwrap();
    assert_eq!(fs::read_to_string(&conf).unwrap(), edited);
    assert_eq!(
        s.read("org.gnome.desktop.interface", "accent-color")
            .unwrap()
            .unwrap()
            .user
            .as_deref(),
        Some("teal")
    );
}

/// Stop a scheduled switch at every step, on top of an earlier one, then
/// recover: the state and the shared backup are as before, and undo still
/// reaches the start.
#[test]
fn exhaustive_crash_test_for_a_scheduled_switch() {
    let mut crash_points = 0;
    for fail_at in 0.. {
        let home = Home::new();
        let s = settings();
        let clean = home.host(&StdFs, &s, T0);
        choose(&clean);
        let start = (home.user_files(), s.snapshot());
        apply(&clean, 0);
        auto(&home, &s, at(0, 7, 0));
        let before = (home.user_files(), s.snapshot());
        let backups = |host: &Host<'_>| engine::history(host).unwrap().0;
        let history = backups(&clean);

        let faults = Faults::new(&StdFs, &s, fail_at);
        let (now, minute) = at(0, 20, 0);
        let dying = home.host(&faults, &faults, now);
        let result = schedule::run_auto(&dying, &no_env, minute, false);
        if result.is_ok() {
            assert_ne!((home.user_files(), s.snapshot()), before);
            undo(&clean);
            assert_eq!((home.user_files(), s.snapshot()), start);
            break;
        }
        crash_points += 1;
        if let Some(r) = engine::recover(&clean).unwrap() {
            assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
        }
        assert_eq!(
            (home.user_files(), s.snapshot()),
            before,
            "state differs after a crash at step {fail_at}"
        );
        assert_eq!(backups(&clean), history, "step {fail_at}");
        // Not seen to, so the next start does it; and undo reaches the start.
        assert_eq!(
            theme_of(&auto(&home, &s, at(0, 20, 5))),
            Some("aetherius/default")
        );
        undo(&clean);
        assert_eq!((home.user_files(), s.snapshot()), start, "step {fail_at}");
    }
    assert!(
        crash_points >= 5,
        "only {crash_points} crash points exercised"
    );
}

#[test]
fn scheduled_backups_do_not_push_the_users_out() {
    let home = Home::new();
    let s = settings();
    let host = home.host(&StdFs, &s, T0);
    choose(&host);
    // A scheduled switch after every change the user makes.
    for i in 0..(store::KEEP as u64 + 4) {
        apply(&host, (i % 2) as usize);
        let hour = if i % 2 == 0 { 7 } else { 20 };
        assert!(theme_of(&auto(&home, &s, at(i, hour, 0))).is_some());
    }
    let (kept, damaged) = engine::history(&host).unwrap();
    assert!(damaged.is_empty());
    let by_user = kept.iter().filter(|m| m.cause == "apply").count();
    assert_eq!(by_user, store::KEEP);
    assert!(kept.len() > store::KEEP, "{}", kept.len());
    // What is kept is one unbroken run, so undoing through it works.
    for pair in kept.windows(2) {
        assert_eq!(pair[0].id, pair[1].id + 1);
    }
    for _ in 0..store::KEEP {
        undo(&host);
    }
    assert!(matches!(
        engine::plan_undo(&host),
        Err(engine::Error::NothingToUndo)
    ));
}
