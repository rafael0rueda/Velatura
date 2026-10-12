//! Everything the window knows about the machine, gathered off the main
//! thread. The app never reads or writes a file itself: it calls the engine.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::gio;
use velatura_engine::engine::{self, ApplyOpts};
use velatura_engine::gio_settings::GioSettings;
use velatura_engine::host::{Dirs, Host, StdFs};
use velatura_engine::library::{self, Entry};
use velatura_engine::plan::Skip;
use velatura_engine::store::Manifest;
use velatura_engine::targets::INSTANCES;
use velatura_theme::Resolved;

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

/// How one app or desktop part stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowState {
    /// The theme in use is applied to it and nothing is pending.
    Themed,
    /// It can be themed; the next apply will include it.
    Ready,
    /// It cannot be themed now, for this reason.
    Unavailable(Skip),
}

#[derive(Clone, Debug)]
pub struct AppRow {
    pub instance: &'static str,
    pub state: RowState,
    pub included: bool,
}

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
    /// Set when Velatura's own records could not be read.
    pub problem: Option<String>,
}

pub fn snapshot(host: &Host<'_>) -> Snapshot {
    let (themes, _) = library::load(host);
    let mut snap = Snapshot {
        home: host.dirs.home.clone(),
        window: engine::window_size(host),
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

    // Ask what applying would do to learn how each part stands. The theme
    // in use is the probe; before the first apply, the first theme is.
    let probe = themes
        .iter()
        .find(|e| {
            snap.applied
                .as_deref()
                .is_some_and(|label| label.split('/').next() == Some(e.theme.id.as_str()))
        })
        .or(themes.first());
    let excluded = engine::excluded(host).unwrap_or_default();
    if let Some(resolved) = probe.and_then(|e| {
        let variant = snap.applied.as_deref().and_then(|l| l.split('/').nth(1));
        Resolved::new(&e.theme, variant).or_else(|| Resolved::new(&e.theme, None))
    }) {
        // Naming every instance looks past the ones that are turned off.
        let all = ApplyOpts {
            only: INSTANCES.iter().map(|s| (*s).to_owned()).collect(),
            ..ApplyOpts::default()
        };
        if let Ok(plan) = engine::plan_apply(host, &env, &resolved, &all) {
            for instance in INSTANCES {
                let Some(t) = plan.targets.iter().find(|t| t.instance == instance) else {
                    continue;
                };
                let state = match &t.skipped {
                    Some(why) => RowState::Unavailable(why.clone()),
                    None if t.changes.is_empty() && snap.applied.is_some() => RowState::Themed,
                    None => RowState::Ready,
                };
                snap.rows.push(AppRow {
                    instance,
                    state,
                    included: !excluded.iter().any(|e| e == instance),
                });
            }
        }
    }
    snap.themes = themes;
    snap
}
