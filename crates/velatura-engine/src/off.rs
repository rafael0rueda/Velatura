//! A part turned off is taken away at the next apply by hand: its files,
//! its blocks in the user's files, the folders it made and the settings it
//! wrote, as Remove Everything does, but for that part only. Otherwise it
//! would keep the colors of whatever theme was applied last.
//!
//! What a part owns is not written down part by part. It is found by
//! planning the part as if Velatura had never touched this machine: its
//! files not there, its blocks out of the user's files, its settings as
//! they were before. Everything the part would then write is its own, and
//! what of that the state still holds is taken away.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use velatura_theme::Resolved;

use crate::block;
use crate::engine::{self, ApplyOpts, Error};
use crate::host::{Fs, Host, Kind, SettingState, Settings, SettingsError};
use crate::plan::{Change, Skip, TargetPlan};
use crate::store::{Ledger, State, path_str, read_opt, setting_id};
use crate::targets::{self, Env};
use crate::templates;

/// The files of this machine as they were before Velatura.
struct Untouched<'a> {
    fs: &'a dyn Fs,
    /// Velatura's own files, as not there, and the user's files that hold
    /// a block, as they are without it: not there if Velatura made them.
    before: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

impl<'a> Untouched<'a> {
    fn new(host: &Host<'a>, state: &State) -> Self {
        let mut before: BTreeMap<PathBuf, Option<Vec<u8>>> = state
            .owned
            .keys()
            .map(|path| (PathBuf::from(path), None))
            .collect();
        for (path, record) in &state.blocks {
            let file = PathBuf::from(path);
            let Ok(Some(now)) = read_opt(host, &file) else {
                continue;
            };
            let rest = if record.target == targets::FLATPAK_SPAN {
                crate::keyfile::remove(
                    &now,
                    targets::FLATPAK_GROUP,
                    targets::FLATPAK_KEY,
                    record.body.as_deref().unwrap_or_default(),
                    record.added_eol,
                )
            } else {
                block::remove(&now, &record.target, record.added_eol)
            };
            if let Some(rest) = rest {
                let gone = record.created_file && rest.is_empty();
                before.insert(file, (!gone).then_some(rest));
            }
        }
        Self {
            fs: host.fs,
            before,
        }
    }
}

fn read_only() -> io::Error {
    io::Error::other("planning only reads")
}

impl Fs for Untouched<'_> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.before.get(path) {
            Some(Some(bytes)) => Ok(bytes.clone()),
            Some(None) => Err(io::ErrorKind::NotFound.into()),
            None => self.fs.read(path),
        }
    }

    fn write_atomic(&self, _: &Path, _: &[u8], _: u32) -> io::Result<()> {
        Err(read_only())
    }

    fn kind(&self, path: &Path) -> io::Result<Option<Kind>> {
        match self.before.get(path) {
            Some(None) => Ok(None),
            _ => self.fs.kind(path),
        }
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.fs.canonicalize(path)
    }

    fn create_dir(&self, _: &Path, _: u32) -> io::Result<()> {
        Err(read_only())
    }

    fn remove_file(&self, _: &Path) -> io::Result<()> {
        Err(read_only())
    }

    fn remove_empty_dir(&self, _: &Path) -> io::Result<()> {
        Err(read_only())
    }

    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let mut names = self.fs.list(path)?;
        names.retain(|name| !matches!(self.before.get(&path.join(name)), Some(None)));
        Ok(names)
    }

    fn links(&self, path: &Path) -> io::Result<u64> {
        self.fs.links(path)
    }
}

/// The settings Velatura wrote, as they were before it.
struct UntouchedSettings<'a> {
    settings: &'a dyn Settings,
    before: BTreeMap<String, Option<String>>,
}

impl Settings for UntouchedSettings<'_> {
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError> {
        let now = self.settings.read(schema, key)?;
        Ok(match (now, self.before.get(&setting_id(schema, key))) {
            (Some(now), Some(was)) => Some(SettingState {
                effective: was.clone().unwrap_or_else(|| now.default.clone()),
                user: was.clone(),
                ..now
            }),
            (now, _) => now,
        })
    }

    fn write(&self, _: &str, _: &str, _: Option<&str>) -> Result<(), SettingsError> {
        Err(SettingsError::NotWritable)
    }

    fn sync(&self) {}

    fn keys(&self, schema: &str) -> Vec<String> {
        self.settings.keys(schema)
    }
}

/// What taking `instance` away would do, given how it plans on a machine
/// Velatura never touched. Reads only.
fn plan_off(
    instance: &str,
    host: &Host<'_>,
    state: &State,
    ledger: &Ledger,
    plan: impl FnOnce(&Host<'_>) -> TargetPlan,
) -> Result<TargetPlan, Error> {
    let fs = Untouched::new(host, state);
    let settings = UntouchedSettings {
        settings: host.settings,
        before: state
            .settings
            .keys()
            .map(|id| {
                let was = ledger.settings.get(id).and_then(|(_, was)| was.clone());
                (id.clone(), was)
            })
            .collect(),
    };
    let untouched = Host {
        dirs: host.dirs,
        fs: &fs,
        settings: &settings,
        now: host.now,
    };
    let wanted = plan(&untouched);
    let nothing = || TargetPlan::skip(instance, Skip::NotSelected);
    if wanted.skipped.is_some() {
        return Ok(nothing());
    }
    let mut out = TargetPlan::new(instance);
    let mut files = BTreeSet::new();
    for change in &wanted.changes {
        match change {
            Change::Owned { path, .. } => {
                if let Some(written) = state.owned.get(&path_str(path)) {
                    engine::take_owned(host, &path_str(path), written, &mut out)?;
                    files.insert(path.clone());
                }
            }
            Change::Block { path, .. } => {
                if let Some(record) = state.blocks.get(&path_str(path)) {
                    engine::unblock(
                        host,
                        &path_str(path),
                        &record.target,
                        (record.added_eol, record.created_file),
                        record.body.as_deref(),
                        &mut out,
                    )?;
                    files.insert(path.clone());
                }
            }
            Change::Setting { schema, key, .. } => {
                let id = setting_id(schema, key);
                if let Some(written) = state.settings.get(&id) {
                    engine::take_setting(host, &id, written, ledger, &mut out);
                }
            }
            Change::CreateDir { .. } | Change::RemoveDir { .. } => {}
        }
    }
    // A folder Velatura made goes with the part's files once nothing else
    // of Velatura's is in it, deepest first; one the user put something
    // in stays where it is.
    let others: Vec<&Path> = state
        .owned
        .keys()
        .chain(state.blocks.keys())
        .map(Path::new)
        .filter(|p| !files.contains(*p))
        .collect();
    for dir in state.dirs.iter().rev().map(Path::new) {
        if files.iter().any(|f| f.starts_with(dir)) && !others.iter().any(|o| o.starts_with(dir)) {
            out.changes.push(Change::RemoveDir {
                path: dir.to_owned(),
            });
        }
    }
    if out.changes.is_empty() && out.notes.is_empty() {
        return Ok(nothing());
    }
    Ok(out)
}

/// What taking away a part that is turned off would do: everything of
/// Velatura's it still has. Skipped as not selected when it has nothing.
/// Reads only.
pub(crate) fn plan_part_off(
    instance: &str,
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    ledger: &Ledger,
) -> Result<TargetPlan, Error> {
    // Nothing is made to find out: a recoloured set is only named.
    let opts = ApplyOpts {
        probe: true,
        ..ApplyOpts::default()
    };
    let taken = match instance.strip_prefix(templates::PREFIX) {
        Some(id) => match templates::load(host)
            .into_iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, entry)| entry.ok())
        {
            Some(entry) => plan_off(instance, host, state, ledger, |h| {
                templates::plan_template(h, theme, state, false, &entry)
            })?,
            None => TargetPlan::skip(instance, Skip::NotSelected),
        },
        None => plan_off(instance, host, state, ledger, |h| {
            targets::plan(instance, h, env, theme, state, &opts)
        })?,
    };
    // A stylesheet only Velatura's extension shows is planned only while
    // the extension is on, and found by its place while it is not.
    if taken.skipped.is_some() && targets::SHEETS.contains(&instance) {
        return Ok(targets::plan_sheet_off(instance, host, state));
    }
    Ok(taken)
}
