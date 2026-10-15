//! Templates of the user's: one file each in Velatura's `templates`
//! folder, naming where its output goes and holding its text.
//!
//! ```toml
//! schema = 1
//! name = "Foot"
//! output = "~/.config/foot/colors.ini"
//! template = '''
//! [colors]
//! background = {{bg|bare}}
//! '''
//! ```
//!
//! A template's text is written out as it is, so it is as trustworthy as
//! a settings file from wherever it came from. What Velatura decides is
//! where it may go: a file it creates and owns, under the settings folder
//! (`~/.config`) or the data folder (`~/.local/share`), never in a place
//! from which programs are started, never one named like Velatura's own,
//! and never over a file it did not make. The first write of each template
//! is made only when agreed to.

use std::path::{Component, Path, PathBuf};

use velatura_theme::Resolved;
use velatura_theme::model::{clean_text, is_slug};
use velatura_theme::template::{self, Template};

use crate::host::Host;
use crate::plan::{Skip, TargetPlan};
use crate::store::{State, read_opt};
use crate::targets::{first_time, own, place};

/// Instances of templates are `template:<id>`.
pub const PREFIX: &str = "template:";

/// The largest template file read: its text, and a few short keys.
const MAX_FILE_BYTES: usize = template::MAX_BYTES + 4096;

/// Folders under the settings folder a template may not write into:
/// programs are started from them.
const NOT_IN_CONFIG: [&str; 4] = ["autostart", "systemd", "environment.d", "velatura"];
/// The same under the data folder: app launchers, services the session
/// bus starts, Shell extensions, file manager scripts, sandbox overrides,
/// and Velatura's own.
const NOT_IN_DATA: [&str; 8] = [
    "applications",
    "dbus-1",
    "systemd",
    "gnome-shell",
    "nautilus",
    "flatpak",
    "kservices5",
    "velatura",
];

/// One template, read and checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file's name without `.toml`: a slug.
    pub id: String,
    /// What it is called, for showing: cleaned, and short.
    pub name: String,
    /// Where its output goes, as the file says it (`~/.config/…`).
    pub written: String,
    pub output: PathBuf,
    pub template: Template,
}

impl Entry {
    pub fn instance(&self) -> String {
        format!("{PREFIX}{}", self.id)
    }
}

/// Why a template file cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    TooLarge,
    /// Not TOML, or not text: the parser's message, cleaned.
    NotToml(String),
    /// Written for a newer Velatura.
    NewerSchema,
    MissingKey(&'static str),
    /// The output is not under `~/.config/` or `~/.local/share/`, or
    /// leaves them, or has a name that cannot be a file's.
    OutputPlace(String),
    /// The output is inside a folder programs are started from, or named
    /// like a file of Velatura's own.
    OutputForbidden(String),
    /// Another template writes the same file.
    OutputTaken(String),
    Template(template::Problem),
}

/// The folder templates are kept in.
pub fn dir(host: &Host<'_>) -> PathBuf {
    host.data_dir().join("templates")
}

/// Where a template's output goes, from the way the file writes it.
pub fn output_path(host: &Host<'_>, written: &str) -> Result<PathBuf, Invalid> {
    let place = || Invalid::OutputPlace(clean_text(written, 120));
    let (base, rest, forbidden): (&Path, &str, &[&str]) =
        if let Some(rest) = written.strip_prefix("~/.config/") {
            (&host.dirs.config, rest, &NOT_IN_CONFIG)
        } else if let Some(rest) = written.strip_prefix("~/.local/share/") {
            (&host.dirs.data, rest, &NOT_IN_DATA)
        } else {
            return Err(place());
        };
    let rest = Path::new(rest);
    let ok = written.len() <= 1024
        && !written.chars().any(char::is_control)
        && !written.ends_with('/')
        && rest.components().count() > 0
        && rest.components().all(|c| match c {
            Component::Normal(part) => part.len() <= 255,
            _ => false,
        });
    if !ok {
        return Err(place());
    }
    let first = rest
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let velatura_like = rest.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .starts_with("velatura")
    });
    if forbidden.contains(&first.as_str()) || velatura_like {
        return Err(Invalid::OutputForbidden(clean_text(written, 120)));
    }
    Ok(base.join(rest))
}

/// Read one template file's text, with `id` from its name.
pub fn read(host: &Host<'_>, id: &str, text: &str) -> Result<Entry, Invalid> {
    if text.len() > MAX_FILE_BYTES {
        return Err(Invalid::TooLarge);
    }
    let table: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| Invalid::NotToml(clean_text(e.message(), 200)))?;
    match table.get("schema").map(toml::Value::as_integer) {
        None | Some(Some(1)) => {}
        Some(Some(n)) if n > 1 => return Err(Invalid::NewerSchema),
        Some(_) => return Err(Invalid::MissingKey("schema")),
    }
    let text_of = |key: &'static str| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .ok_or(Invalid::MissingKey(key))
    };
    let written = text_of("output")?;
    let output = output_path(host, written)?;
    let template = template::parse(text_of("template")?).map_err(Invalid::Template)?;
    let name = table
        .get("name")
        .and_then(toml::Value::as_str)
        .map(|n| clean_text(n, 60))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| id.to_owned());
    Ok(Entry {
        id: id.to_owned(),
        name,
        written: written.to_owned(),
        output,
        template,
    })
}

/// Every template in the folder, by id, each read or with why it cannot
/// be used.
pub fn load(host: &Host<'_>) -> Vec<(String, Result<Entry, Invalid>)> {
    let folder = dir(host);
    let mut names: Vec<String> = host
        .fs
        .list(&folder)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.to_str().map(str::to_owned))
        .filter_map(|n| n.strip_suffix(".toml").map(str::to_owned))
        .filter(|id| is_slug(id))
        .collect();
    names.sort();
    let mut out: Vec<(String, Result<Entry, Invalid>)> = Vec::new();
    for id in names {
        let path = folder.join(format!("{id}.toml"));
        let entry = match read_opt(host, &path) {
            Ok(Some(bytes)) if bytes.len() > MAX_FILE_BYTES => Err(Invalid::TooLarge),
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => read(host, &id, &text),
                Err(_) => Err(Invalid::NotToml("not text".to_owned())),
            },
            _ => continue,
        };
        // Two templates that write one file would undo each other.
        let entry = entry.and_then(|e| {
            let taken = out
                .iter()
                .any(|(_, other)| other.as_ref().is_ok_and(|o| o.output == e.output));
            if taken {
                Err(Invalid::OutputTaken(e.written))
            } else {
                Ok(e)
            }
        });
        out.push((id, entry));
    }
    out
}

/// The output of one template for `theme`: a file Velatura creates and
/// owns, in folders it makes where they are missing.
pub fn plan_template(
    host: &Host<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
    entry: &Entry,
) -> TargetPlan {
    let instance = entry.instance();
    // The folders between the settings or data folder and the file.
    let base = if entry.output.starts_with(&host.dirs.config) {
        &host.dirs.config
    } else {
        &host.dirs.data
    };
    let levels: Vec<PathBuf> = entry
        .output
        .ancestors()
        .skip(1)
        .take_while(|d| d.starts_with(base))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let levels: Vec<&Path> = levels.iter().map(PathBuf::as_path).collect();
    let mut plan = TargetPlan::new(&instance);
    let mut fill = || {
        place(host, &levels, &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            entry.output.clone(),
            entry.template.render(theme).as_bytes(),
            &mut plan,
        )
    };
    match fill() {
        Ok(()) => {
            if !plan.changes.is_empty() {
                plan.needs_consent = first_time(state, &instance, &entry.output);
            }
            plan
        }
        Err(why) => TargetPlan::skip(&instance, why),
    }
}

/// The plan for a template that cannot be used.
pub fn invalid(id: &str, why: Invalid) -> TargetPlan {
    TargetPlan::skip(&format!("{PREFIX}{id}"), Skip::TemplateInvalid(why))
}

/// Why a template was not added or removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The id is not a slug: lower-case letters, digits and dashes.
    BadId(String),
    Invalid(Invalid),
    /// A template with this id is there already.
    Exists(String),
    NotFound(String),
    Io(String),
}

fn io(e: &std::io::Error) -> Refused {
    Refused::Io(clean_text(&e.to_string(), 200))
}

/// Check a template's text and put it into the templates folder as
/// `<id>.toml`. With `replace`, one with the same id is replaced.
pub fn add(host: &Host<'_>, id: &str, text: &str, replace: bool) -> Result<Entry, Refused> {
    if !is_slug(id) {
        return Err(Refused::BadId(clean_text(id, 64)));
    }
    let entry = read(host, id, text).map_err(Refused::Invalid)?;
    let path = dir(host).join(format!("{id}.toml"));
    if !replace && crate::targets::exists(host, &path) {
        return Err(Refused::Exists(id.to_owned()));
    }
    let taken = load(host)
        .into_iter()
        .filter(|(other, _)| other != id)
        .any(|(_, other)| other.is_ok_and(|o| o.output == entry.output));
    if taken {
        return Err(Refused::Invalid(Invalid::OutputTaken(entry.written)));
    }
    crate::store::ensure_dir(host, &dir(host)).map_err(|e| io(&e))?;
    host.fs
        .write_atomic(&path, text.as_bytes(), 0o644)
        .map_err(|e| io(&e))?;
    Ok(entry)
}

/// Take a template out of the folder, keeping a copy first, so it can be
/// added again. The file it wrote stays, as a part's files do when it is
/// turned off; Undo or Remove Everything takes it away. Gives where the
/// copy is.
pub fn remove(host: &Host<'_>, id: &str) -> Result<PathBuf, Refused> {
    if !is_slug(id) {
        return Err(Refused::BadId(clean_text(id, 64)));
    }
    let path = dir(host).join(format!("{id}.toml"));
    let Some(text) = read_opt(host, &path).map_err(|e| io(&e))? else {
        return Err(Refused::NotFound(id.to_owned()));
    };
    let kept_dir = host.data_dir().join("removed").join("templates");
    let kept = kept_dir.join(format!("{id}.toml"));
    crate::store::ensure_dir(host, &kept_dir).map_err(|e| io(&e))?;
    host.fs
        .write_atomic(&kept, &text, 0o644)
        .map_err(|e| io(&e))?;
    host.fs.remove_file(&path).map_err(|e| io(&e))?;
    Ok(kept)
}
