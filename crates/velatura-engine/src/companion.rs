//! Velatura's own GNOME Shell extension: installing it for this user,
//! removing it, and knowing whether it is there and turned on.
//!
//! The extension loads one stylesheet that the Shell target writes. It
//! holds no colours, so it is the same for every theme and is compiled
//! into this program. Like the schedule's timer it is set up outside the
//! undo history: Undo reverses changes of theme.
//!
//! Velatura installs the files and stops there. Turning an extension on
//! is the user's to do, and the Shell only sees a new extension after the
//! next sign-in.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use velatura_theme::sha256_hex;

use crate::host::{Host, Kind, acquire_lock};
use crate::schedule::Refused;
use crate::store::{VERSION, ensure_dir, path_str, read_opt, to_bytes};
use crate::targets::{self, Env};

pub const UUID: &str = "velatura@rafael0rueda.github.io";

/// The extension, as it is published. Two files and nothing else.
pub const FILES: [(&str, &str); 2] = [
    (
        "metadata.json",
        include_str!("../../../extension/velatura@rafael0rueda.github.io/metadata.json"),
    ),
    (
        "extension.js",
        include_str!("../../../extension/velatura@rafael0rueda.github.io/extension.js"),
    ),
];

/// The file the extension loads for the Shell's colours: with the next
/// five and the list of tasks, the whole contract between the two.
pub fn stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("gnome-shell.css")
}

/// The file that gives the extension's outline around the focused window
/// its colour. Without it the extension draws no outline.
pub fn outline_stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("window-outline.css")
}

/// The file that draws the top bar as a full bar or as pills, and how
/// opaque. Without it the bar is as the Shell part leaves it.
pub fn top_bar_stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("top-bar.css")
}

/// The UPS indicator's look. The extension shows the indicator only
/// while this file is there.
pub fn ups_stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("ups.css")
}

/// The checklist's look. The extension shows the checklist only while
/// this file is there.
pub fn tasks_stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("tasks.css")
}

/// The media player's look. The extension shows the player only while
/// this file is there.
pub fn media_stylesheet(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("shell").join("media.css")
}

/// The tasks themselves, which the extension writes and the app never
/// does: the user's own, so no apply, undo or removal touches them.
pub fn tasks_list(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("tasks.json")
}

fn user_dir(host: &Host<'_>) -> PathBuf {
    host.dirs.data.join("gnome-shell/extensions").join(UUID)
}

fn record_path(host: &Host<'_>) -> PathBuf {
    host.state_dir().join("extension.json")
}

/// File name to the hash of what Velatura wrote, and the folders it made.
type Record = (Vec<(String, String)>, Vec<String>);

fn record(host: &Host<'_>) -> Option<Record> {
    let bytes = read_opt(host, &record_path(host)).ok().flatten()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    if v.get("version")?.as_u64()? > VERSION {
        return None;
    }
    let files = v
        .get("files")?
        .as_object()?
        .iter()
        .filter_map(|(name, sha)| Some((name.clone(), sha.as_str()?.to_owned())))
        .collect();
    let dirs = v
        .get("dirs")?
        .as_array()?
        .iter()
        .filter_map(|d| d.as_str().map(str::to_owned))
        .collect();
    Some((files, dirs))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Where the extension is: this user's copy, or one a package put in
    /// the system's folders.
    pub installed: Option<PathBuf>,
    /// True if this user's copy was put there by Velatura.
    pub by_velatura: bool,
    /// Velatura's copy differs from the one this build carries: installing
    /// it again brings it up to date. Not for a copy of a later version,
    /// as a later Velatura installs: this build leaves that one as it is.
    pub differs: bool,
    /// Velatura's copy is an earlier version than the one this build
    /// carries, so parts that need the later one cannot be shown. A copy
    /// that differs only in its description is not.
    pub outdated: bool,
    /// Turned on in the Shell's settings. `None` if those cannot be read.
    pub enabled: Option<bool>,
    /// The running Shell has an earlier version of Velatura's copy loaded
    /// than the one in its folder, as after an update: the Shell takes the
    /// new one after signing out and in. False when it cannot be asked.
    pub waiting: bool,
}

impl Status {
    /// True if the Shell loads the stylesheet: the Shell target then
    /// writes for the extension and not for User Themes.
    pub fn active(&self) -> bool {
        self.installed.is_some() && self.enabled == Some(true)
    }
}

fn hash_at(host: &Host<'_>, path: &Path) -> Option<String> {
    read_opt(host, path).ok().flatten().map(|b| sha256_hex(&b))
}

/// The `version-name` an extension's metadata gives, as a number.
fn version_of(metadata: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(metadata).ok()?;
    v.get("version-name")?.as_str()?.parse().ok()
}

/// The version of the copy in this user's extensions folder, if it says.
fn user_version(host: &Host<'_>) -> Option<u64> {
    let bytes = read_opt(host, &user_dir(host).join("metadata.json"))
        .ok()
        .flatten()?;
    version_of(&bytes)
}

/// True if this user's copy is of a later version than this build's.
fn later_than_ours(host: &Host<'_>) -> bool {
    user_version(host).is_some_and(|v| Some(v) > version_of(FILES[0].1.as_bytes()))
}

pub fn status(host: &Host<'_>, env: Env<'_>) -> Status {
    let installed = targets::data_dirs(host, env)
        .into_iter()
        .map(|d| d.join("gnome-shell/extensions").join(UUID))
        .find(|d| matches!(host.fs.kind(&d.join("metadata.json")), Ok(Some(_))));
    let ours = installed.as_ref() == Some(&user_dir(host)) && record(host).is_some();
    let differs = ours
        && !later_than_ours(host)
        && FILES.iter().any(|(name, text)| {
            hash_at(host, &user_dir(host).join(name)) != Some(sha256_hex(text.as_bytes()))
        });
    // A copy whose version cannot be read is taken as an earlier one.
    let outdated =
        differs && user_version(host).is_none_or(|v| Some(v) < version_of(FILES[0].1.as_bytes()));
    let loaded = host
        .settings
        .read(&format!("extension:{UUID}"), "version-name")
        .ok()
        .flatten()
        .and_then(|s| s.effective.parse::<u64>().ok());
    let waiting =
        ours && loaded.is_some_and(|running| user_version(host).is_some_and(|v| running < v));
    let enabled = match host.settings.read("org.gnome.shell", "enabled-extensions") {
        Ok(Some(list)) => {
            let off = matches!(
                host.settings.read("org.gnome.shell", "disable-user-extensions"),
                Ok(Some(s)) if s.effective == "true"
            );
            Some(!off && targets::listed(&list.effective).contains(&UUID))
        }
        _ => None,
    };
    Status {
        installed,
        by_velatura: ours,
        differs,
        outdated,
        enabled,
        waiting,
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T, Refused> {
    result.map_err(|e| Refused::Io(e.to_string()))
}

/// Put the extension in this user's extensions folder, or bring Velatura's
/// copy there up to date. `Ok(false)` if it was already as it should be,
/// or is Velatura's copy of a later version, which is kept. A folder of
/// that name that Velatura did not make is left alone.
pub fn install(host: &Host<'_>) -> Result<bool, Refused> {
    let dir = user_dir(host);
    let before = record(host);
    if before.is_some() && later_than_ours(host) {
        return Ok(false);
    }
    let wanted: Vec<(&str, String)> = FILES
        .iter()
        .map(|(name, text)| (*name, sha256_hex(text.as_bytes())))
        .collect();
    let mut current = true;
    for (name, sha) in &wanted {
        let ours = before
            .as_ref()
            .and_then(|(files, _)| files.iter().find(|(n, _)| n == name))
            .map(|(_, sha)| sha.as_str());
        match hash_at(host, &dir.join(name)) {
            Some(now) if now == *sha => {}
            Some(now) if Some(now.as_str()) == ours => current = false,
            Some(_) => return Err(Refused::Foreign(dir.join(name))),
            None => current = false,
        }
    }
    if before.is_none() && matches!(host.fs.kind(&dir), Ok(Some(_))) && !current {
        return Err(Refused::Foreign(dir));
    }
    if current && before.is_some() {
        return Ok(false);
    }
    if !targets::inside_home(host, &host.dirs.data) {
        return Err(Refused::OutsideHome(dir));
    }
    io(ensure_dir(host, &host.state_dir()))?;
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Refused::Busy);
    };
    // Up to three levels are made: gnome-shell, extensions, and its own.
    let mut made = before.map(|(_, dirs)| dirs).unwrap_or_default();
    let levels: Vec<&Path> = dir.ancestors().take(3).collect();
    for level in levels.into_iter().rev() {
        match io(host.fs.kind(level))? {
            None => {
                io(host.fs.create_dir(level, 0o755))?;
                made.push(path_str(level));
            }
            Some(Kind::Dir | Kind::Symlink) => {}
            Some(_) => return Err(Refused::Foreign(level.to_owned())),
        }
    }
    // The record first: a file without one would later count as foreign.
    let files: serde_json::Map<String, Value> = wanted
        .iter()
        .map(|(name, sha)| ((*name).to_owned(), json!(sha)))
        .collect();
    io(host.fs.write_atomic(
        &record_path(host),
        &to_bytes(&json!({"version": VERSION, "files": files, "dirs": made})),
        0o600,
    ))?;
    for (name, text) in FILES {
        io(host
            .fs
            .write_atomic(&dir.join(name), text.as_bytes(), 0o644))?;
    }
    Ok(true)
}

/// Remove the copy Velatura installed. `Ok(None)` if there is none;
/// otherwise the files left because they were changed by hand. The
/// stylesheet is the Shell target's and goes with the theme, not here.
pub fn remove(host: &Host<'_>) -> Result<Option<Vec<PathBuf>>, Refused> {
    let Some((files, dirs)) = record(host) else {
        return Ok(None);
    };
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Refused::Busy);
    };
    let dir = user_dir(host);
    let mut kept = Vec::new();
    for (name, written) in &files {
        let path = dir.join(name);
        match hash_at(host, &path) {
            None => {}
            Some(now) if now == *written => io(host.fs.remove_file(&path))?,
            Some(_) => kept.push(path),
        }
    }
    for made in dirs.iter().rev() {
        // Fails, and is meant to, when something else lives there now.
        let _ = host.fs.remove_empty_dir(Path::new(made));
    }
    io(host.fs.remove_file(&record_path(host)))?;
    Ok(Some(kept))
}
