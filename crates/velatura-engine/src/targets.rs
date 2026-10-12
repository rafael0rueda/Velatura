//! Connectors: where a theme's rendered output goes.
//!
//! A connector only reads. It looks at the machine and returns the changes
//! it would make; the engine carries them out.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use velatura_theme::{Appearance, Resolved, kitty, nearest_accent, sha256_hex};

use crate::block;
use crate::host::{Host, Kind, SettingsError};
use crate::plan::{Change, Note, Skip, TargetPlan};
use crate::store::{State, path_str, read_opt};

/// Environment lookup, supplied by the caller.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub const KITTY: &str = "kitty:default";
pub const APPEARANCE: &str = "gnome:appearance";

/// Every instance this build knows, in the order they are applied.
pub const INSTANCES: [&str; 2] = [KITTY, APPEARANCE];

pub const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";

/// True if `path`, after following links, is inside the home folder.
fn inside_home(host: &Host<'_>, path: &Path) -> bool {
    match (
        host.fs.canonicalize(path),
        host.fs.canonicalize(&host.dirs.home),
    ) {
        (Ok(real), Ok(home)) => real.starts_with(home),
        _ => false,
    }
}

pub fn kitty_dir(host: &Host<'_>, env: Env<'_>) -> PathBuf {
    env("KITTY_CONFIG_DIRECTORY")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| host.dirs.config.join("kitty"))
}

fn on_path(host: &Host<'_>, env: Env<'_>, program: &str) -> bool {
    env("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .any(|dir| host.fs.kind(&dir.join(program)).ok().flatten().is_some())
    })
}

/// Line numbers after the block that set a colour the theme sets, or pull in
/// another file that might.
fn kitty_overrides(content: &[u8]) -> Vec<usize> {
    let block::Found::Present { end, .. } = block::find(content, "kitty") else {
        return Vec::new();
    };
    let first = content[..end].iter().filter(|b| **b == b'\n').count() + 1;
    String::from_utf8_lossy(&content[end..])
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let key = line.split_whitespace().next().unwrap_or("");
            kitty::KEYS.contains(&key) || matches!(key, "include" | "globinclude" | "envinclude")
        })
        .map(|(i, _)| first + i)
        .collect()
}

pub fn plan_kitty(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let dir = kitty_dir(host, env);
    let dir_kind = host.fs.kind(&dir).ok().flatten();
    if dir_kind.is_none() && !on_path(host, env, "kitty") {
        return TargetPlan::skip(KITTY, Skip::NotInstalled);
    }
    let mut plan = TargetPlan::new(KITTY);

    match dir_kind {
        None => {
            // Only the last level is created; a missing parent means this
            // is not a place Kitty has ever used.
            match dir.parent() {
                Some(parent) if inside_home(host, parent) => {
                    plan.changes.push(Change::CreateDir { path: dir.clone() });
                }
                _ => return TargetPlan::skip(KITTY, Skip::OutsideHome(dir)),
            }
        }
        Some(Kind::Dir | Kind::Symlink) if inside_home(host, &dir) => {}
        Some(Kind::Dir | Kind::Symlink) => return TargetPlan::skip(KITTY, Skip::OutsideHome(dir)),
        Some(_) => return TargetPlan::skip(KITTY, Skip::NotAFile(dir)),
    }

    // The colour file: created and owned by Velatura.
    let owned = dir.join("velatura.conf");
    let wanted = kitty::render(theme).into_bytes();
    let current = match host.fs.kind(&owned) {
        Ok(None) => None,
        Ok(Some(Kind::File)) => match host.fs.read(&owned) {
            Ok(bytes) => Some(bytes),
            Err(_) => return TargetPlan::skip(KITTY, Skip::Unreadable(owned)),
        },
        _ => return TargetPlan::skip(KITTY, Skip::ForeignFile(owned)),
    };
    let pre = current.as_deref().map(sha256_hex);
    match (&pre, state.owned.get(&path_str(&owned))) {
        (None, _) => {}
        (Some(_), None) => return TargetPlan::skip(KITTY, Skip::ForeignFile(owned)),
        (Some(now), Some(written)) if now != written && !replace_changed => {
            return TargetPlan::skip(KITTY, Skip::ChangedOutside(owned));
        }
        _ => {}
    }
    if current.as_deref() != Some(wanted.as_slice()) {
        plan.changes.push(Change::Owned {
            path: owned,
            content: Some(wanted),
            pre,
        });
    }

    // The user's file: one marked block holding the include line.
    let conf = dir.join("kitty.conf");
    let current = match host.fs.kind(&conf) {
        Ok(None) => None,
        Ok(Some(Kind::File)) => read_opt(host, &conf).ok().flatten(),
        Ok(Some(Kind::Symlink)) => {
            let is_file = host
                .fs
                .canonicalize(&conf)
                .ok()
                .and_then(|real| host.fs.kind(&real).ok().flatten())
                == Some(Kind::File);
            if !is_file {
                return TargetPlan::skip(KITTY, Skip::NotAFile(conf));
            }
            if !inside_home(host, &conf) {
                return TargetPlan::skip(KITTY, Skip::OutsideHome(conf));
            }
            read_opt(host, &conf).ok().flatten()
        }
        _ => return TargetPlan::skip(KITTY, Skip::NotAFile(conf)),
    };
    if current.is_none() && host.fs.kind(&conf).ok().flatten().is_some() {
        return TargetPlan::skip(KITTY, Skip::Unreadable(conf));
    }
    let before = current.clone().unwrap_or_default();
    let Some(up) = block::upsert(&before, "kitty", "include velatura.conf") else {
        return TargetPlan::skip(KITTY, Skip::MalformedBlock(conf));
    };
    let record = state.blocks.get(&path_str(&conf));
    let overrides = kitty_overrides(&up.content);
    if up.content != before {
        plan.needs_consent = up.before.is_none() && !state.consents.contains(KITTY);
        plan.changes.push(Change::Block {
            path: conf.clone(),
            target: "kitty".to_owned(),
            pre: current.as_deref().map(sha256_hex),
            body_before: up.before.clone(),
            body_after: Some("include velatura.conf".to_owned()),
            added_eol: match (&up.before, record) {
                (Some(_), Some(r)) => r.added_eol,
                _ => up.added_eol,
            },
            created_file: match (&up.before, record) {
                (Some(_), Some(r)) => r.created_file,
                _ => current.is_none(),
            },
            content: Some(up.content),
        });
    }

    if !overrides.is_empty() {
        plan.notes.push(Note::KittyOverride {
            path: conf,
            lines: overrides,
        });
    }
    for name in [
        "dark-theme.auto.conf",
        "light-theme.auto.conf",
        "no-preference-theme.auto.conf",
    ] {
        let path = dir.join(name);
        if host.fs.kind(&path).ok().flatten().is_some() {
            plan.notes.push(Note::KittyAutoTheme { path });
        }
    }
    if !plan.changes.is_empty() {
        plan.notes.push(Note::KittyReload);
    }
    plan
}

pub fn plan_appearance(host: &Host<'_>, theme: &Resolved<'_>) -> TargetPlan {
    let mut plan = TargetPlan::new(APPEARANCE);
    let scheme = match theme.colors.appearance {
        Appearance::Dark => "prefer-dark",
        Appearance::Light => "default",
    };
    let accent = nearest_accent(theme.colors.ui.accent);
    let wanted = [
        ("color-scheme", scheme),
        ("accent-color", accent.setting_value()),
    ];
    for (key, value) in wanted {
        match host.settings.read(INTERFACE_SCHEMA, key) {
            Err(SettingsError::Unavailable(why)) => {
                return TargetPlan::skip(APPEARANCE, Skip::SettingsUnavailable(why));
            }
            Err(e) => {
                return TargetPlan::skip(APPEARANCE, Skip::SettingsUnavailable(format!("{e:?}")));
            }
            Ok(None) if key == "accent-color" => {
                plan.notes.push(Note::AccentUnsupported);
                continue;
            }
            Ok(None) => {
                return TargetPlan::skip(
                    APPEARANCE,
                    Skip::SettingsUnavailable(format!("{INTERFACE_SCHEMA} {key}")),
                );
            }
            Ok(Some(now)) => {
                if key == "accent-color" {
                    plan.notes.push(Note::AccentNearest {
                        wanted: theme.colors.ui.accent.hex(),
                        used: accent.setting_value(),
                    });
                }
                if now.effective == value {
                    continue;
                }
                if !now.writable {
                    plan.notes.push(Note::SettingLocked {
                        key: key.to_owned(),
                    });
                    continue;
                }
                plan.changes.push(Change::Setting {
                    schema: INTERFACE_SCHEMA.to_owned(),
                    key: key.to_owned(),
                    value: Some(value.to_owned()),
                    pre: now.user,
                });
            }
        }
    }
    plan
}
