//! Switching between the daytime and the nighttime theme by the clock.
//!
//! One systemd user timer starts `velatura switch auto` at the two times.
//! That command works out which theme belongs to the present time, so a
//! missed start (the machine was asleep or off) corrects itself at the
//! next one. No process stays running in between.
//!
//! The timer is set up outside the undo history: Undo reverses changes of
//! theme, and turning the schedule off is its own reverse.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use velatura_theme::{Appearance, Resolved, sha256_hex};

use crate::engine::{self, ApplyOpts, Error, Report};
use crate::host::{Host, Kind, Systemd, acquire_lock};
use crate::library;
use crate::plan::Skip;
use crate::store::{
    DayNight, VERSION, ensure_dir, load_config, path_str, read_opt, save_config, to_bytes,
};
use crate::targets::{self, Env, OtherTool};

pub const TIMER: &str = "velatura-switch.timer";
pub const SERVICE: &str = "velatura-switch.service";
/// The unit that starts the service when the desktop's settings change,
/// for following the dark style.
pub const WATCH: &str = "velatura-follow.path";

/// When each of the two themes starts, in minutes after midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub day: u16,
    pub night: u16,
}

impl Default for Times {
    fn default() -> Self {
        Self {
            day: 7 * 60,
            night: 20 * 60,
        }
    }
}

/// `HH:MM` on a 24-hour clock, as minutes after midnight.
pub fn parse_time(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    let digits = |s: &str, max: usize| {
        (!s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())).then_some(())
    };
    digits(h, 2)?;
    digits(m, 2).filter(|()| m.len() == 2)?;
    let (h, m) = (h.parse::<u16>().ok()?, m.parse::<u16>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_time(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}

impl Times {
    /// From the config file; a missing or unusable entry is the default.
    pub fn load(host: &Host<'_>) -> Self {
        let table = load_config(host);
        let get = |key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .and_then(parse_time)
        };
        let d = Self::default();
        let times = Self {
            day: get("day_at").unwrap_or(d.day),
            night: get("night_at").unwrap_or(d.night),
        };
        if times.day == times.night { d } else { times }
    }

    pub fn save(&self, host: &Host<'_>) -> std::io::Result<()> {
        save_config(
            host,
            &[
                ("day_at", Some(format_time(self.day))),
                ("night_at", Some(format_time(self.night))),
            ],
        )
    }

    /// Which theme belongs to `minute` of the day.
    pub fn period(&self, minute: u16) -> Period {
        let in_day = if self.day < self.night {
            (self.day..self.night).contains(&minute)
        } else {
            !(self.night..self.day).contains(&minute)
        };
        if in_day { Period::Day } else { Period::Night }
    }

    /// Minutes since the period that holds `minute` began.
    fn since_start(&self, minute: u16) -> u16 {
        let start = match self.period(minute) {
            Period::Day => self.day,
            Period::Night => self.night,
        };
        (minute + 1440 - start) % 1440
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Night,
}

impl Period {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Night => "night",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "day" => Some(Self::Day),
            "night" => Some(Self::Night),
            _ => None,
        }
    }
}

// ------------------------------------------------------------ unit files

fn unit_dir(host: &Host<'_>) -> PathBuf {
    host.dirs.config.join("systemd").join("user")
}

/// `exe` as one word of a systemd command line, or `None` if it holds
/// something that cannot be written there.
fn quoted(exe: &Path) -> Option<String> {
    let text = exe.to_str()?;
    if !exe.is_absolute() || text.chars().any(char::is_control) {
        return None;
    }
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // Specifiers and variables are expanded by systemd unless doubled.
            '%' | '$' => {
                out.push(c);
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

/// The service and the timer, as text. Nothing in them but the path of
/// the command and the two times.
pub fn units(exe: &Path, times: Times) -> Option<(String, String)> {
    let service = format!(
        "# Written by Velatura. To remove it: velatura schedule off\n\
         [Unit]\n\
         Description=Velatura: apply the daytime or the nighttime theme\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={} switch auto\n\
         # 5: done, with something left out for the user to look at.\n\
         SuccessExitStatus=5\n",
        quoted(exe)?
    );
    let timer = format!(
        "# Written by Velatura. To remove it: velatura schedule off\n\
         [Unit]\n\
         Description=Velatura: change theme at {day} and {night}\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* {day}:00\n\
         OnCalendar=*-*-* {night}:00\n\
         # A time missed while signed out is made up for at the next sign-in.\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        day = format_time(times.day),
        night = format_time(times.night),
    );
    Some((service, timer))
}

/// The service and the unit that watches the desktop's settings, for
/// following the dark style. `settings` is the settings database file.
pub fn follow_units(exe: &Path, settings: &Path) -> Option<(String, String)> {
    let (service, _) = units(exe, Times::default())?;
    // systemd reads a path here as it is, but for `%`: no quotes and no
    // escapes. It drops white space at the end of a line and joins a line
    // that ends in a backslash to the next, so a path ending in either is
    // refused.
    let watched = settings.to_str().filter(|text| {
        settings.is_absolute()
            && !text.chars().any(char::is_control)
            && !text.ends_with(char::is_whitespace)
            && !text.ends_with('\\')
    })?;
    // The settings file changes in bursts, and systemd by default refuses
    // to start a service more than five times in ten seconds, after which
    // the watching unit fails and stays off. The command is cheap and does
    // nothing unless the style changed, so the limit is lifted.
    let service = service.replacen("[Unit]\n", "[Unit]\nStartLimitIntervalSec=0\n", 1);
    // Signed in, the service runs once, so the theme matches from the start.
    let service = format!("{service}\n[Install]\nWantedBy=default.target\n");
    let watch = format!(
        "# Written by Velatura. To remove it: velatura schedule off\n\
         [Unit]\n\
         Description=Velatura: watch for a change between the light and the dark style\n\
         \n\
         [Path]\n\
         # Every change of a desktop setting rewrites this file. The command\n\
         # it starts does nothing unless the style is not the one it last saw.\n\
         PathChanged={}\n\
         Unit={SERVICE}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        // The one thing systemd reads something into in this path.
        watched.replace('%', "%%")
    );
    Some((service, watch))
}

/// True if a timer can count on finding the command at `exe` later: not a
/// copy in a build folder.
pub fn lasting(host: &Host<'_>, exe: &Path) -> bool {
    let build = exe.components().any(|c| c.as_os_str() == "target")
        || exe.parent().is_some_and(|dir| {
            matches!(host.fs.kind(&dir.join(".cargo-lock")), Ok(Some(_)))
                || matches!(host.fs.kind(&dir.join("deps")), Ok(Some(Kind::Dir)))
        });
    exe.is_absolute() && !build && matches!(host.fs.kind(exe), Ok(Some(_)))
}

// ---------------------------------------------------------------- record

/// What was set up, kept with the machine's facts so the files can be told
/// from ones somebody else wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Setup {
    exe: String,
    service: String,
    /// The hash of the unit that starts the service: the timer, or with
    /// `follow` the unit that watches the settings.
    timer: String,
    /// True when the themes follow the dark style and not the clock.
    follow: bool,
    /// Folders made for the unit files, removed again if still empty.
    dirs: Vec<String>,
}

fn setup_path(host: &Host<'_>) -> PathBuf {
    host.state_dir().join("schedule.json")
}

fn read_record(host: &Host<'_>, path: &Path) -> Option<Value> {
    let bytes = read_opt(host, path).ok().flatten()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    (v.get("version")?.as_u64()? <= VERSION).then_some(v)
}

impl Setup {
    /// The unit that starts the service.
    fn trigger(&self) -> &'static str {
        if self.follow { WATCH } else { TIMER }
    }

    fn load(host: &Host<'_>) -> Option<Self> {
        let v = read_record(host, &setup_path(host))?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            exe: text("exe")?,
            service: text("service")?,
            timer: text("timer")?,
            follow: v.get("follow").and_then(Value::as_bool).unwrap_or(false),
            dirs: v
                .get("dirs")?
                .as_array()?
                .iter()
                .filter_map(|d| d.as_str().map(str::to_owned))
                .collect(),
        })
    }

    fn save(&self, host: &Host<'_>) -> std::io::Result<()> {
        ensure_dir(host, &host.state_dir())?;
        host.fs.write_atomic(
            &setup_path(host),
            &to_bytes(&json!({
                "version": VERSION, "exe": self.exe,
                "service": self.service, "timer": self.timer,
                "follow": self.follow, "dirs": self.dirs,
            })),
            0o600,
        )
    }
}

/// What the last run of the schedule did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastRun {
    pub at: u64,
    pub period: Period,
    /// `theme/variant` that was applied, if one was.
    pub theme: Option<String>,
    /// Apps and desktop parts it could not theme without the user.
    pub left_out: Vec<String>,
    pub problem: Option<Problem>,
}

/// Why a scheduled switch did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No theme is chosen for that part of the day, or it is gone.
    NoTheme,
    Failed,
}

fn auto_path(host: &Host<'_>) -> PathBuf {
    host.state_dir().join("auto.json")
}

/// The start of the period the schedule last acted on, and its last run.
fn load_auto(host: &Host<'_>) -> (Option<(Period, u64)>, Option<LastRun>) {
    let Some(v) = read_record(host, &auto_path(host)) else {
        return (None, None);
    };
    let period = |v: &Value| {
        v.get("period")
            .and_then(Value::as_str)
            .and_then(Period::parse)
    };
    let at = |v: &Value| v.get("at").and_then(Value::as_u64);
    let handled = v.get("handled").and_then(|h| period(h).zip(at(h)));
    let last = v.get("last").and_then(|l| {
        Some(LastRun {
            at: at(l)?,
            period: period(l)?,
            theme: l.get("theme").and_then(Value::as_str).map(str::to_owned),
            left_out: l
                .get("left_out")?
                .as_array()?
                .iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect(),
            problem: match l.get("problem").and_then(Value::as_str) {
                Some("no-theme") => Some(Problem::NoTheme),
                Some(_) => Some(Problem::Failed),
                None => None,
            },
        })
    });
    (handled, last)
}

fn save_auto(host: &Host<'_>, handled: Option<(Period, u64)>, last: Option<&LastRun>) {
    let value = json!({
        "version": VERSION,
        "handled": handled.map(|(p, at)| json!({"period": p.as_str(), "at": at})),
        "last": last.map(|l| json!({
            "at": l.at, "period": l.period.as_str(), "theme": l.theme,
            "left_out": l.left_out,
            "problem": l.problem.map(|p| match p {
                Problem::NoTheme => "no-theme",
                Problem::Failed => "failed",
            }),
        })),
    });
    // A record of what happened; losing it loses nothing else.
    let _ = ensure_dir(host, &host.state_dir()).and_then(|()| {
        host.fs
            .write_atomic(&auto_path(host), &to_bytes(&value), 0o600)
    });
}

/// Forget what the last scheduled switch left out: the user has applied a
/// theme since and seen the reasons.
pub(crate) fn clear_left_out(host: &Host<'_>) {
    if let (handled, Some(mut last)) = load_auto(host)
        && !(last.left_out.is_empty() && last.problem.is_none())
    {
        last.left_out.clear();
        last.problem = None;
        save_auto(host, handled, Some(&last));
    }
}

// ---------------------------------------------------------------- status

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub on: bool,
    /// On, and following the desktop's dark style instead of the clock.
    pub follows_style: bool,
    /// The timer's files or the command it starts were changed or removed
    /// outside Velatura. Setting the schedule again repairs it.
    pub damaged: bool,
    pub times: Times,
    pub last: Option<LastRun>,
}

fn hash_at(host: &Host<'_>, path: &Path) -> Option<String> {
    read_opt(host, path).ok().flatten().map(|b| sha256_hex(&b))
}

pub fn status(host: &Host<'_>) -> Status {
    let setup = Setup::load(host);
    let dir = unit_dir(host);
    let intact = setup.as_ref().is_some_and(|s| {
        hash_at(host, &dir.join(SERVICE)).as_deref() == Some(s.service.as_str())
            && hash_at(host, &dir.join(s.trigger())).as_deref() == Some(s.timer.as_str())
            && matches!(host.fs.kind(Path::new(&s.exe)), Ok(Some(_)))
    });
    Status {
        on: setup.is_some(),
        follows_style: setup.as_ref().is_some_and(|s| s.follow),
        damaged: setup.is_some() && !intact,
        times: Times::load(host),
        last: load_auto(host).1,
    }
}

// ------------------------------------------------------------- on and off

/// Why the schedule was not turned on or off. Nothing was changed, unless
/// the variant says otherwise.
#[derive(Debug)]
pub enum Refused {
    /// A daytime and a nighttime theme have to be chosen first.
    NoThemes,
    /// The two times are the same.
    SameTime,
    /// Following the dark style needs a light daytime theme and a dark
    /// nighttime theme.
    NotAPair,
    /// The command runs from a place a timer cannot count on.
    NotInstalled(PathBuf),
    /// Other timers that may change the theme are turned on.
    OtherTimers(Vec<String>),
    /// A file with the name Velatura uses exists and is not Velatura's.
    Foreign(PathBuf),
    OutsideHome(PathBuf),
    Busy,
    Io(String),
    /// systemd did not take it. What was written has been removed again.
    Systemd(String),
}

fn io<T>(result: std::io::Result<T>) -> Result<T, Refused> {
    result.map_err(|e| Refused::Io(e.to_string()))
}

/// Write the timer and turn it on, or replace it when the times changed.
/// `exe` is the command the timer starts. `anyway` turns it on even though
/// another timer looks like it changes the theme too.
pub fn enable(
    host: &Host<'_>,
    systemd: &dyn Systemd,
    exe: &Path,
    times: Times,
    anyway: bool,
) -> Result<(), Refused> {
    set_up(host, systemd, exe, Some(times), anyway)
}

/// Change between the two themes when the desktop changes between its
/// light and its dark style, in place of the clock. Needs a light theme
/// for the day and a dark one for the night. While this is on, applying a
/// theme leaves the style setting alone: the style leads, the theme follows.
pub fn enable_follow(
    host: &Host<'_>,
    systemd: &dyn Systemd,
    exe: &Path,
    anyway: bool,
) -> Result<(), Refused> {
    set_up(host, systemd, exe, None, anyway)
}

/// The appearance of the chosen daytime and nighttime themes.
fn chosen_pair(host: &Host<'_>) -> Option<(Appearance, Appearance)> {
    let chosen = DayNight::load(host);
    let (themes, _) = library::load(host);
    let appearance = |spec: Option<String>| {
        let spec = spec?;
        let (id, variant) = match spec.split_once('/') {
            Some((id, v)) => (id, Some(v)),
            None => (spec.as_str(), None),
        };
        let entry = themes.iter().find(|e| e.theme.id == id)?;
        Resolved::new(&entry.theme, variant).map(|r| r.colors.appearance)
    };
    appearance(chosen.day).zip(appearance(chosen.night))
}

fn set_up(
    host: &Host<'_>,
    systemd: &dyn Systemd,
    exe: &Path,
    times: Option<Times>,
    anyway: bool,
) -> Result<(), Refused> {
    let follow = times.is_none();
    let chosen = DayNight::load(host);
    if chosen.day.is_none() || chosen.night.is_none() {
        return Err(Refused::NoThemes);
    }
    if times.is_some_and(|t| t.day == t.night) {
        return Err(Refused::SameTime);
    }
    if follow && chosen_pair(host) != Some((Appearance::Light, Appearance::Dark)) {
        return Err(Refused::NotAPair);
    }
    let written = match times {
        Some(times) => units(exe, times),
        None => follow_units(exe, &host.dirs.config.join("dconf").join("user")),
    };
    let Some((service, timer)) = written.filter(|_| lasting(host, exe)) else {
        return Err(Refused::NotInstalled(exe.to_owned()));
    };
    let trigger = if follow { WATCH } else { TIMER };
    let others: Vec<String> = targets::other_tools(host)
        .into_iter()
        .filter_map(|tool| match tool {
            OtherTool::Timer { unit } => Some(unit),
            _ => None,
        })
        .collect();
    if !others.is_empty() && !anyway {
        return Err(Refused::OtherTimers(others));
    }

    // One or the other: the clock's timer goes when the style takes over,
    // and the other way round.
    if Setup::load(host).is_some_and(|s| s.follow != follow) {
        disable(host, systemd)?;
    }
    let dir = unit_dir(host);
    let before = Setup::load(host);
    let wanted = [(SERVICE, &service), (trigger, &timer)];
    for (name, content) in wanted {
        let path = dir.join(name);
        let ours = before.as_ref().map(|s| match name {
            SERVICE => s.service.as_str(),
            _ => s.timer.as_str(),
        });
        match hash_at(host, &path) {
            None => {}
            Some(now) if Some(now.as_str()) == ours || now == sha256_hex(content.as_bytes()) => {}
            Some(_) => return Err(Refused::Foreign(path)),
        }
    }

    io(ensure_dir(host, &host.state_dir()))?;
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Refused::Busy);
    };

    // Only the last two levels are ever made; the config folder is there.
    let mut made = before.as_ref().map(|s| s.dirs.clone()).unwrap_or_default();
    for level in [dir.parent().unwrap_or(&dir), &dir] {
        if io(host.fs.kind(level))?.is_none() {
            io(host.fs.create_dir(level, 0o755))?;
            made.push(path_str(level));
        }
    }
    if !targets::inside_home(host, &dir) {
        remove_dirs(host, &made);
        return Err(Refused::OutsideHome(dir));
    }
    if let Some(times) = times {
        io(times.save(host))?;
    }
    let setup = Setup {
        exe: path_str(exe),
        service: sha256_hex(service.as_bytes()),
        timer: sha256_hex(timer.as_bytes()),
        follow,
        dirs: made,
    };
    // The record first: a file without one would later count as foreign.
    io(setup.save(host))?;
    for (name, content) in wanted {
        io(host
            .fs
            .write_atomic(&dir.join(name), content.as_bytes(), 0o644))?;
    }

    let enable: &[&str] = if follow {
        &["enable", WATCH, SERVICE]
    } else {
        &["enable", TIMER]
    };
    let started = systemd
        .user(&["daemon-reload"])
        .and_then(|()| systemd.user(enable))
        // Restarting makes a running timer take the new times.
        .and_then(|()| systemd.user(&["restart", trigger]));
    if let Err(why) = started {
        if before.is_none() {
            remove_files(host, &setup);
            let _ = systemd.user(&["daemon-reload"]);
        }
        return Err(Refused::Systemd(why));
    }
    Ok(())
}

fn remove_dirs(host: &Host<'_>, dirs: &[String]) {
    for dir in dirs.iter().rev() {
        // Fails, and is meant to, when something else lives there now.
        let _ = host.fs.remove_empty_dir(Path::new(dir));
    }
}

/// Take away the unit files that are still as written, the folders made
/// for them, and the record. Returns the files left because they changed.
fn remove_files(host: &Host<'_>, setup: &Setup) -> Vec<PathBuf> {
    let dir = unit_dir(host);
    let mut kept = Vec::new();
    for (name, written) in [(SERVICE, &setup.service), (setup.trigger(), &setup.timer)] {
        let path = dir.join(name);
        match hash_at(host, &path) {
            None => {}
            Some(now) if now == *written => {
                let _ = host.fs.remove_file(&path);
            }
            Some(_) => kept.push(path),
        }
    }
    remove_dirs(host, &setup.dirs);
    let _ = host.fs.remove_file(&setup_path(host));
    kept
}

/// Turn the timer off and remove it. `Ok(None)` if it was not on;
/// otherwise the files left in place because they were changed by hand.
pub fn disable(host: &Host<'_>, systemd: &dyn Systemd) -> Result<Option<Vec<PathBuf>>, Refused> {
    let Some(setup) = Setup::load(host) else {
        return Ok(None);
    };
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Refused::Busy);
    };
    // A timer whose file is already gone has nothing left to turn off.
    let present = matches!(
        host.fs.kind(&unit_dir(host).join(setup.trigger())),
        Ok(Some(_))
    );
    let off: &[&str] = if setup.follow {
        &["disable", "--now", WATCH, SERVICE]
    } else {
        &["disable", "--now", TIMER]
    };
    if let Err(why) = systemd.user(off)
        && present
    {
        return Err(Refused::Systemd(why));
    }
    let kept = remove_files(host, &setup);
    let _ = systemd.user(&["daemon-reload"]);
    Ok(Some(kept))
}

// ------------------------------------------------------------ the switch

/// What `switch auto` did.
#[derive(Debug)]
pub enum Auto {
    /// This part of the day was already seen to; whatever theme the user
    /// chose since is left alone until the next change of period.
    NotDue(Period),
    /// No theme is chosen for this part of the day, or it no longer exists.
    NoTheme(Period),
    Done {
        period: Period,
        /// `theme/variant`.
        theme: String,
        report: Report,
    },
}

/// True while the themes follow the desktop's dark style. Applying a
/// theme then leaves the style setting alone.
pub fn follows_style(host: &Host<'_>) -> bool {
    Setup::load(host).is_some_and(|s| s.follow)
}

/// A reason for leaving something out that needs nobody's attention.
fn expected(why: &Skip) -> bool {
    matches!(
        why,
        Skip::NotSelected | Skip::NotInstalled | Skip::NeedsAdwGtk3 | Skip::NeedsUserThemes
    )
}

/// Apply the theme that belongs to the present time, once per period, with
/// no one there to answer: nothing that needs agreement is done, nothing is
/// removed, and what was left out is recorded. `minute` is the local time
/// in minutes after midnight. `again` acts even if this period was seen to.
pub fn run_auto(host: &Host<'_>, env: Env<'_>, minute: u16, again: bool) -> Result<Auto, Error> {
    let times = Times::load(host);
    let (handled, _) = load_auto(host);
    let (period, due) = if follows_style(host) {
        // The style leads: dark is the nighttime theme. Acts once per
        // change of style, so a theme picked by hand since then stays.
        let dark = matches!(
            host.settings.read(targets::INTERFACE_SCHEMA, "color-scheme"),
            Ok(Some(s)) if s.effective == "prefer-dark"
        );
        let period = if dark { Period::Night } else { Period::Day };
        (period, handled.is_none_or(|(p, _)| p != period))
    } else {
        let period = times.period(minute);
        // An hour of slack absorbs a clock change between then and now.
        let started = (host.now / 60).saturating_sub(u64::from(times.since_start(minute))) * 60;
        (
            period,
            handled.is_none_or(|(p, at)| p != period || at + 3600 < started),
        )
    };
    if !due && !again {
        return Ok(Auto::NotDue(period));
    }

    let chosen = DayNight::load(host);
    let spec = match period {
        Period::Day => chosen.day,
        Period::Night => chosen.night,
    };
    let (themes, _) = library::load(host);
    let resolved = spec.as_deref().and_then(|spec| {
        let (id, variant) = match spec.split_once('/') {
            Some((id, v)) => (id, Some(v)),
            None => (spec, None),
        };
        let entry = themes.iter().find(|e| e.theme.id == id)?;
        Resolved::new(&entry.theme, variant)
    });
    let mut last = LastRun {
        at: host.now,
        period,
        theme: None,
        left_out: Vec::new(),
        problem: None,
    };
    let Some(resolved) = resolved else {
        last.problem = Some(Problem::NoTheme);
        // Seen to, so the same thing is not reported at every start.
        save_auto(host, Some((period, host.now)), Some(&last));
        return Ok(Auto::NoTheme(period));
    };

    let opts = ApplyOpts {
        unattended: true,
        ..ApplyOpts::default()
    };
    let done = engine::plan_apply(host, env, &resolved, &opts)
        .and_then(|plan| engine::execute(host, &plan, &[]));
    match done {
        Ok(report) => {
            last.theme = Some(resolved.label());
            last.left_out = report
                .skipped
                .iter()
                .filter(|(_, why)| !expected(why))
                .map(|(instance, _)| instance.clone())
                .collect();
            save_auto(host, Some((period, host.now)), Some(&last));
            Ok(Auto::Done {
                period,
                theme: resolved.label(),
                report,
            })
        }
        Err(e) => {
            // Not seen to: the next start tries again.
            last.problem = Some(Problem::Failed);
            save_auto(host, handled, Some(&last));
            Err(e)
        }
    }
}
