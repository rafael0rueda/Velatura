//! The operations: preview, apply, undo, restore, history, status.
//!
//! Planning only reads. [`execute`] is the single place where the machine is
//! changed, always under the lock, always after recovering an interrupted
//! run, and always through a journaled transaction.

use std::path::{Path, PathBuf};
use std::time::Duration;

use velatura_theme::Resolved;
use velatura_theme::model::CONTRAST_FLOOR;

use crate::block;
use crate::host::{Host, acquire_lock};
use crate::plan::{Cause, Change, Note, Plan, Skip, TargetPlan};
use crate::store::{
    self, Ledger, Manifest, Prefs, Record, State, StoreError, get_blob, hash_opt, path_str,
    put_blob, read_opt, setting_id,
};
use crate::targets::{self, Env};
use crate::txn::{self, Failure, Rollback, Step};

#[derive(Debug)]
pub enum Error {
    /// Another Velatura is changing things right now.
    Busy,
    Store(StoreError),
    Io(String),
    NothingToUndo,
    /// An earlier interrupted run could not be fully rolled back. Nothing
    /// new is changed until it is.
    Recovery(Rollback),
    Failed(Failure),
}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T, Error> {
    result.map_err(|e| Error::Io(e.to_string()))
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOpts {
    /// If not empty, only these targets (`kitty` or `kitty:default`).
    pub only: Vec<String>,
    pub except: Vec<String>,
    /// Overwrite files Velatura made that were changed outside it.
    pub replace_changed: bool,
}

fn matches(instance: &str, names: &[String]) -> bool {
    names
        .iter()
        .any(|n| n == instance || instance.split(':').next() == Some(n.as_str()))
}

/// What applying `theme` would do. Reads only.
pub fn plan_apply(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    opts: &ApplyOpts,
) -> Result<Plan, Error> {
    let state = State::load(host)?;
    let prefs = Prefs::load(host)?;
    let mut plans = Vec::new();
    for instance in targets::INSTANCES {
        // Naming a target outright overrides having turned it off.
        let selected = if opts.only.is_empty() {
            !prefs.excluded.contains(instance)
        } else {
            matches(instance, &opts.only)
        } && !matches(instance, &opts.except);
        plans.push(if selected {
            targets::plan(instance, host, env, theme, &state, opts.replace_changed)
        } else {
            TargetPlan::skip(instance, Skip::NotSelected)
        });
    }
    let low = theme.colors.ui.fg.contrast(theme.colors.ui.bg) < CONTRAST_FLOOR;
    if let Some(first) = plans.first_mut().filter(|_| low) {
        first.notes.push(Note::LowContrast);
    }
    Ok(Plan {
        cause: Cause::Apply,
        theme: Some((theme.label(), theme.fingerprint())),
        undo_of: None,
        targets: plans,
    })
}

/// Take the block out of the file as it is now, keeping everything else.
fn unblock(
    host: &Host<'_>,
    path: &str,
    target: &str,
    added_eol: bool,
    created_file: bool,
    out: &mut TargetPlan,
) -> Result<(), Error> {
    let file = PathBuf::from(path);
    let Some(current) = io(read_opt(host, &file))? else {
        return Ok(());
    };
    let (body, rest) = match block::find(&current, target) {
        block::Found::Absent => return Ok(()),
        block::Found::Malformed => {
            out.notes.push(Note::KeptFile { path: file });
            return Ok(());
        }
        block::Found::Present { body, .. } => (
            body,
            block::remove(&current, target, added_eol).unwrap_or_default(),
        ),
    };
    out.changes.push(Change::Block {
        path: file,
        target: target.to_owned(),
        pre: Some(velatura_theme::sha256_hex(&current)),
        content: (!(created_file && rest.is_empty())).then_some(rest),
        body_before: Some(body),
        body_after: None,
        added_eol,
        created_file,
    });
    Ok(())
}

fn current_setting(host: &Host<'_>, schema: &str, key: &str) -> Option<Option<String>> {
    host.settings
        .read(schema, key)
        .ok()
        .flatten()
        .map(|s| s.user)
}

/// The note for a setting that could not be planned: the store cannot be
/// reached at all, or the setting was changed by someone else.
fn kept_setting(host: &Host<'_>, schema: &str, key: &str, wanted: &Option<String>) -> Note {
    match host.settings.read(schema, key) {
        Err(crate::host::SettingsError::Unavailable(_)) => Note::SettingNeedsSession {
            schema: schema.to_owned(),
            key: key.to_owned(),
            value: wanted.clone(),
        },
        _ => Note::KeptSetting {
            key: setting_id(schema, key),
        },
    }
}

/// What undoing the most recent change would do. Reads only.
pub fn plan_undo(host: &Host<'_>) -> Result<Plan, Error> {
    let (all, _) = io(store::manifests(host))?;
    let Some(last) = all.iter().rev().find(|m| !m.undone) else {
        return Err(Error::NothingToUndo);
    };
    let mut out = TargetPlan::new("");
    for record in last.records.iter().rev() {
        match record {
            Record::CreateDir { path } => out.changes.push(Change::RemoveDir {
                path: PathBuf::from(path),
            }),
            Record::RemoveDir { path } => {
                if io(host.fs.kind(Path::new(path)))?.is_none() {
                    out.changes.push(Change::CreateDir {
                        path: PathBuf::from(path),
                    });
                }
            }
            Record::Owned { path, pre, post } => {
                let file = PathBuf::from(path);
                let now = hash_opt(io(read_opt(host, &file))?.as_deref());
                if now == *pre {
                    continue;
                }
                if now != *post {
                    out.notes.push(Note::KeptFile { path: file });
                    continue;
                }
                let content = match pre {
                    Some(sha) => Some(io(get_blob(host, sha))?),
                    None => None,
                };
                out.changes.push(Change::Owned {
                    path: file,
                    content,
                    pre: now,
                });
            }
            Record::Block {
                path,
                target,
                body_before,
                added_eol,
                created_file,
                ..
            } => match body_before {
                None => unblock(host, path, target, *added_eol, *created_file, &mut out)?,
                Some(body) => {
                    let file = PathBuf::from(path);
                    let current = io(read_opt(host, &file))?;
                    let before = current.clone().unwrap_or_default();
                    match block::upsert(&before, target, body) {
                        None => out.notes.push(Note::KeptFile { path: file }),
                        Some(up) if up.content == before => {}
                        Some(up) => out.changes.push(Change::Block {
                            path: file,
                            target: target.clone(),
                            pre: hash_opt(current.as_deref()),
                            content: Some(up.content),
                            body_before: up.before,
                            body_after: Some(body.clone()),
                            added_eol: *added_eol,
                            created_file: *created_file,
                        }),
                    }
                }
            },
            Record::Setting {
                schema,
                key,
                pre,
                post,
            } => match current_setting(host, schema, key) {
                Some(now) if now == *pre => {}
                Some(now) if now == *post => out.changes.push(Change::Setting {
                    schema: schema.clone(),
                    key: key.clone(),
                    value: pre.clone(),
                    pre: now,
                }),
                _ => out.notes.push(kept_setting(host, schema, key, pre)),
            },
        }
    }
    Ok(Plan {
        cause: Cause::Undo,
        theme: last.theme.clone().map(|t| (t, String::new())),
        undo_of: Some(last.id),
        targets: vec![out],
    })
}

/// What removing everything Velatura added would do: its files, its blocks,
/// and the settings it wrote put back to what the ledger recorded. Reads only.
pub fn plan_restore_original(host: &Host<'_>) -> Result<Plan, Error> {
    let state = State::load(host)?;
    let ledger = Ledger::load(host)?;
    let mut out = TargetPlan::new("");
    for (path, written) in &state.owned {
        let file = PathBuf::from(path);
        match hash_opt(io(read_opt(host, &file))?.as_deref()) {
            None => {}
            Some(now) if now == *written => out.changes.push(Change::Owned {
                path: file,
                content: None,
                pre: Some(now),
            }),
            Some(_) => out.notes.push(Note::KeptFile { path: file }),
        }
    }
    for (path, record) in &state.blocks {
        unblock(
            host,
            path,
            &record.target,
            record.added_eol,
            record.created_file,
            &mut out,
        )?;
    }
    for dir in state.dirs.iter().rev() {
        out.changes.push(Change::RemoveDir {
            path: PathBuf::from(dir),
        });
    }
    for (id, written) in &state.settings {
        let Some((schema, key)) = id.rsplit_once(' ') else {
            continue;
        };
        let original = ledger.settings.get(id).and_then(|(_, was)| was.clone());
        match current_setting(host, schema, key) {
            Some(now) if now == original => {}
            Some(now) if now == *written => out.changes.push(Change::Setting {
                schema: schema.to_owned(),
                key: key.to_owned(),
                value: original,
                pre: now,
            }),
            _ => out.notes.push(kept_setting(host, schema, key, &original)),
        }
    }
    Ok(Plan {
        cause: Cause::RestoreOriginal,
        theme: None,
        undo_of: None,
        targets: vec![out],
    })
}

/// What an executed plan did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The backup that undoes this operation, if anything was changed.
    pub backup: Option<u64>,
    pub changes: Vec<Change>,
    pub skipped: Vec<(String, Skip)>,
    pub notes: Vec<(String, Note)>,
    /// Set when an interrupted earlier run was rolled back first.
    pub recovered: Option<Rollback>,
}

fn lower(changes: &[Change]) -> Vec<Step> {
    let step = |c: &Change| match c {
        Change::CreateDir { path } => Step::MkDir { path: path.clone() },
        Change::RemoveDir { path } => Step::RmDir { path: path.clone() },
        Change::Owned { path, content, pre }
        | Change::Block {
            path, content, pre, ..
        } => match content {
            Some(bytes) => Step::Write {
                path: path.clone(),
                content: bytes.clone(),
                pre: pre.clone(),
                mode: 0o644,
            },
            None => Step::Remove {
                path: path.clone(),
                pre: pre.clone().unwrap_or_default(),
            },
        },
        Change::Setting {
            schema,
            key,
            value,
            pre,
        } => Step::Setting {
            schema: schema.clone(),
            key: key.clone(),
            value: value.clone(),
            pre: pre.clone(),
        },
    };
    // Every step that can fail for an ordinary reason comes before the
    // settings, which cannot be made fully transactional.
    let is_setting = |c: &&Change| matches!(c, Change::Setting { .. });
    changes
        .iter()
        .filter(|c| !is_setting(c))
        .chain(changes.iter().filter(is_setting))
        .map(step)
        .collect()
}

fn internal(host: &Host<'_>, path: PathBuf, content: Vec<u8>) -> Result<Step, Error> {
    let pre = hash_opt(io(read_opt(host, &path))?.as_deref());
    Ok(Step::Write {
        path,
        content,
        pre,
        mode: 0o600,
    })
}

/// Carry out a plan. `consented` names the instances the user agreed to
/// have a line added for; others that need agreement are skipped.
pub fn execute(host: &Host<'_>, plan: &Plan, consented: &[String]) -> Result<Report, Error> {
    io(store::ensure_store(host))?;
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Error::Busy);
    };

    let recovered = txn::recover(host);
    if let Some(r) = recovered.as_ref().filter(|r| !r.is_clean()) {
        return Err(Error::Recovery(r.clone()));
    }

    let mut report = Report {
        recovered,
        ..Report::default()
    };
    let mut changes = Vec::new();
    let mut agreed = Vec::new();
    for target in &plan.targets {
        for note in &target.notes {
            report.notes.push((target.instance.clone(), note.clone()));
        }
        if let Some(why) = &target.skipped {
            report.skipped.push((target.instance.clone(), why.clone()));
        } else if target.needs_consent && !consented.contains(&target.instance) {
            report
                .skipped
                .push((target.instance.clone(), Skip::NoConsent));
        } else {
            if target.needs_consent {
                agreed.push(target.instance.clone());
            }
            changes.extend(target.changes.iter().cloned());
        }
    }
    // An undo with nothing left to reverse is still recorded as done.
    if changes.is_empty() && plan.cause != Cause::Undo {
        return Ok(report);
    }

    let state = State::load(host)?;
    let mut steps = Vec::new();
    let mut new_state;
    match plan.cause {
        Cause::Apply | Cause::RestoreOriginal => {
            let id = next_id(host)?;
            let manifest = Manifest {
                id,
                time: host.now,
                cause: plan.cause.as_str().to_owned(),
                theme: plan.theme.as_ref().map(|(label, _)| label.clone()),
                undone: false,
                state_before: Some(io(put_blob(host, &state.to_bytes()))?),
                records: changes.iter().map(Record::of).collect(),
            };
            steps.push(internal(
                host,
                Manifest::path(host, id),
                manifest.to_bytes(),
            )?);
            let ledger = Ledger::load(host)?;
            let added = ledger.with(&changes, host.now);
            if added != ledger {
                steps.push(internal(host, Ledger::path(host), added.to_bytes())?);
            }
            steps.extend(lower(&changes));
            if plan.cause == Cause::Apply {
                new_state = state.after(&changes);
                new_state.applied = plan.theme.clone();
                new_state.consents.extend(agreed);
            } else {
                new_state = State {
                    consents: state.consents.clone(),
                    ..State::default()
                };
            }
            report.backup = Some(id);
        }
        Cause::Undo => {
            let (all, _) = io(store::manifests(host))?;
            let Some(mut manifest) = all.into_iter().find(|m| Some(m.id) == plan.undo_of) else {
                return Err(Error::NothingToUndo);
            };
            new_state = match &manifest.state_before {
                Some(sha) => State::from_bytes(&io(get_blob(host, sha))?)
                    .ok_or_else(|| Error::Io("backup content is damaged".to_owned()))?,
                None => State::default(),
            };
            new_state.consents.extend(state.consents.iter().cloned());
            manifest.undone = true;
            steps.extend(lower(&changes));
            steps.push(internal(
                host,
                Manifest::path(host, manifest.id),
                manifest.to_bytes(),
            )?);
        }
    }
    steps.push(internal(host, State::path(host), new_state.to_bytes())?);

    txn::run(host, &steps).map_err(Error::Failed)?;
    if plan.cause == Cause::Apply {
        // Housekeeping; a failure here loses nothing.
        let _ = store::prune(host);
    }
    report.changes = changes;
    Ok(report)
}

fn next_id(host: &Host<'_>) -> Result<u64, Error> {
    let dir = host.backups_dir();
    let mut max = 0;
    for name in io(host.fs.list(&dir))? {
        let name = name.to_string_lossy();
        if let Some(n) = name
            .strip_suffix(".json")
            .and_then(|s| s.parse::<u64>().ok())
        {
            max = max.max(n);
        }
    }
    Ok(max + 1)
}

/// Turn an instance on or off for future applies. What is already applied
/// to it stays until the next apply or undo.
pub fn set_included(host: &Host<'_>, instance: &str, included: bool) -> Result<(), Error> {
    let mut prefs = Prefs::load(host)?;
    let changed = if included {
        prefs.excluded.remove(instance)
    } else {
        prefs.excluded.insert(instance.to_owned())
    };
    if changed {
        io(prefs.save(host))?;
    }
    Ok(())
}

/// Remember the app window's size for its next start.
pub fn save_window_size(host: &Host<'_>, width: i32, height: i32) -> Result<(), Error> {
    let mut prefs = Prefs::load(host)?;
    prefs.window = Some((width, height));
    io(prefs.save(host))
}

pub fn window_size(host: &Host<'_>) -> Option<(i32, i32)> {
    Prefs::load(host).ok().and_then(|p| p.window)
}

/// Instances turned off for future applies.
pub fn excluded(host: &Host<'_>) -> Result<Vec<String>, Error> {
    Ok(Prefs::load(host)?.excluded.into_iter().collect())
}

/// Undo backups, newest first, down to and including `id`: the result is
/// the state from before that backup. Stops at the first step that fails.
pub fn restore_to(host: &Host<'_>, id: u64) -> Result<Vec<Report>, Error> {
    let mut reports = Vec::new();
    loop {
        let plan = match plan_undo(host) {
            Err(Error::NothingToUndo) if !reports.is_empty() => return Ok(reports),
            other => other?,
        };
        if plan.undo_of.is_none_or(|current| current < id) {
            return if reports.is_empty() {
                Err(Error::NothingToUndo)
            } else {
                Ok(reports)
            };
        }
        reports.push(execute(host, &plan, &[])?);
    }
}

/// Roll back an interrupted run, if there is one. Used at start-up and by
/// the recovery command.
pub fn recover(host: &Host<'_>) -> Result<Option<Rollback>, Error> {
    if io(read_opt(host, &txn::journal_path(host)))?.is_none() {
        return Ok(None);
    }
    let Some(_lock) = io(acquire_lock(
        &host.state_dir().join("lock"),
        Duration::from_secs(5),
    ))?
    else {
        return Err(Error::Busy);
    };
    Ok(txn::recover(host))
}

/// Backups, newest first, and any backup files that could not be read.
pub fn history(host: &Host<'_>) -> Result<(Vec<Manifest>, Vec<PathBuf>), Error> {
    let (mut all, bad) = io(store::manifests(host))?;
    all.reverse();
    Ok((all, bad))
}

/// How one thing Velatura put on the machine looks now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Ok,
    /// Changed outside Velatura since it was written.
    Changed,
    Missing,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub applied: Option<(String, String)>,
    /// An interrupted run is waiting to be rolled back.
    pub unfinished: bool,
    pub files: Vec<(String, Health)>,
    pub blocks: Vec<(String, Health)>,
    pub settings: Vec<(String, Option<String>, Health)>,
}

pub fn status(host: &Host<'_>) -> Result<Status, Error> {
    let state = State::load(host)?;
    let mut s = Status {
        applied: state.applied.clone(),
        unfinished: io(read_opt(host, &txn::journal_path(host)))?.is_some(),
        ..Status::default()
    };
    for (path, written) in &state.owned {
        let health = match hash_opt(io(read_opt(host, Path::new(path)))?.as_deref()) {
            None => Health::Missing,
            Some(now) if now == *written => Health::Ok,
            Some(_) => Health::Changed,
        };
        s.files.push((path.clone(), health));
    }
    for (path, record) in &state.blocks {
        let health = match io(read_opt(host, Path::new(path)))? {
            None => Health::Missing,
            Some(bytes) => match block::find(&bytes, &record.target) {
                block::Found::Present { .. } => Health::Ok,
                block::Found::Absent => Health::Missing,
                block::Found::Malformed => Health::Changed,
            },
        };
        s.blocks.push((path_str(Path::new(path)), health));
    }
    for (id, written) in &state.settings {
        let health = match id.rsplit_once(' ') {
            Some((schema, key)) => match current_setting(host, schema, key) {
                Some(now) if now == *written => Health::Ok,
                Some(_) => Health::Changed,
                None => Health::Missing,
            },
            None => Health::Missing,
        };
        s.settings.push((id.clone(), written.clone(), health));
    }
    Ok(s)
}
