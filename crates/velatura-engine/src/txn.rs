//! Transactions: carry out a list of steps so that either all of them
//! happen, or the machine is put back as it was.
//!
//! Before the first step a journal is written that names every step with the
//! hash of what was there and of what will be there. An interrupted run is
//! rolled back from the journal the next time Velatura starts. Rollback
//! restores an item only if it still holds exactly what the interrupted run
//! wrote; anything changed since is newer work and is kept.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use velatura_theme::sha256_hex;

use crate::host::{Host, Kind, TMP_MARK};
use crate::store::{self, VERSION, get_blob, hash_opt, path_str, put_blob, read_opt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    MkDir {
        path: PathBuf,
    },
    /// Removes the directory if it is empty; otherwise does nothing.
    RmDir {
        path: PathBuf,
    },
    Write {
        path: PathBuf,
        content: Vec<u8>,
        pre: Option<String>,
        mode: u32,
    },
    Remove {
        path: PathBuf,
        pre: String,
    },
    Setting {
        schema: String,
        key: String,
        value: Option<String>,
        pre: Option<String>,
    },
}

/// Why a run did not complete.
#[derive(Debug)]
pub enum Failure {
    /// Something changed between planning and executing. Nothing was done.
    Stale(String),
    /// A step failed. `rollback` says how the undoing went.
    Step {
        what: String,
        error: String,
        rollback: Rollback,
    },
    /// The backup or the journal could not be written. Nothing was done.
    Prepare(String),
}

/// The outcome of rolling a run back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rollback {
    /// Items left as they are because they were changed after the run.
    pub kept: Vec<String>,
    /// Items that could not be restored, with the reason. While any remain
    /// the journal stays, and the next start tries again.
    pub errors: Vec<(String, String)>,
}

impl Rollback {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

pub(crate) fn journal_path(host: &Host<'_>) -> PathBuf {
    host.state_dir().join("journal.json")
}

fn opt(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |s| json!(s))
}

fn journal_bytes(steps: &[Step]) -> Vec<u8> {
    let steps: Vec<Value> = steps
        .iter()
        .map(|step| match step {
            Step::MkDir { path } => json!({"op": "mkdir", "path": path_str(path)}),
            Step::RmDir { path } => json!({"op": "rmdir", "path": path_str(path)}),
            Step::Write {
                path,
                content,
                pre,
                mode,
            } => json!({
                "op": "write", "path": path_str(path), "mode": mode,
                "pre": opt(pre), "post": sha256_hex(content),
            }),
            Step::Remove { path, pre } => json!({
                "op": "write", "path": path_str(path), "mode": 0o644,
                "pre": pre, "post": Value::Null,
            }),
            Step::Setting {
                schema,
                key,
                value,
                pre,
            } => json!({
                "op": "setting", "schema": schema, "key": key,
                "pre": opt(pre), "post": opt(value),
            }),
        })
        .collect();
    store::to_bytes(&json!({"version": VERSION, "steps": steps}))
}

fn current_hash(host: &Host<'_>, path: &Path) -> io::Result<Option<String>> {
    Ok(hash_opt(read_opt(host, path)?.as_deref()))
}

fn user_value(host: &Host<'_>, schema: &str, key: &str) -> Result<Option<String>, String> {
    match host.settings.read(schema, key) {
        Ok(Some(state)) => Ok(state.user),
        Ok(None) => Err("no such setting".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

fn do_step(host: &Host<'_>, step: &Step) -> Result<(), String> {
    let io = |r: io::Result<()>| r.map_err(|e| e.to_string());
    match step {
        Step::MkDir { path } => io(host.fs.create_dir(path, 0o755)),
        Step::RmDir { path } => {
            if host.fs.kind(path).map_err(|e| e.to_string())? == Some(Kind::Dir)
                && host.fs.list(path).map_err(|e| e.to_string())?.is_empty()
            {
                io(host.fs.remove_empty_dir(path))
            } else {
                Ok(())
            }
        }
        Step::Write {
            path,
            content,
            pre,
            mode,
        } => {
            // Checked again immediately before writing.
            if current_hash(host, path).map_err(|e| e.to_string())? != *pre {
                return Err("changed since the preview".to_owned());
            }
            io(host.fs.write_atomic(path, content, *mode))
        }
        Step::Remove { path, pre } => {
            if current_hash(host, path)
                .map_err(|e| e.to_string())?
                .as_ref()
                != Some(pre)
            {
                return Err("changed since the preview".to_owned());
            }
            if host.fs.kind(path).map_err(|e| e.to_string())? != Some(Kind::File) {
                return Err("not a regular file".to_owned());
            }
            io(host.fs.remove_file(path))
        }
        Step::Setting {
            schema,
            key,
            value,
            pre,
        } => {
            if user_value(host, schema, key)? != *pre {
                return Err("changed since the preview".to_owned());
            }
            host.settings
                .write(schema, key, value.as_deref())
                .map_err(|e| e.to_string())
        }
    }
}

fn label(step: &Step) -> String {
    match step {
        Step::MkDir { path }
        | Step::RmDir { path }
        | Step::Write { path, .. }
        | Step::Remove { path, .. } => path_str(path),
        Step::Setting { schema, key, .. } => store::setting_id(schema, key),
    }
}

/// Carry out `steps` in order, or leave the machine as it was.
pub(crate) fn run(host: &Host<'_>, steps: &[Step]) -> Result<(), Failure> {
    // 1. Nothing may have moved since the plan was made, and the content
    //    about to be replaced is stored before anything is touched.
    for step in steps {
        match step {
            Step::Write { path, pre, .. } => {
                let current = read_opt(host, path).map_err(|e| Failure::Prepare(format!("{e}")))?;
                if hash_opt(current.as_deref()) != *pre {
                    return Err(Failure::Stale(path_str(path)));
                }
                if let Some(bytes) = current {
                    put_blob(host, &bytes).map_err(|e| Failure::Prepare(format!("{e}")))?;
                }
            }
            Step::Remove { path, pre } => {
                let current = read_opt(host, path).map_err(|e| Failure::Prepare(format!("{e}")))?;
                if hash_opt(current.as_deref()).as_ref() != Some(pre) {
                    return Err(Failure::Stale(path_str(path)));
                }
                if let Some(bytes) = current {
                    put_blob(host, &bytes).map_err(|e| Failure::Prepare(format!("{e}")))?;
                }
            }
            Step::Setting {
                schema, key, pre, ..
            } => {
                if user_value(host, schema, key).map_err(Failure::Prepare)? != *pre {
                    return Err(Failure::Stale(store::setting_id(schema, key)));
                }
            }
            Step::MkDir { path } => {
                if host.fs.kind(path).ok().flatten().is_some() {
                    return Err(Failure::Stale(path_str(path)));
                }
            }
            Step::RmDir { .. } => {}
        }
    }

    // 2. The journal is the promise to finish or undo.
    let journal = journal_bytes(steps);
    host.fs
        .write_atomic(&journal_path(host), &journal, 0o600)
        .map_err(|e| Failure::Prepare(format!("{e}")))?;

    // 3. Files first, settings last (the caller orders them so).
    for step in steps {
        if let Err(error) = do_step(host, step) {
            return Err(Failure::Step {
                what: label(step),
                error,
                rollback: rollback(host, &journal),
            });
        }
    }
    host.settings.sync();
    // A settings store can accept a write and lose it. Believe only what
    // reads back.
    for step in steps {
        if let Step::Setting {
            schema, key, value, ..
        } = step
            && user_value(host, schema, key).ok().as_ref() != Some(value)
        {
            return Err(Failure::Step {
                what: label(step),
                error: "the setting did not take the new value".to_owned(),
                rollback: rollback(host, &journal),
            });
        }
    }

    // 4. Removing the journal is the commit.
    if let Err(e) = host.fs.remove_file(&journal_path(host)) {
        return Err(Failure::Step {
            what: path_str(&journal_path(host)),
            error: e.to_string(),
            rollback: rollback(host, &journal),
        });
    }
    Ok(())
}

/// If an earlier run was interrupted, roll it back. `None` when there was
/// nothing to do.
pub fn recover(host: &Host<'_>) -> Option<Rollback> {
    let journal = read_opt(host, &journal_path(host)).ok().flatten()?;
    Some(rollback(host, &journal))
}

fn rollback(host: &Host<'_>, journal: &[u8]) -> Rollback {
    let mut report = Rollback::default();
    let parsed: Option<Value> = serde_json::from_slice(journal).ok();
    let steps = parsed
        .as_ref()
        .filter(|v| v.get("version").and_then(Value::as_u64) == Some(VERSION))
        .and_then(|v| v.get("steps"))
        .and_then(Value::as_array);
    let Some(steps) = steps else {
        report.errors.push((
            path_str(&journal_path(host)),
            "the journal cannot be read".to_owned(),
        ));
        return report;
    };

    for step in steps.iter().rev() {
        let text = |k: &str| step.get(k).and_then(Value::as_str);
        let optional = |k: &str| text(k).map(str::to_owned);
        let outcome: Result<bool, String> = match text("op") {
            Some("mkdir") => {
                // Only an empty directory is ever removed.
                if let Some(path) = text("path") {
                    let _ = host.fs.remove_empty_dir(Path::new(path));
                }
                Ok(true)
            }
            Some("rmdir") => match text("path").map(Path::new) {
                Some(path) if matches!(host.fs.kind(path), Ok(None)) => host
                    .fs
                    .create_dir(path, 0o755)
                    .map(|()| true)
                    .map_err(|e| e.to_string()),
                _ => Ok(true),
            },
            Some("write") => match text("path").map(Path::new) {
                Some(path) => restore_file(
                    host,
                    path,
                    optional("pre"),
                    optional("post"),
                    step.get("mode").and_then(Value::as_u64).unwrap_or(0o644) as u32,
                ),
                None => Err("damaged journal entry".to_owned()),
            },
            Some("setting") => match (text("schema"), text("key")) {
                (Some(schema), Some(key)) => {
                    let (pre, post) = (optional("pre"), optional("post"));
                    match user_value(host, schema, key) {
                        Ok(now) if now == pre => Ok(true),
                        Ok(now) if now == post => host
                            .settings
                            .write(schema, key, pre.as_deref())
                            .map(|()| true)
                            .map_err(|e| e.to_string()),
                        Ok(_) => Ok(false),
                        Err(e) => Err(e),
                    }
                }
                _ => Err("damaged journal entry".to_owned()),
            },
            _ => Err("damaged journal entry".to_owned()),
        };
        let name = text("path").map_or_else(
            || store::setting_id(text("schema").unwrap_or("?"), text("key").unwrap_or("?")),
            str::to_owned,
        );
        match outcome {
            Ok(true) => {}
            Ok(false) => report.kept.push(name),
            Err(e) => report.errors.push((name, e)),
        }
        if let Some(path) = text("path").filter(|_| text("op") == Some("write")) {
            sweep_temporaries(host, Path::new(path));
        }
    }
    host.settings.sync();

    if report.is_clean()
        && let Err(e) = host.fs.remove_file(&journal_path(host))
    {
        report
            .errors
            .push((path_str(&journal_path(host)), e.to_string()));
    }
    report
}

/// `Ok(true)` restored or already as before, `Ok(false)` kept as newer work.
fn restore_file(
    host: &Host<'_>,
    path: &Path,
    pre: Option<String>,
    post: Option<String>,
    mode: u32,
) -> Result<bool, String> {
    let now = current_hash(host, path).map_err(|e| e.to_string())?;
    if now == pre {
        return Ok(true);
    }
    if now != post {
        return Ok(false);
    }
    match pre {
        // Restored by copying, never by moving the backup into place.
        Some(sha) => {
            let bytes = get_blob(host, &sha).map_err(|e| e.to_string())?;
            host.fs
                .write_atomic(path, &bytes, mode)
                .map_err(|e| e.to_string())?;
        }
        None => host.fs.remove_file(path).map_err(|e| e.to_string())?,
    }
    Ok(true)
}

/// Remove what an interrupted atomic write may have left next to `path`.
fn sweep_temporaries(host: &Host<'_>, path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!(".{}{TMP_MARK}", name.to_string_lossy());
    for entry in host.fs.list(dir).unwrap_or_default() {
        if entry.to_string_lossy().starts_with(&prefix) {
            let _ = host.fs.remove_file(&dir.join(entry));
        }
    }
}
