//! The themes available on this machine: the bundled ones and the user's.

use std::path::{Path, PathBuf};

use velatura_theme::import::{self, Format, Incomplete};
use velatura_theme::model::{Code, Diagnostic, Invalid, MAX_FILE_BYTES, is_slug, to_toml};
use velatura_theme::{Color, Loaded, Theme, bundled, parse_theme};

use crate::host::{Host, Kind};
use crate::store::{ensure_dir, read_opt};

#[derive(Clone, Debug)]
pub struct Entry {
    pub theme: Theme,
    pub bundled: bool,
    pub warnings: Vec<Diagnostic>,
    /// The file a theme of the user's was read from.
    pub file: Option<PathBuf>,
}

/// A file in the themes folder that was not loaded, and why.
#[derive(Clone, Debug)]
pub struct Rejected {
    pub file: String,
    pub why: Why,
}

#[derive(Clone, Debug)]
pub enum Why {
    Unreadable,
    Invalid(Invalid),
    /// Its id belongs to a bundled theme, or to a file listed before it.
    IdTaken(String),
}

/// Bundled themes first, then the user's by file name.
pub fn load(host: &Host<'_>) -> (Vec<Entry>, Vec<Rejected>) {
    let mut entries: Vec<Entry> = bundled::themes()
        .into_iter()
        .map(|theme| Entry {
            theme,
            bundled: true,
            warnings: Vec::new(),
            file: None,
        })
        .collect();
    let mut rejected = Vec::new();
    let dir = host.themes_dir();
    for name in host.fs.list(&dir).unwrap_or_default() {
        let file = name.to_string_lossy().into_owned();
        if !file.ends_with(".toml") {
            continue;
        }
        let path = dir.join(&name);
        let mut reject = |why| {
            rejected.push(Rejected {
                file: velatura_theme::model::clean_text(&file, 120),
                why,
            });
        };
        if !matches!(host.fs.kind(&path), Ok(Some(Kind::File | Kind::Symlink))) {
            continue;
        }
        let Ok(bytes) = host.fs.read(&path) else {
            reject(Why::Unreadable);
            continue;
        };
        if bytes.len() > MAX_FILE_BYTES {
            reject(Why::Invalid(Invalid(vec![Diagnostic {
                code: Code::TooLarge,
                at: String::new(),
            }])));
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            reject(Why::Unreadable);
            continue;
        };
        match parse_theme(&text) {
            Err(invalid) => reject(Why::Invalid(invalid)),
            Ok(loaded) => {
                if entries.iter().any(|e| e.theme.id == loaded.theme.id) {
                    reject(Why::IdTaken(loaded.theme.id));
                } else {
                    entries.push(Entry {
                        theme: loaded.theme,
                        bundled: false,
                        warnings: loaded.warnings,
                        file: Some(path.clone()),
                    });
                }
            }
        }
    }
    (entries, rejected)
}

/// Why a theme was not saved, removed or read. Nothing was changed.
#[derive(Debug)]
pub enum Refused {
    /// The id belongs to a bundled theme.
    Reserved(String),
    /// A theme with this id is in the library and `replace` was not given.
    Exists(String),
    /// Not a name a theme can have.
    BadId(String),
    /// No theme of the user's has this id.
    NotFound(String),
    /// A file of this name is in the themes folder and holds something
    /// else, or cannot be read.
    FileInTheWay(PathBuf),
    /// The file to read is too large to be a theme, or is not text.
    NotAThemeFile,
    Invalid(Invalid),
    /// A theme in another format that lacks colours a theme needs.
    Incomplete(Incomplete),
    Io(String),
}

fn io<T>(result: std::io::Result<T>) -> Result<T, Refused> {
    result.map_err(|e| Refused::Io(e.to_string()))
}

/// A file read for importing: the theme, the text to store it as, and the
/// format it was in if that was not Velatura's own.
pub struct Inspected {
    pub loaded: Loaded,
    pub source: String,
    pub from: Option<Format>,
    /// For a file in another format: its colours that can be the accent,
    /// by their names in the file, and which one the theme has.
    pub accents: Vec<(String, Color)>,
    pub accent: usize,
}

impl Inspected {
    /// Give the theme another accent before it is stored. False, with
    /// nothing changed, if that does not leave a valid theme.
    pub fn choose_accent(&mut self, accent: Color) -> bool {
        let Some(theme) = import::with_accent(&self.loaded.theme, accent) else {
            return false;
        };
        self.source = to_toml(&theme);
        self.loaded.theme = theme;
        self.accent = self
            .accents
            .iter()
            .position(|(_, c)| *c == accent)
            .unwrap_or(self.accents.len());
        true
    }
}

/// Read a file someone wants to import and check it as a theme: one of
/// Velatura's own, or a base16, base24 or Kitty one, which becomes a theme
/// with every colour written out. The file is untrusted: size-capped
/// before it is parsed, and never run.
pub fn inspect(host: &Host<'_>, path: &Path) -> Result<Inspected, Refused> {
    if !matches!(host.fs.kind(path), Ok(Some(Kind::File | Kind::Symlink))) {
        return Err(Refused::NotAThemeFile);
    }
    let bytes = io(host.fs.read(path))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Refused::NotAThemeFile);
    }
    let text = String::from_utf8(bytes).map_err(|_| Refused::NotAThemeFile)?;
    let invalid = match parse_theme(&text) {
        Ok(loaded) => {
            return Ok(Inspected {
                loaded,
                source: text,
                from: None,
                accents: Vec::new(),
                accent: 0,
            });
        }
        Err(invalid) => invalid,
    };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    match import::read(&text, &stem) {
        Some(Ok(found)) => Ok(Inspected {
            source: to_toml(&found.loaded.theme),
            loaded: found.loaded,
            from: Some(found.format),
            accents: found.accents,
            accent: found.accent,
        }),
        Some(Err(incomplete)) => Err(Refused::Incomplete(incomplete)),
        None => Err(Refused::Invalid(invalid)),
    }
}

/// True if a theme of this id is in the library, bundled or the user's.
pub fn taken(host: &Host<'_>, id: &str) -> bool {
    load(host).0.iter().any(|e| e.theme.id == id)
}

/// Put a theme in the user's library. `source` is the text it was read
/// from: if it still describes this theme it is stored as it is, comments
/// and all; otherwise the theme is written out in full. With `replace`, a
/// theme of the user's with the same id is replaced; a hand-written file
/// that is replaced is first kept beside it, once, as `<name>.orig`.
pub fn save(
    host: &Host<'_>,
    theme: &Theme,
    source: Option<&str>,
    replace: bool,
) -> Result<PathBuf, Refused> {
    if !is_slug(&theme.id) {
        return Err(Refused::BadId(theme.id.clone()));
    }
    if bundled::is_reserved(&theme.id) {
        return Err(Refused::Reserved(theme.id.clone()));
    }
    let (entries, _) = load(host);
    let existing = entries.iter().find(|e| e.theme.id == theme.id);
    if existing.is_some() && !replace {
        return Err(Refused::Exists(theme.id.clone()));
    }
    let dir = host.themes_dir();
    let path = existing
        .and_then(|e| e.file.clone())
        .unwrap_or_else(|| dir.join(format!("{}.toml", theme.id)));
    let text = source
        .filter(|text| parse_theme(text).is_ok_and(|l| l.theme == *theme))
        .map_or_else(|| to_toml(theme), str::to_owned);
    // What is written must read back as the theme that was meant.
    match parse_theme(&text) {
        Ok(_) if text.len() <= MAX_FILE_BYTES => {}
        Ok(_) => return Err(Refused::NotAThemeFile),
        Err(invalid) => return Err(Refused::Invalid(invalid)),
    }
    io(ensure_dir(host, &dir))?;
    match io(read_opt(host, &path))? {
        None => {}
        Some(old) if existing.is_some() => {
            // Written by hand, with comments or keys this build does not
            // write: not lost to an edit made in the app.
            let old_text = String::from_utf8_lossy(&old);
            let plain = parse_theme(&old_text).is_ok_and(|l| to_toml(&l.theme) == old_text);
            let keep = path.with_extension("toml.orig");
            if !plain && old != text.as_bytes() && io(host.fs.kind(&keep))?.is_none() {
                io(host.fs.write_atomic(&keep, &old, 0o644))?;
            }
        }
        // A file of that name that is not this theme: rejected by the
        // library for some reason, and not Velatura's to overwrite.
        Some(_) => return Err(Refused::FileInTheWay(path)),
    }
    io(host.fs.write_atomic(&path, text.as_bytes(), 0o644))?;
    Ok(path)
}

/// Take a theme of the user's out of the library. Returns the text of its
/// file, which is all that is needed to put it back with [`save`].
pub fn remove(host: &Host<'_>, id: &str) -> Result<String, Refused> {
    let (entries, _) = load(host);
    let Some(file) = entries
        .iter()
        .find(|e| e.theme.id == id && !e.bundled)
        .and_then(|e| e.file.clone())
    else {
        return Err(if bundled::is_reserved(id) {
            Refused::Reserved(id.to_owned())
        } else {
            Refused::NotFound(id.to_owned())
        });
    };
    let text = String::from_utf8(io(host.fs.read(&file))?).map_err(|_| Refused::NotAThemeFile)?;
    io(host.fs.remove_file(&file))?;
    Ok(text)
}

/// The text of a theme's file, to give to someone else: the user's file
/// as it is, or the bundled one.
pub fn source(host: &Host<'_>, id: &str) -> Result<String, Refused> {
    if let Some((_, text)) = bundled::SOURCES.iter().find(|(bundled, _)| *bundled == id) {
        return Ok((*text).to_owned());
    }
    let (entries, _) = load(host);
    let file = entries
        .iter()
        .find(|e| e.theme.id == id)
        .and_then(|e| e.file.clone())
        .ok_or_else(|| Refused::NotFound(id.to_owned()))?;
    String::from_utf8(io(host.fs.read(&file))?).map_err(|_| Refused::NotAThemeFile)
}

/// Write a theme's file where the user asked for it. An existing file is
/// replaced only with `overwrite`.
pub fn export(host: &Host<'_>, id: &str, to: &Path, overwrite: bool) -> Result<(), Refused> {
    let text = source(host, id)?;
    if !overwrite && io(host.fs.kind(to))?.is_some() {
        return Err(Refused::FileInTheWay(to.to_owned()));
    }
    io(host.fs.write_atomic(to, text.as_bytes(), 0o644))
}
