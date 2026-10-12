//! The themes available on this machine: the bundled ones and the user's.

use velatura_theme::model::{Code, Diagnostic, Invalid, MAX_FILE_BYTES};
use velatura_theme::{Theme, bundled, parse_theme};

use crate::host::{Host, Kind};

#[derive(Clone, Debug)]
pub struct Entry {
    pub theme: Theme,
    pub bundled: bool,
    pub warnings: Vec<Diagnostic>,
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
                    });
                }
            }
        }
    }
    (entries, rejected)
}
