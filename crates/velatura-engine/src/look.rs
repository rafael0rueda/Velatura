//! Icons, cursor, wallpaper and fonts: a choice among what is installed.
//!
//! Each is a plain desktop setting. The choice belongs to the user and the
//! machine, not to a theme, so it is made on its own, stays when the theme
//! changes, and goes through the same backup and undo as everything else.

use std::path::{Path, PathBuf};

use velatura_theme::model::clean_text;

use crate::engine::Error;
use crate::host::{Host, Kind as FileKind, SettingsError};
use crate::plan::{Cause, Change, Note, Plan, Skip, TargetPlan};
use crate::store::{State, path_str, read_opt};
use crate::targets::{Env, INTERFACE_SCHEMA, data_dirs};

/// The name these choices go by in plans and backups.
pub const INSTANCE: &str = "gnome:look";

pub const BACKGROUND_SCHEMA: &str = "org.gnome.desktop.background";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Icons,
    Cursor,
    Wallpaper,
    Font,
    DocumentFont,
    MonospaceFont,
}

pub const KINDS: [Kind; 6] = [
    Kind::Icons,
    Kind::Cursor,
    Kind::Wallpaper,
    Kind::Font,
    Kind::DocumentFont,
    Kind::MonospaceFont,
];

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Icons => "icons",
            Self::Cursor => "cursor",
            Self::Wallpaper => "wallpaper",
            Self::Font => "font",
            Self::DocumentFont => "document-font",
            Self::MonospaceFont => "monospace-font",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        KINDS.into_iter().find(|k| k.id() == id)
    }

    /// The settings that hold the choice. The wallpaper has one for each
    /// style, and both get the same picture.
    pub fn keys(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Icons => &[(INTERFACE_SCHEMA, "icon-theme")],
            Self::Cursor => &[(INTERFACE_SCHEMA, "cursor-theme")],
            Self::Wallpaper => &[
                (BACKGROUND_SCHEMA, "picture-uri"),
                (BACKGROUND_SCHEMA, "picture-uri-dark"),
            ],
            Self::Font => &[(INTERFACE_SCHEMA, "font-name")],
            Self::DocumentFont => &[(INTERFACE_SCHEMA, "document-font-name")],
            Self::MonospaceFont => &[(INTERFACE_SCHEMA, "monospace-font-name")],
        }
    }

    fn is_font(self) -> bool {
        matches!(self, Self::Font | Self::DocumentFont | Self::MonospaceFont)
    }
}

/// One thing that can be chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the setting is given: a theme's folder name, or a file's URI.
    pub value: String,
    /// What a person is shown. Plain text, cleaned.
    pub name: String,
}

/// Folders that hold icon and cursor themes, the user's first.
fn icon_dirs(host: &Host<'_>, env: Env<'_>) -> Vec<PathBuf> {
    let mut dirs = vec![host.dirs.home.join(".icons")];
    dirs.extend(data_dirs(host, env).into_iter().map(|d| d.join("icons")));
    dirs
}

/// `Name=` from a theme's index file, if it has one.
fn theme_name(host: &Host<'_>, index: &Path) -> Option<String> {
    let bytes = read_opt(host, index).ok().flatten()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .find_map(|l| l.strip_prefix("Name="))
        .map(|n| clean_text(n.trim(), 80))
        .filter(|n| !n.is_empty())
}

/// A folder name that can be a setting's value as it is.
fn plain(name: &str) -> bool {
    !name.is_empty() && name.len() <= 120 && !name.chars().any(char::is_control)
}

fn themes(host: &Host<'_>, env: Env<'_>, cursors: bool) -> Vec<Choice> {
    let mut found: Vec<Choice> = Vec::new();
    for dir in icon_dirs(host, env) {
        for entry in host.fs.list(&dir).unwrap_or_default() {
            let Some(name) = entry.to_str().filter(|n| plain(n)) else {
                continue;
            };
            // Fallbacks every theme inherits from, not themes to pick.
            if matches!(name, "default" | "hicolor" | "locolor") {
                continue;
            }
            // Velatura's own recoloured sets follow the theme; the icon
            // theme they were made from is the one to pick.
            if crate::icon_set::made_from(host, name).is_some() {
                continue;
            }
            let theme = dir.join(name);
            let index = theme.join("index.theme");
            let is = if cursors {
                matches!(
                    host.fs.kind(&theme.join("cursors")),
                    Ok(Some(FileKind::Dir | FileKind::Symlink))
                )
            } else {
                // An icon theme lists its folders; a cursor-only one does not.
                read_opt(host, &index).ok().flatten().is_some_and(|b| {
                    String::from_utf8_lossy(&b)
                        .lines()
                        .any(|l| l.starts_with("Directories="))
                })
            };
            if is && !found.iter().any(|c| c.value == name) {
                found.push(Choice {
                    value: name.to_owned(),
                    name: theme_name(host, &index).unwrap_or_else(|| name.to_owned()),
                });
            }
        }
    }
    found.sort_by_key(|c| c.name.to_lowercase());
    found
}

/// A path as a `file://` address, with everything but plain characters
/// written as `%XX`.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

const PICTURES: [&str; 6] = ["jpg", "jpeg", "png", "webp", "jxl", "svg"];

fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PICTURES.contains(&e.to_ascii_lowercase().as_str()))
}

fn pictures(host: &Host<'_>, dir: &Path, depth: u8, out: &mut Vec<Choice>) {
    for entry in host.fs.list(dir).unwrap_or_default() {
        if out.len() >= 500 {
            return;
        }
        let path = dir.join(&entry);
        match host.fs.kind(&path) {
            Ok(Some(FileKind::Dir)) if depth > 0 => pictures(host, &path, depth - 1, out),
            Ok(Some(FileKind::File)) if is_picture(&path) => {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let folder = path
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap_or_default()
                    .to_string_lossy();
                out.push(Choice {
                    value: file_uri(&path),
                    name: clean_text(&format!("{stem} ({folder})"), 80),
                });
            }
            _ => {}
        }
    }
}

/// What is installed and can be chosen. Empty for fonts: the app asks
/// the desktop's own font chooser, and the command takes a font by name.
pub fn available(host: &Host<'_>, env: Env<'_>, kind: Kind) -> Vec<Choice> {
    match kind {
        Kind::Icons => themes(host, env, false),
        Kind::Cursor => themes(host, env, true),
        Kind::Wallpaper => {
            let mut found = Vec::new();
            for dir in data_dirs(host, env) {
                pictures(host, &dir.join("backgrounds"), 3, &mut found);
            }
            found.sort_by_key(|c| c.name.to_lowercase());
            found
        }
        _ => Vec::new(),
    }
}

/// What is in use now. `None` if the desktop has no such setting or it
/// cannot be read.
pub fn current(host: &Host<'_>, kind: Kind) -> Option<String> {
    let (schema, key) = kind.keys()[0];
    host.settings
        .read(schema, key)
        .ok()
        .flatten()
        .map(|s| s.effective)
}

/// Why a choice was not accepted. Nothing was planned.
#[derive(Debug)]
pub enum Rejected {
    /// Not among the installed themes or pictures, and not a readable
    /// picture file in the home folder.
    NotInstalled,
    /// Not something a font setting can hold.
    NotAFont,
    Engine(Error),
}

/// A font as the desktop writes it: a family, and a size at the end.
fn font_like(text: &str) -> bool {
    let size = text.rsplit(' ').next().unwrap_or("");
    text.len() <= 120
        && !text.chars().any(char::is_control)
        && text.contains(' ')
        && size
            .parse::<f32>()
            .is_ok_and(|s| (1.0..=200.0).contains(&s))
}

/// The value to write for what the user asked for.
fn resolve(host: &Host<'_>, env: Env<'_>, kind: Kind, wanted: &str) -> Result<String, Rejected> {
    if kind.is_font() {
        return if font_like(wanted) {
            Ok(wanted.to_owned())
        } else {
            Err(Rejected::NotAFont)
        };
    }
    // By value or by the name shown, among what was found: never a path
    // put together from what was typed.
    if let Some(found) = available(host, env, kind)
        .into_iter()
        .find(|c| c.value == wanted || c.name == wanted)
    {
        return Ok(found.value);
    }
    // A picture of the user's own, named by its full path.
    let path = Path::new(wanted);
    if kind == Kind::Wallpaper
        && path.is_absolute()
        && is_picture(path)
        && matches!(host.fs.kind(path), Ok(Some(FileKind::File)))
        && host
            .fs
            .canonicalize(path)
            .is_ok_and(|real| path_str(&real) == wanted)
    {
        return Ok(file_uri(path));
    }
    Err(Rejected::NotInstalled)
}

/// What choosing `wanted` for `kind` would do; `None` goes back to the
/// desktop's default. Reads only.
pub fn plan_select(
    host: &Host<'_>,
    env: Env<'_>,
    kind: Kind,
    wanted: Option<&str>,
) -> Result<Plan, Rejected> {
    let value = match wanted {
        Some(wanted) => Some(resolve(host, env, kind, wanted)?),
        None => None,
    };
    let state = State::load(host).map_err(|e| Rejected::Engine(Error::Store(e)))?;
    let mut target = TargetPlan::new(INSTANCE);
    for (schema, key) in kind.keys() {
        match host.settings.read(schema, key) {
            Err(SettingsError::Unavailable(why)) => {
                target = TargetPlan::skip(INSTANCE, Skip::SettingsUnavailable(why));
                break;
            }
            Err(e) => {
                target = TargetPlan::skip(INSTANCE, Skip::SettingsUnavailable(format!("{e:?}")));
                break;
            }
            // An older desktop without, say, the dark wallpaper key.
            Ok(None) => {}
            Ok(Some(now))
                if now.user == value
                    || (value.is_some() && Some(&now.effective) == value.as_ref()) => {}
            Ok(Some(now)) if !now.writable => target.notes.push(Note::SettingLocked {
                key: (*key).to_owned(),
            }),
            Ok(Some(now)) => target.changes.push(Change::Setting {
                schema: (*schema).to_owned(),
                key: (*key).to_owned(),
                value: value.clone(),
                pre: now.user,
            }),
        }
    }
    Ok(Plan {
        cause: Cause::Select,
        theme: state.applied,
        undo_of: None,
        scheduled: false,
        targets: vec![target],
    })
}

/// True if a theme of this name is installed: how a theme file's
/// suggestion is matched, by name and nothing else.
pub fn installed(host: &Host<'_>, env: Env<'_>, kind: Kind, name: &str) -> bool {
    available(host, env, kind).iter().any(|c| c.value == name)
}
