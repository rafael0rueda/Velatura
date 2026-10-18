//! Recoloured cursors: a small cursor theme of Velatura's own, made from
//! the one in use, with every cursor in the theme's colours. It names the
//! one it was made from as the theme to look in for anything it lacks.
//!
//! Like an icon set, it is ordinary files of Velatura's own, planned and
//! undone like any others, and its folder's name changes with what it
//! holds, so the desktop loads it again when the colours change. A
//! cursor theme gives one picture several names by links; the set holds a
//! copy under each name, because a name it lacked would be looked up in
//! the original and show the original's colours.

use std::path::{Path, PathBuf};

use velatura_theme::cursor::{self, CursorRecolor};
use velatura_theme::editor::slug;
use velatura_theme::{Resolved, sha256_hex};

use crate::host::{Host, Kind};
use crate::icon_set::{
    clear_other_sets, is_cursor_set, ours_under, read_capped, sets_dir, theme_dirs,
};
use crate::plan::{Change, Skip, TargetPlan};
use crate::store::State;
use crate::targets::{Env, INTERFACE_SCHEMA, inside_home};

pub const CURSORS: &str = "cursors:recolor";

/// The start of every cursor set's folder name.
pub const PREFIX: &str = "Velatura-cursors-";

/// What a cursor set may hold at most.
const MAX_CURSORS: usize = 2_000;
const MAX_BYTES: usize = 48 * 1024 * 1024;
const MAX_CURSOR_BYTES: usize = 4 * 1024 * 1024;
const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// The folder of the cursor theme called `name`: one with a `cursors`
/// folder in it.
fn find(host: &Host<'_>, env: Env<'_>, name: &str) -> Option<PathBuf> {
    theme_dirs(host, env)
        .into_iter()
        .map(|d| d.join(name))
        .find(|d| {
            matches!(
                host.fs.kind(&d.join("cursors")),
                Ok(Some(Kind::Dir | Kind::Symlink))
            )
        })
}

/// The cursor theme a set of Velatura's was made from, if `name` is one.
pub fn made_from(host: &Host<'_>, name: &str) -> Option<String> {
    if !is_cursor_set(name) {
        return None;
    }
    let index = read_capped(
        host,
        &sets_dir(host).join(name).join("index.theme"),
        MAX_INDEX_BYTES,
    )?;
    velatura_theme::icons::base_of(&String::from_utf8_lossy(&index))
}

/// A set's files: each one's path inside the set, and its bytes.
type Files = Vec<(String, Vec<u8>)>;

/// The set's name, and with `full` its files.
fn build(
    host: &Host<'_>,
    base_dir: &Path,
    base: &str,
    theme: &Resolved<'_>,
    full: bool,
) -> Result<(String, Files), Skip> {
    let unusable = || Skip::CursorsNotRecolorable(base.to_owned());
    let recolor = CursorRecolor::new(theme);
    let dir = base_dir.join("cursors");
    let names: Vec<String> = host
        .fs
        .list(&dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.to_str().map(str::to_owned))
        .filter(|n| !n.starts_with('.') && velatura_theme::icons::plain_name(n))
        .collect();
    if names.is_empty() || names.len() > MAX_CURSORS {
        return Err(unusable());
    }
    // The name follows the colours and every cursor of the original.
    let mut key = format!("{base}\n{}", recolor.recipe());
    let mut files = Vec::new();
    let mut bytes = 0;
    let mut read = 0;
    for name in &names {
        let Some(original) = read_capped(host, &dir.join(name), MAX_CURSOR_BYTES) else {
            continue;
        };
        if !cursor::is_xcursor(&original) {
            continue;
        }
        read += 1;
        key.push_str(name);
        key.push_str(&sha256_hex(&original));
        if full {
            let out = recolor.apply(&original).ok_or_else(unusable)?;
            bytes += out.len();
            if bytes > MAX_BYTES {
                return Err(unusable());
            }
            files.push((format!("cursors/{name}"), out));
        }
    }
    if read == 0 {
        return Err(unusable());
    }
    let short = slug(base);
    let name = format!(
        "{PREFIX}{}-{}",
        if short.is_empty() { "theme" } else { &short },
        &sha256_hex(key.as_bytes())[..10]
    );
    if full {
        files.insert(
            0,
            (
                "index.theme".to_owned(),
                cursor::index_theme(base).into_bytes(),
            ),
        );
    }
    Ok((name, files))
}

/// The cursor theme in use, recoloured. `clear_old` takes away the sets
/// made for earlier colours, which only a run with someone present does.
/// With `probe`, a set that is not there yet is only named, not made.
pub fn plan_cursors(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    clear_old: bool,
    probe: bool,
) -> TargetPlan {
    let skip = |why| TargetPlan::skip(CURSORS, why);
    if crate::targets::high_contrast(host) {
        return skip(Skip::HighContrast);
    }
    let now = match host.settings.read(INTERFACE_SCHEMA, "cursor-theme") {
        Ok(Some(now)) => now,
        Ok(None) => {
            return skip(Skip::SettingsUnavailable(format!(
                "{INTERFACE_SCHEMA} cursor-theme"
            )));
        }
        Err(e) => return skip(Skip::SettingsUnavailable(e.to_string())),
    };
    let base = made_from(host, &now.effective).unwrap_or_else(|| now.effective.clone());
    if !velatura_theme::icons::plain_name(&base) {
        return skip(Skip::CursorsNotRecolorable(base));
    }
    let Some(base_dir) = find(host, env, &base) else {
        return skip(Skip::CursorsNotRecolorable(base));
    };
    let root = sets_dir(host);
    if !inside_home(host, &host.dirs.data) {
        return skip(Skip::OutsideHome(root));
    }
    let name = match build(host, &base_dir, &base, theme, false) {
        Ok((name, _)) => name,
        Err(why) => return skip(why),
    };
    let set = root.join(&name);
    let mut plan = TargetPlan::new(CURSORS);
    match host.fs.kind(&set) {
        Ok(Some(Kind::Dir)) if !ours_under(state, &set).is_empty() => {}
        Ok(None) if probe => plan.changes.push(Change::CreateDir { path: set.clone() }),
        Ok(None) => {
            let files = match build(host, &base_dir, &base, theme, true) {
                Ok((_, files)) => files,
                Err(why) => return skip(why),
            };
            if !crate::targets::exists(host, &root) {
                plan.changes.push(Change::CreateDir { path: root.clone() });
            }
            plan.changes.push(Change::CreateDir { path: set.clone() });
            plan.changes.push(Change::CreateDir {
                path: set.join("cursors"),
            });
            for (rel, content) in files {
                plan.changes.push(Change::Owned {
                    path: set.join(rel),
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
            &crate::icon_set::recent_sets(host, "cursor-theme"),
            &is_cursor_set,
            &mut plan,
        );
    }
    match crate::targets::set_text(host, (INTERFACE_SCHEMA, "cursor-theme"), &name, &mut plan) {
        Ok(()) => {
            if !plan.changes.is_empty() {
                plan.notes.push(crate::plan::Note::CursorReopen);
            }
            plan
        }
        Err(why) => skip(why),
    }
}
