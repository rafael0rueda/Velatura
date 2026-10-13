//! Everything the window knows about the machine, gathered off the main
//! thread. The app never reads or writes a file itself: it calls the engine.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::gio;
use velatura_engine::engine;
use velatura_engine::gio_settings::GioSettings;
use velatura_engine::host::{Dirs, Host, StdFs, self_command};
use velatura_engine::library::{self, Entry};
use velatura_engine::schedule;
use velatura_engine::store::{DayNight, Manifest};

pub fn env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Run `f` with access to the machine. `None` if there is no home folder.
fn with_host<T>(f: impl FnOnce(&Host<'_>) -> T) -> Option<T> {
    let dirs = Dirs::from_env(env)?;
    let settings = GioSettings::new();
    let host = Host {
        dirs: &dirs,
        fs: &StdFs,
        settings: &settings,
        now: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    Some(f(&host))
}

/// Run an engine call on a worker thread and wait for it without blocking
/// the window.
pub async fn run<T: Send + 'static>(f: impl FnOnce(&Host<'_>) -> T + Send + 'static) -> Option<T> {
    gio::spawn_blocking(move || with_host(f))
        .await
        .ok()
        .flatten()
}

/// Remember the window's size. Called from a worker thread.
pub fn save_window_size(width: i32, height: i32) {
    with_host(|host| {
        let _ = engine::save_window_size(host, width, height);
    });
}

pub use velatura_engine::engine::{Part as AppRow, PartState as RowState};

/// One consistent picture of the machine.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub home: PathBuf,
    pub themes: Vec<Entry>,
    /// `theme/variant` of the theme in use.
    pub applied: Option<String>,
    /// An interrupted change is waiting to be rolled back.
    pub unfinished: bool,
    pub history: Vec<Manifest>,
    pub rows: Vec<AppRow>,
    pub window: Option<(i32, i32)>,
    pub day_night: DayNight,
    pub schedule: schedule::Status,
    /// The `velatura` command beside this app, for a timer to start.
    pub command: Option<PathBuf>,
    /// True if that command is installed where a timer can count on it.
    pub can_schedule: bool,
    /// Set when Velatura's own records could not be read.
    pub problem: Option<String>,
}

pub fn snapshot(host: &Host<'_>) -> Snapshot {
    let (themes, _) = library::load(host);
    let mut snap = Snapshot {
        home: host.dirs.home.clone(),
        window: engine::window_size(host),
        day_night: DayNight::load(host),
        schedule: schedule::status(host),
        command: self_command(Some("velatura")),
        ..Snapshot::default()
    };
    match engine::status(host) {
        Ok(status) => {
            snap.applied = status.applied.map(|(label, _)| label);
            snap.unfinished = status.unfinished;
        }
        Err(e) => snap.problem = Some(format!("{e:?}")),
    }
    if let Ok((history, _)) = engine::history(host) {
        snap.history = history;
    }

    snap.can_schedule = snap
        .command
        .as_ref()
        .is_some_and(|c| schedule::lasting(host, c));
    snap.rows = engine::parts(host, &env, &themes);
    snap.themes = themes;
    snap
}
