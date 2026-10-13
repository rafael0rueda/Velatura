//! What Velatura keeps on disk: its state record, backups and the
//! original-setup ledger.
//!
//! Everything is plain JSON with a version number, so it can be read and
//! repaired by hand from a text console.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use velatura_theme::sha256_hex;

use crate::host::{Host, Kind};
use crate::plan::Change;

/// Version of every persisted format. A newer one makes this build refuse to
/// change anything.
pub const VERSION: u64 = 1;

/// Backups kept, not counting the original-setup ledger.
pub const KEEP: usize = 20;

#[derive(Debug)]
pub enum StoreError {
    Io(PathBuf, io::Error),
    /// Not JSON, or not the shape this build writes.
    Damaged(PathBuf),
    /// Written by a newer version.
    Newer(PathBuf),
}

type Result<T> = std::result::Result<T, StoreError>;

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn hash_opt(content: Option<&[u8]>) -> Option<String> {
    content.map(sha256_hex)
}

/// Read a file that may not exist.
pub fn read_opt(host: &Host<'_>, path: &Path) -> io::Result<Option<Vec<u8>>> {
    match host.fs.read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Create `path` and any missing parents, owner-only.
pub fn ensure_dir(host: &Host<'_>, path: &Path) -> io::Result<()> {
    if host.fs.kind(path)?.is_some() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_dir(host, parent)?;
    }
    match host.fs.create_dir(path, 0o700) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

fn read_json(host: &Host<'_>, path: &Path) -> Result<Option<Value>> {
    let Some(bytes) = read_opt(host, path).map_err(|e| StoreError::Io(path.to_owned(), e))? else {
        return Ok(None);
    };
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| StoreError::Damaged(path.to_owned()))?;
    match value.get("version").and_then(Value::as_u64) {
        Some(v) if v <= VERSION => Ok(Some(value)),
        Some(_) => Err(StoreError::Newer(path.to_owned())),
        None => Err(StoreError::Damaged(path.to_owned())),
    }
}

pub fn to_bytes(value: &Value) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text.into_bytes()
}

fn opt(value: &Option<String>) -> Value {
    value.as_ref().map_or(Value::Null, |s| json!(s))
}

fn get_opt(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn get_str(value: &Value, key: &str) -> Option<String> {
    get_opt(value, key)
}

fn get_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

// ---------------------------------------------------------------- state

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockRecord {
    pub target: String,
    pub added_eol: bool,
    pub created_file: bool,
}

/// What Velatura has put on this machine and still considers its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// `theme/variant` and fingerprint of the theme in use.
    pub applied: Option<(String, String)>,
    /// Instances where adding a line to the user's file was agreed to.
    pub consents: BTreeSet<String>,
    /// Owned files: path to the hash of what was written.
    pub owned: BTreeMap<String, String>,
    pub blocks: BTreeMap<String, BlockRecord>,
    pub dirs: BTreeSet<String>,
    /// `schema key` to the value written (`None`: reset to default).
    pub settings: BTreeMap<String, Option<String>>,
}

pub fn setting_id(schema: &str, key: &str) -> String {
    format!("{schema} {key}")
}

impl State {
    pub fn path(host: &Host<'_>) -> PathBuf {
        host.state_dir().join("state.json")
    }

    pub fn load(host: &Host<'_>) -> Result<Self> {
        let path = Self::path(host);
        match read_json(host, &path)? {
            Some(value) => Self::from_json(&value).ok_or(StoreError::Damaged(path)),
            None => Ok(Self::default()),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::from_json(&serde_json::from_slice(bytes).ok()?)
    }

    fn from_json(v: &Value) -> Option<Self> {
        let mut s = Self::default();
        if let Some(a) = v.get("applied").filter(|a| !a.is_null()) {
            s.applied = Some((get_str(a, "theme")?, get_str(a, "fingerprint")?));
        }
        for c in v.get("consents")?.as_array()? {
            s.consents.insert(c.as_str()?.to_owned());
        }
        for (path, sha) in v.get("owned")?.as_object()? {
            s.owned.insert(path.clone(), sha.as_str()?.to_owned());
        }
        for (path, b) in v.get("blocks")?.as_object()? {
            s.blocks.insert(
                path.clone(),
                BlockRecord {
                    target: get_str(b, "target")?,
                    added_eol: get_bool(b, "added_eol"),
                    created_file: get_bool(b, "created_file"),
                },
            );
        }
        for d in v.get("dirs")?.as_array()? {
            s.dirs.insert(d.as_str()?.to_owned());
        }
        for (id, written) in v.get("settings")?.as_object()? {
            s.settings
                .insert(id.clone(), written.as_str().map(str::to_owned));
        }
        Some(s)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let blocks: Map<String, Value> = self
            .blocks
            .iter()
            .map(|(path, b)| {
                (
                    path.clone(),
                    json!({"target": b.target, "added_eol": b.added_eol, "created_file": b.created_file}),
                )
            })
            .collect();
        let settings: Map<String, Value> = self
            .settings
            .iter()
            .map(|(id, written)| (id.clone(), opt(written)))
            .collect();
        to_bytes(&json!({
            "version": VERSION,
            "applied": self.applied.as_ref().map(|(theme, fingerprint)| json!({"theme": theme, "fingerprint": fingerprint})),
            "consents": self.consents,
            "owned": self.owned,
            "blocks": blocks,
            "dirs": self.dirs,
            "settings": settings,
        }))
    }

    /// The state after `changes` have been carried out.
    pub fn after(&self, changes: &[Change]) -> Self {
        let mut s = self.clone();
        for change in changes {
            match change {
                Change::CreateDir { path } => {
                    s.dirs.insert(path_str(path));
                }
                Change::RemoveDir { path } => {
                    s.dirs.remove(&path_str(path));
                }
                Change::Owned { path, content, .. } => match content {
                    Some(bytes) => {
                        s.owned.insert(path_str(path), sha256_hex(bytes));
                    }
                    None => {
                        s.owned.remove(&path_str(path));
                    }
                },
                Change::Block {
                    path,
                    target,
                    body_after,
                    added_eol,
                    created_file,
                    ..
                } => {
                    if body_after.is_some() {
                        s.blocks.insert(
                            path_str(path),
                            BlockRecord {
                                target: target.clone(),
                                added_eol: *added_eol,
                                created_file: *created_file,
                            },
                        );
                    } else {
                        s.blocks.remove(&path_str(path));
                    }
                }
                Change::Setting {
                    schema, key, value, ..
                } => {
                    s.settings.insert(setting_id(schema, key), value.clone());
                }
            }
        }
        s
    }
}

// ---------------------------------------------------------------- prefs

/// Choices that are about this machine, not about any one theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    /// Instances the user turned off: left out when a theme is applied.
    pub excluded: BTreeSet<String>,
    /// Width and height of the app window when it was last closed.
    pub window: Option<(i32, i32)>,
}

impl Prefs {
    pub fn path(host: &Host<'_>) -> PathBuf {
        host.state_dir().join("prefs.json")
    }

    pub fn load(host: &Host<'_>) -> Result<Self> {
        let path = Self::path(host);
        let Some(v) = read_json(host, &path)? else {
            return Ok(Self::default());
        };
        let excluded = v
            .get("excluded")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect())
            .ok_or(StoreError::Damaged(path))?;
        let size = |k: &str| v.get(k).and_then(Value::as_i64).map(|n| n as i32);
        Ok(Self {
            excluded,
            window: size("window_width").zip(size("window_height")),
        })
    }

    pub fn save(&self, host: &Host<'_>) -> io::Result<()> {
        ensure_dir(host, &host.state_dir())?;
        host.fs.write_atomic(
            &Self::path(host),
            &to_bytes(&json!({
                "version": VERSION,
                "excluded": self.excluded,
                "window_width": self.window.map(|w| w.0),
                "window_height": self.window.map(|w| w.1),
            })),
            0o600,
        )
    }
}

// ------------------------------------------------------------- day/night

/// Which theme is for the day and which for the night. Unlike the rest,
/// this is a preference that makes sense on every machine, so it lives in
/// the config folder, in a file a person can read and edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayNight {
    /// `theme` or `theme/variant`.
    pub day: Option<String>,
    pub night: Option<String>,
}

/// True for `theme` or `theme/variant` made of slugs.
pub fn is_theme_spec(spec: &str) -> bool {
    let mut parts = spec.split('/');
    let ok = |p: Option<&str>| p.is_some_and(velatura_theme::model::is_slug);
    ok(parts.next()) && parts.next().is_none_or(|v| ok(Some(v))) && parts.next().is_none()
}

impl DayNight {
    pub fn path(host: &Host<'_>) -> PathBuf {
        host.dirs.config.join("velatura").join("config.toml")
    }

    /// A missing or unreadable file means nothing has been chosen.
    pub fn load(host: &Host<'_>) -> Self {
        let table = load_config(host);
        let get = |key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .filter(|s| is_theme_spec(s))
                .map(str::to_owned)
        };
        Self {
            day: get("day"),
            night: get("night"),
        }
    }

    /// Rewrite the two lines and keep every other line of the file.
    pub fn save(&self, host: &Host<'_>) -> io::Result<()> {
        let spec = |v: &Option<String>| v.clone().filter(|s| is_theme_spec(s));
        save_config(
            host,
            &[("day", spec(&self.day)), ("night", spec(&self.night))],
        )
    }
}

/// The top-level table of the config file. A missing or unreadable file
/// reads as empty.
pub fn load_config(host: &Host<'_>) -> toml::Table {
    read_opt(host, &DayNight::path(host))
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|text| text.parse().ok())
        .unwrap_or_default()
}

/// Rewrite the lines of the named top-level keys in the config file and
/// keep every other line. A value of `None` takes the key out. Values are
/// written as quoted text and must not need escaping.
pub fn save_config(host: &Host<'_>, keys: &[(&str, Option<String>)]) -> io::Result<()> {
    let path = DayNight::path(host);
    if let Some(dir) = path.parent() {
        ensure_dir(host, dir)?;
    }
    let old = read_opt(host, &path)?
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let ours = |line: &str| {
        let key = line.split('=').next().unwrap_or("").trim();
        keys.iter().any(|(k, _)| *k == key)
    };
    let mut lines: Vec<String> = Vec::new();
    for (key, value) in keys {
        if let Some(value) = value {
            lines.push(format!("{key} = \"{value}\""));
        }
    }
    // Top-level keys must come before any table, so ours go first.
    lines.extend(old.lines().filter(|l| !ours(l)).map(str::to_owned));
    let mut text = lines.join("\n");
    text.push('\n');
    host.fs.write_atomic(&path, text.as_bytes(), 0o644)
}

// ---------------------------------------------------------------- blobs

fn blobs_dir(host: &Host<'_>) -> PathBuf {
    host.backups_dir().join("blobs")
}

/// Create the backup store if it is not there yet. Marked as a cache
/// directory so backup tools skip it.
pub fn ensure_store(host: &Host<'_>) -> io::Result<()> {
    ensure_dir(host, &blobs_dir(host))?;
    ensure_dir(host, &host.state_dir())?;
    let tag = host.backups_dir().join("CACHEDIR.TAG");
    if host.fs.kind(&tag)?.is_none() {
        host.fs.write_atomic(
            &tag,
            b"Signature: 8a477f597d28d172789f06886806bc55\n# Backups made by Velatura. Safe to exclude from other backups.\n",
            0o600,
        )?;
    }
    Ok(())
}

/// Store `content` under its hash and return the hash.
pub fn put_blob(host: &Host<'_>, content: &[u8]) -> io::Result<String> {
    let sha = sha256_hex(content);
    let path = blobs_dir(host).join(&sha);
    if host.fs.kind(&path)?.is_none() {
        host.fs.write_atomic(&path, content, 0o600)?;
    }
    Ok(sha)
}

/// Read a blob and check it still has the content it was stored with.
pub fn get_blob(host: &Host<'_>, sha: &str) -> io::Result<Vec<u8>> {
    let safe = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !safe {
        return Err(io::Error::other("not a backup id"));
    }
    let bytes = host.fs.read(&blobs_dir(host).join(sha))?;
    if sha256_hex(&bytes) != sha {
        return Err(io::Error::other("backup content is damaged"));
    }
    Ok(bytes)
}

// ------------------------------------------------------------- manifests

/// A [`Change`] as recorded in a backup: hashes in place of content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    CreateDir {
        path: String,
    },
    RemoveDir {
        path: String,
    },
    Owned {
        path: String,
        pre: Option<String>,
        post: Option<String>,
    },
    Block {
        path: String,
        target: String,
        pre: Option<String>,
        post: Option<String>,
        body_before: Option<String>,
        body_after: Option<String>,
        added_eol: bool,
        created_file: bool,
    },
    Setting {
        schema: String,
        key: String,
        pre: Option<String>,
        post: Option<String>,
    },
}

impl Record {
    pub fn of(change: &Change) -> Self {
        match change {
            Change::CreateDir { path } => Self::CreateDir {
                path: path_str(path),
            },
            Change::RemoveDir { path } => Self::RemoveDir {
                path: path_str(path),
            },
            Change::Owned { path, content, pre } => Self::Owned {
                path: path_str(path),
                pre: pre.clone(),
                post: hash_opt(content.as_deref()),
            },
            Change::Block {
                path,
                target,
                content,
                pre,
                body_before,
                body_after,
                added_eol,
                created_file,
            } => Self::Block {
                path: path_str(path),
                target: target.clone(),
                pre: pre.clone(),
                post: hash_opt(content.as_deref()),
                body_before: body_before.clone(),
                body_after: body_after.clone(),
                added_eol: *added_eol,
                created_file: *created_file,
            },
            Change::Setting {
                schema,
                key,
                value,
                pre,
            } => Self::Setting {
                schema: schema.clone(),
                key: key.clone(),
                pre: pre.clone(),
                post: value.clone(),
            },
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::CreateDir { path } => json!({"op": "create-dir", "path": path}),
            Self::RemoveDir { path } => json!({"op": "remove-dir", "path": path}),
            Self::Owned { path, pre, post } => {
                json!({"op": "owned", "path": path, "pre": opt(pre), "post": opt(post)})
            }
            Self::Block {
                path,
                target,
                pre,
                post,
                body_before,
                body_after,
                added_eol,
                created_file,
            } => json!({
                "op": "block", "path": path, "target": target,
                "pre": opt(pre), "post": opt(post),
                "body_before": opt(body_before), "body_after": opt(body_after),
                "added_eol": added_eol, "created_file": created_file,
            }),
            Self::Setting {
                schema,
                key,
                pre,
                post,
            } => {
                json!({"op": "setting", "schema": schema, "key": key, "pre": opt(pre), "post": opt(post)})
            }
        }
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(match v.get("op")?.as_str()? {
            "create-dir" => Self::CreateDir {
                path: get_str(v, "path")?,
            },
            "remove-dir" => Self::RemoveDir {
                path: get_str(v, "path")?,
            },
            "owned" => Self::Owned {
                path: get_str(v, "path")?,
                pre: get_opt(v, "pre"),
                post: get_opt(v, "post"),
            },
            "block" => Self::Block {
                path: get_str(v, "path")?,
                target: get_str(v, "target")?,
                pre: get_opt(v, "pre"),
                post: get_opt(v, "post"),
                body_before: get_opt(v, "body_before"),
                body_after: get_opt(v, "body_after"),
                added_eol: get_bool(v, "added_eol"),
                created_file: get_bool(v, "created_file"),
            },
            "setting" => Self::Setting {
                schema: get_str(v, "schema")?,
                key: get_str(v, "key")?,
                pre: get_opt(v, "pre"),
                post: get_opt(v, "post"),
            },
            _ => return None,
        })
    }
}

/// One backup: what an operation changed and what was there before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub id: u64,
    pub time: u64,
    pub cause: String,
    pub theme: Option<String>,
    pub undone: bool,
    /// Blob holding the state record as it was before the operation.
    pub state_before: Option<String>,
    pub records: Vec<Record>,
}

impl Manifest {
    pub fn path(host: &Host<'_>, id: u64) -> PathBuf {
        host.backups_dir().join(format!("{id:06}.json"))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        to_bytes(&json!({
            "version": VERSION,
            "id": self.id,
            "time": self.time,
            "cause": self.cause,
            "theme": opt(&self.theme),
            "undone": self.undone,
            "state_before": opt(&self.state_before),
            "records": self.records.iter().map(Record::to_json).collect::<Vec<_>>(),
        }))
    }

    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            id: v.get("id")?.as_u64()?,
            time: v.get("time")?.as_u64()?,
            cause: get_str(v, "cause")?,
            theme: get_opt(v, "theme"),
            undone: get_bool(v, "undone"),
            state_before: get_opt(v, "state_before"),
            records: v
                .get("records")?
                .as_array()?
                .iter()
                .map(Record::from_json)
                .collect::<Option<_>>()?,
        })
    }

    /// Every blob this backup needs in order to be undone.
    fn blobs(&self) -> impl Iterator<Item = &String> {
        self.state_before
            .iter()
            .chain(self.records.iter().filter_map(|r| match r {
                Record::Owned { pre, .. } | Record::Block { pre, .. } => pre.as_ref(),
                _ => None,
            }))
    }
}

/// All readable backups, oldest first, and the files that could not be read.
pub fn manifests(host: &Host<'_>) -> io::Result<(Vec<Manifest>, Vec<PathBuf>)> {
    let dir = host.backups_dir();
    let mut good = Vec::new();
    let mut bad = Vec::new();
    if host.fs.kind(&dir)? != Some(Kind::Dir) {
        return Ok((good, bad));
    }
    for name in host.fs.list(&dir)? {
        let name = name.to_string_lossy();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if stem.len() != 6 || !stem.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = dir.join(&*name);
        match read_json(host, &path) {
            Ok(Some(v)) => match Manifest::from_json(&v) {
                Some(m) => good.push(m),
                None => bad.push(path),
            },
            _ => bad.push(path),
        }
    }
    good.sort_by_key(|m| m.id);
    Ok((good, bad))
}

// ----------------------------------------------------------------- ledger

/// The state each file and setting was in the first time Velatura touched
/// it. Entries are only ever added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    /// Path to `(time, hash of the original content or None if absent)`.
    pub files: BTreeMap<String, (u64, Option<String>)>,
    /// `schema key` to `(time, the user's original value)`.
    pub settings: BTreeMap<String, (u64, Option<String>)>,
}

impl Ledger {
    pub fn path(host: &Host<'_>) -> PathBuf {
        host.backups_dir().join("original.json")
    }

    pub fn load(host: &Host<'_>) -> Result<Self> {
        let path = Self::path(host);
        let Some(v) = read_json(host, &path)? else {
            return Ok(Self::default());
        };
        let parse = |section: &str| -> Option<BTreeMap<String, (u64, Option<String>)>> {
            v.get(section)?
                .as_object()?
                .iter()
                .map(|(k, e)| Some((k.clone(), (e.get("time")?.as_u64()?, get_opt(e, "was")))))
                .collect()
        };
        match (parse("files"), parse("settings")) {
            (Some(files), Some(settings)) => Ok(Self { files, settings }),
            _ => Err(StoreError::Damaged(path)),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let section = |m: &BTreeMap<String, (u64, Option<String>)>| -> Map<String, Value> {
            m.iter()
                .map(|(k, (time, was))| (k.clone(), json!({"time": time, "was": opt(was)})))
                .collect()
        };
        to_bytes(&json!({
            "version": VERSION,
            "note": "State before Velatura first changed each item. Never pruned.",
            "files": section(&self.files),
            "settings": section(&self.settings),
        }))
    }

    /// Add the pre-change state of anything in `changes` not yet recorded.
    pub fn with(&self, changes: &[Change], now: u64) -> Self {
        let mut l = self.clone();
        for change in changes {
            match change {
                Change::Owned { path, pre, .. } | Change::Block { path, pre, .. } => {
                    l.files
                        .entry(path_str(path))
                        .or_insert_with(|| (now, pre.clone()));
                }
                Change::Setting {
                    schema, key, pre, ..
                } => {
                    l.settings
                        .entry(setting_id(schema, key))
                        .or_insert_with(|| (now, pre.clone()));
                }
                Change::CreateDir { .. } | Change::RemoveDir { .. } => {}
            }
        }
        l
    }
}

// ------------------------------------------------------------------ prune

/// True for a backup of a change the schedule made.
pub fn is_scheduled(m: &Manifest) -> bool {
    m.cause == crate::plan::Cause::Scheduled.as_str()
}

/// Drop backups older than the newest [`KEEP`] changes the user made, and
/// the blobs nothing refers to any more. Backups of scheduled switches do
/// not count towards the number and go with the change they follow. The
/// ledger and what it refers to are never dropped.
pub fn prune(host: &Host<'_>) -> io::Result<()> {
    let (all, _) = manifests(host)?;
    let ledger = Ledger::load(host).unwrap_or_default();
    let drop = all
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| !is_scheduled(m))
        .nth(KEEP - 1)
        .map_or(0, |(i, _)| i);
    for m in &all[..drop] {
        host.fs.remove_file(&Manifest::path(host, m.id))?;
    }
    let mut live: BTreeSet<&String> = all[drop..].iter().flat_map(Manifest::blobs).collect();
    live.extend(ledger.files.values().filter_map(|(_, was)| was.as_ref()));
    for name in host.fs.list(&blobs_dir(host))? {
        let name = name.to_string_lossy().into_owned();
        if !live.contains(&name) && !name.contains(crate::host::TMP_MARK) {
            host.fs.remove_file(&blobs_dir(host).join(&name))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- compose

/// The records of two changes made one after the other, as the records of
/// one change: undoing the result is undoing `second` and then `first`.
///
/// This is how the schedule's switches, twice a day, stay one backup
/// between two changes of the user's instead of pushing those out.
pub fn compose(first: &[Record], second: &[Record]) -> Vec<Record> {
    let key = |r: &Record| match r {
        Record::CreateDir { path } | Record::RemoveDir { path } => format!("d {path}"),
        Record::Owned { path, .. } | Record::Block { path, .. } => format!("f {path}"),
        Record::Setting { schema, key, .. } => format!("s {}", setting_id(schema, key)),
    };
    let mut out: Vec<Option<Record>> = first.iter().cloned().map(Some).collect();
    for later in second {
        let slot = out
            .iter_mut()
            .find(|r| r.as_ref().is_some_and(|r| key(r) == key(later)));
        let Some(slot) = slot else {
            out.push(Some(later.clone()));
            continue;
        };
        *slot = match (slot.take(), later) {
            // Made and then removed, or removed and then made again.
            (Some(Record::CreateDir { .. }), Record::RemoveDir { .. })
            | (Some(Record::RemoveDir { .. }), Record::CreateDir { .. }) => None,
            (
                Some(Record::Owned { path, pre, post }),
                Record::Owned {
                    pre: pre2,
                    post: post2,
                    ..
                },
            ) if post == *pre2 => (pre != *post2).then(|| Record::Owned {
                path,
                pre,
                post: post2.clone(),
            }),
            (
                Some(Record::Block {
                    path,
                    target,
                    pre,
                    body_before,
                    added_eol,
                    created_file,
                    ..
                }),
                Record::Block {
                    target: target2,
                    post: post2,
                    body_after: body_after2,
                    ..
                },
            ) if target == *target2 => (body_before != *body_after2).then(|| Record::Block {
                path,
                target,
                pre,
                post: post2.clone(),
                body_before,
                body_after: body_after2.clone(),
                added_eol,
                created_file,
            }),
            (
                Some(Record::Setting {
                    schema,
                    key,
                    pre,
                    post,
                }),
                Record::Setting {
                    pre: pre2,
                    post: post2,
                    ..
                },
            ) if post == *pre2 => (pre != *post2).then(|| Record::Setting {
                schema,
                key,
                pre,
                post: post2.clone(),
            }),
            // Changed by someone else in between: the later record stands
            // alone, and undoing it gives back what that someone left.
            _ => Some(later.clone()),
        };
    }
    out.into_iter().flatten().collect()
}
