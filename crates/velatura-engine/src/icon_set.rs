//! Recoloured icons: a small icon theme of Velatura's own, made from the
//! one in use, holding only the icons that carry that theme's accent, with
//! the theme's accent in their place. It names the one it was made from as
//! the theme to look in for everything else, so every other icon is the
//! original.
//!
//! The set is ordinary files of Velatura's own, planned and undone like
//! any others. Its folder's name changes with what it holds, because GTK
//! loads an icon theme again only when the name it is given changes.

use std::path::{Path, PathBuf};

use velatura_theme::editor::slug;
use velatura_theme::icons::{self, Folder, PixelRecolor, Recolor};
use velatura_theme::png;
use velatura_theme::{Resolved, sha256_hex};

use crate::host::{Host, Kind};
use crate::plan::{Change, Note, Skip, TargetPlan};
use crate::store::{State, path_str, read_opt};
use crate::targets::{Env, INTERFACE_SCHEMA, data_dirs, inside_home};

pub const ICONS: &str = "icons:recolor";

/// What a set may hold at most. Icon themes that would need more are
/// left out rather than copied in bulk.
const MAX_ICONS: usize = 20_000;
const MAX_BYTES: usize = 48 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 512 * 1024;
const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// Folders that hold icon themes, in the order GTK looks in them.
pub fn theme_dirs(host: &Host<'_>, env: Env<'_>) -> Vec<PathBuf> {
    let mut dirs = vec![host.dirs.home.join(".icons")];
    dirs.extend(data_dirs(host, env).into_iter().map(|d| d.join("icons")));
    dirs
}

/// Where Velatura's own sets go.
pub fn sets_dir(host: &Host<'_>) -> PathBuf {
    host.dirs.data.join("icons")
}

fn find(host: &Host<'_>, env: Env<'_>, name: &str) -> Option<PathBuf> {
    theme_dirs(host, env)
        .into_iter()
        .map(|d| d.join(name))
        .find(|d| {
            matches!(
                host.fs.kind(&d.join("index.theme")),
                Ok(Some(Kind::File | Kind::Symlink))
            )
        })
}

pub(crate) fn read_capped(host: &Host<'_>, path: &Path, cap: usize) -> Option<Vec<u8>> {
    read_opt(host, path)
        .ok()
        .flatten()
        .filter(|b| b.len() <= cap)
}

/// The icon theme a set of Velatura's was made from, if `name` is one.
pub fn made_from(host: &Host<'_>, name: &str) -> Option<String> {
    if !name.starts_with("Velatura-") {
        return None;
    }
    let index = read_capped(
        host,
        &sets_dir(host).join(name).join("index.theme"),
        MAX_INDEX_BYTES,
    )?;
    icons::base_of(&String::from_utf8_lossy(&index))
}

/// How an icon theme's colour is moved: in drawings, colour by colour; in
/// pictures, pixel by pixel.
enum Way {
    Drawn(Recolor),
    Pictures(PixelRecolor),
}

/// The files of a set, ready to be written.
struct Built {
    name: String,
    files: Vec<(String, Vec<u8>)>,
}

/// Work out the set for `base` and `theme`. With `full`, every icon is
/// read and recoloured; without, only what decides the set's name is.
fn build(
    host: &Host<'_>,
    base_dir: &Path,
    base: &str,
    theme: &Resolved<'_>,
    full: bool,
) -> Result<Built, Skip> {
    let unusable = || Skip::IconsNotRecolorable(base.to_owned());
    let index_bytes =
        read_capped(host, &base_dir.join("index.theme"), MAX_INDEX_BYTES).ok_or_else(unusable)?;
    let index = icons::parse_index(&String::from_utf8_lossy(&index_bytes));
    let folders: Vec<&Folder> = index.folders.iter().filter(|f| f.recolored()).collect();

    // The accent is found in the folder icons: drawn ones where there
    // are, pictures otherwise.
    let found = |ext: &str| {
        let mut out = Vec::new();
        for f in folders.iter().filter(|f| f.context == "Places") {
            for name in ["folder", "folder-documents", "folder-download"] {
                let path = base_dir.join(&f.name).join(format!("{name}.{ext}"));
                if let Some(bytes) = read_capped(host, &path, MAX_ICON_BYTES) {
                    out.push(bytes);
                }
            }
        }
        out
    };
    let accent = theme.colors.ui.accent;
    let mut folder_icons = found("svg");
    let refs: Vec<&[u8]> = folder_icons.iter().map(Vec::as_slice).collect();
    let way = match Recolor::new(&refs, accent) {
        Some(r) => Way::Drawn(r),
        None => {
            folder_icons = found("png");
            let pictures: Vec<png::Image> =
                folder_icons.iter().filter_map(|b| png::decode(b)).collect();
            let refs: Vec<&png::Image> = pictures.iter().collect();
            Way::Pictures(PixelRecolor::new(&refs, accent).ok_or_else(unusable)?)
        }
    };
    let recipe = match &way {
        Way::Drawn(r) => r.recipe(),
        Way::Pictures(r) => r.recipe(),
    };

    let mut key = format!("{}\n{base}\n{recipe}\n", icons::FORMAT);
    key.push_str(&sha256_hex(&index_bytes));
    for svg in &folder_icons {
        key.push_str(&sha256_hex(svg));
    }
    // An icon theme whose name starts like a cursor set's is told apart.
    let short = match slug(base) {
        s if s.is_empty() => "icons".to_owned(),
        s if s.starts_with("cursors") => format!("icons-{s}"),
        s => s,
    };
    let name = format!("Velatura-{}-{}", short, &sha256_hex(key.as_bytes())[..10]);
    let mut built = Built {
        name,
        files: Vec::new(),
    };
    if !full {
        return Ok(built);
    }

    let mut used: Vec<&Folder> = Vec::new();
    let mut bytes = 0;
    for f in &folders {
        let dir = base_dir.join(&f.name);
        let mut any = false;
        // A theme drawn in pictures can have drawn icons too, as for the
        // top bar; they are moved by the same family.
        let wanted = |n: &str| match way {
            Way::Drawn(_) => n.ends_with(".svg"),
            Way::Pictures(_) => n.ends_with(".png") || n.ends_with(".svg"),
        };
        for entry in host.fs.list(&dir).unwrap_or_default() {
            let Some(file) = entry
                .to_str()
                .filter(|n| wanted(n) && !n.starts_with('.') && !icons::symbolic(n))
            else {
                continue;
            };
            let Some(content) = read_capped(host, &dir.join(file), MAX_ICON_BYTES) else {
                continue;
            };
            let out = match &way {
                Way::Drawn(r) => r.apply(&content),
                Way::Pictures(r) if file.ends_with(".svg") => r.apply_svg(&content),
                Way::Pictures(r) => png::decode(&content)
                    .and_then(|mut image| r.apply(&mut image).then(|| png::encode(&image))),
            };
            if let Some(out) = out {
                bytes += out.len();
                built.files.push((format!("{}/{file}", f.name), out));
                any = true;
                if built.files.len() > MAX_ICONS || bytes > MAX_BYTES {
                    return Err(Skip::IconsTooMany(base.to_owned()));
                }
            }
        }
        if any {
            used.push(f);
        }
    }
    if built.files.is_empty() {
        return Err(unusable());
    }
    built.files.insert(
        0,
        (
            "index.theme".to_owned(),
            icons::index_theme(base, &used).into_bytes(),
        ),
    );
    Ok(built)
}

/// Files and folders under `root` that Velatura wrote, with what it wrote.
pub(crate) fn ours_under<'a>(state: &'a State, root: &Path) -> Vec<(PathBuf, &'a String)> {
    state
        .owned
        .iter()
        .map(|(p, h)| (PathBuf::from(p), h))
        .filter(|(p, _)| p.starts_with(root))
        .collect()
}

/// The folders between each file and `root`, `root` included, deepest
/// first, so each is empty by the time its turn comes.
pub(crate) fn folders_up_to(files: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = files
        .iter()
        .flat_map(|f| f.ancestors().skip(1).take_while(|d| d.starts_with(root)))
        .map(Path::to_path_buf)
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    dirs
}

/// The icon theme in use, recoloured. `clear_old` takes away the sets
/// Velatura made for earlier themes, which only a run with someone present
/// does. With `probe`, a set that is not there yet is only named, not
/// made: the plan says that it would be, and is never carried out.
pub fn plan_icons(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    clear_old: bool,
    probe: bool,
) -> TargetPlan {
    let skip = |why| TargetPlan::skip(ICONS, why);
    if crate::targets::high_contrast(host) {
        return skip(Skip::HighContrast);
    }
    let now = match host.settings.read(INTERFACE_SCHEMA, "icon-theme") {
        Ok(Some(now)) => now,
        Ok(None) => {
            return skip(Skip::SettingsUnavailable(format!(
                "{INTERFACE_SCHEMA} icon-theme"
            )));
        }
        Err(e) => return skip(Skip::SettingsUnavailable(e.to_string())),
    };
    let base = made_from(host, &now.effective).unwrap_or_else(|| now.effective.clone());
    if !icons::plain_name(&base) {
        return skip(Skip::IconsNotRecolorable(base));
    }
    let Some(base_dir) = find(host, env, &base) else {
        return skip(Skip::IconsNotRecolorable(base));
    };
    let root = sets_dir(host);
    if !inside_home(host, &host.dirs.data) {
        return skip(Skip::OutsideHome(root));
    }
    let named = match build(host, &base_dir, &base, theme, false) {
        Ok(b) => b,
        Err(why) => return skip(why),
    };
    let set = root.join(&named.name);
    let mut plan = TargetPlan::new(ICONS);
    let mine = ours_under(state, &set);
    match host.fs.kind(&set) {
        // Made already for these colours; nothing to write.
        Ok(Some(Kind::Dir)) if !mine.is_empty() => {}
        Ok(None) if probe => plan.changes.push(Change::CreateDir { path: set.clone() }),
        Ok(None) => {
            let built = match build(host, &base_dir, &base, theme, true) {
                Ok(b) => b,
                Err(why) => return skip(why),
            };
            if !crate::targets::exists(host, &root) {
                plan.changes.push(Change::CreateDir { path: root.clone() });
            }
            let mut made: Vec<PathBuf> = vec![set.clone()];
            plan.changes.push(Change::CreateDir { path: set.clone() });
            for (rel, content) in built.files {
                let path = set.join(&rel);
                for dir in path
                    .ancestors()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    if dir.starts_with(&set) && !made.iter().any(|m| m == dir) {
                        made.push(dir.to_path_buf());
                        plan.changes.push(Change::CreateDir {
                            path: dir.to_path_buf(),
                        });
                    }
                }
                plan.changes.push(Change::Owned {
                    path,
                    content: Some(content),
                    pre: None,
                });
            }
        }
        _ => return skip(Skip::ForeignFile(set)),
    }

    if clear_old {
        clear_other_sets(
            host,
            state,
            &set,
            &recent_sets(host, "icon-theme"),
            &|name| !is_cursor_set(name),
            &mut plan,
        );
    }

    match crate::targets::set_text(
        host,
        (INTERFACE_SCHEMA, "icon-theme"),
        &named.name,
        &mut plan,
    ) {
        Ok(()) => {
            if !plan.changes.is_empty() {
                plan.notes.push(Note::IconsReopen);
            }
            plan
        }
        Err(why) => skip(why),
    }
}

/// How many sets of each kind are kept besides the one in use, so going
/// back to a recent theme uses its set again instead of making it anew.
pub const KEPT_SETS: usize = 3;

/// The names of Velatura's sets that `key` (`icon-theme` or
/// `cursor-theme`) was set to or from, most recent first: read from the
/// backups. The one in use may be among them.
pub(crate) fn recent_sets(host: &Host<'_>, key: &str) -> Vec<String> {
    let Ok((all, _)) = crate::store::manifests(host) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for manifest in all.iter().rev() {
        for record in manifest.records.iter().rev() {
            let crate::store::Record::Setting {
                key: k, pre, post, ..
            } = record
            else {
                continue;
            };
            if k != key {
                continue;
            }
            for name in [post, pre].into_iter().flatten() {
                if name.starts_with("Velatura-") && !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
    }
    out
}

/// Sets of one kind Velatura made for earlier colours, as far as they are
/// as written, taken away: all but `keep`, the one in use now, and the
/// [`KEPT_SETS`] used last before it among `recent`, which are kept to be
/// used again. `kind` tells a set's folder name of that kind from the
/// other's.
pub(crate) fn clear_other_sets(
    host: &Host<'_>,
    state: &State,
    keep: &Path,
    recent: &[String],
    kind: &dyn Fn(&str) -> bool,
    plan: &mut TargetPlan,
) {
    let root = sets_dir(host);
    // Counted without the one in use, which is the most recent when the
    // same colours are planned again: else one set fewer would be kept
    // than when they were applied.
    let in_use = keep.file_name().and_then(|n| n.to_str());
    let recent: Vec<&String> = recent
        .iter()
        .filter(|r| Some(r.as_str()) != in_use)
        .take(KEPT_SETS)
        .collect();
    // Sets made for earlier colours, as far as they are as written.
    let mut others: std::collections::BTreeMap<PathBuf, Vec<(PathBuf, &String)>> =
        std::collections::BTreeMap::new();
    for (path, hash) in ours_under(state, &root) {
        let Ok(rest) = path.strip_prefix(&root) else {
            continue;
        };
        let Some(top) = rest.components().next() else {
            continue;
        };
        let top = root.join(top);
        let named = top.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.starts_with("Velatura-") && kind(n) && !recent.iter().any(|r| *r == n)
        });
        if top != keep && named {
            others.entry(top).or_default().push((path, hash));
        }
    }
    for (top, files) in others {
        let mut gone = Vec::new();
        for (path, written) in files {
            let now = read_opt(host, &path).ok().flatten();
            match now.as_deref().map(sha256_hex) {
                Some(hash) if hash == *written => {
                    plan.changes.push(Change::Owned {
                        path: path.clone(),
                        content: None,
                        pre: Some(hash),
                    });
                    gone.push(path);
                }
                Some(_) => plan.notes.push(Note::KeptFile { path }),
                None => {}
            }
        }
        for dir in folders_up_to(&gone, &top) {
            plan.changes.push(Change::RemoveDir { path: dir });
        }
    }
}

/// True for the folder name of a cursor set: icon sets are never named so.
pub fn is_cursor_set(name: &str) -> bool {
    name.starts_with(crate::cursor_set::PREFIX)
}

/// The number of icons in a set a list of changes writes or removes, by
/// the set's folder: for saying it in one line instead of one per icon.
pub fn sets_in(changes: &[Change]) -> Vec<(PathBuf, usize, bool)> {
    let mut out: Vec<(PathBuf, usize, bool)> = Vec::new();
    for c in changes {
        let (path, written) = match c {
            Change::Owned { path, content, .. } => (path, content.is_some()),
            _ => continue,
        };
        let Some(top) = set_of(path) else { continue };
        if path_str(path).ends_with("/index.theme") {
            continue;
        }
        match out.iter_mut().find(|(t, _, w)| *t == top && *w == written) {
            Some(entry) => entry.1 += 1,
            None => out.push((top, 1, written)),
        }
    }
    out
}

/// The set folder a path lies in, if it lies in one: `…/icons/Velatura-…`.
pub fn set_of(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|a| {
            a.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("Velatura-"))
                && a.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("icons"))
        })
        .map(Path::to_path_buf)
}
