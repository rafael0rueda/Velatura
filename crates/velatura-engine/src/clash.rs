//! Other Shell extensions that may show or draw what a part of Velatura's
//! does: a second media player or UPS reading in the top bar, a list of
//! their own beside the clock, a look of their own for the bar, a line of
//! their own around the window in use, or a clock kept at a fixed place.
//! Found by reading the files and settings of the extensions turned on.
//! Nothing is run, and no extension is known by name.

use std::path::{Path, PathBuf};

use crate::host::{Host, Kind};
use crate::plan::{Clash, Note};
use crate::targets::{self, Env, MEDIA, OUTLINE, TASKS, TOP_BAR, UPS, exists};

/// Scripts larger than this are not read.
const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
/// At most this many scripts are read of one extension.
const MAX_SCRIPTS: usize = 300;
/// Folders deeper than this inside an extension are not looked in.
const MAX_DEPTH: usize = 4;

/// An enabled Shell extension other than Velatura's own.
pub(crate) struct Extension {
    /// Its name as it gives it, or its uuid.
    pub name: String,
    pub dir: PathBuf,
    /// Its settings as the settings store names them, if it has any.
    pub schema: Option<String>,
}

/// The extensions turned on, other than Velatura's, found in `dirs` (the
/// data folders, the user's first) by their `metadata.json`.
pub(crate) fn enabled(host: &Host<'_>, dirs: &[PathBuf]) -> Vec<Extension> {
    let Ok(Some(list)) = host.settings.read("org.gnome.shell", "enabled-extensions") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for uuid in targets::listed(&list.effective) {
        if uuid == crate::companion::UUID || uuid.contains('/') || uuid.starts_with('.') {
            continue;
        }
        let Some(dir) = dirs
            .iter()
            .map(|d| d.join("gnome-shell/extensions").join(uuid))
            .find(|d| exists(host, &d.join("metadata.json")))
        else {
            continue;
        };
        let Some(meta) = crate::icon_set::read_capped(host, &dir.join("metadata.json"), 64 * 1024)
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            continue;
        };
        let schema = meta
            .get("settings-schema")
            .and_then(serde_json::Value::as_str)
            .filter(|id| {
                id.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
            .map(|id| {
                let local = dir.join("schemas");
                if exists(host, &local.join("gschemas.compiled")) {
                    format!("{id}@{}", local.display())
                } else {
                    id.to_owned()
                }
            });
        let name = meta
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(uuid)
            .to_owned();
        found.push(Extension { name, dir, schema });
    }
    found
}

/// What `instance` may meet in other extensions, as notes for its plan.
/// Empty for parts no extension is looked at for.
pub(crate) fn notes(host: &Host<'_>, env: Env<'_>, instance: &str) -> Vec<Note> {
    let clash = match instance {
        TASKS => Clash::Middle,
        MEDIA => Clash::Media,
        UPS => Clash::Ups,
        TOP_BAR => Clash::TopBar,
        OUTLINE => Clash::Outline,
        _ => return Vec::new(),
    };
    let found = enabled(host, &targets::data_dirs(host, env));
    let mut notes = Vec::new();
    let names: Vec<String> = found
        .iter()
        .filter(|e| shows(host, e, clash))
        .map(|e| e.name.clone())
        .collect();
    if !names.is_empty() {
        notes.push(Note::ExtensionClash { clash, names });
    }
    if matches!(instance, TASKS | MEDIA) {
        // Tasks sits first in the middle of the bar, so the clock comes
        // after it; without Tasks, the clock comes first.
        let tasks_on =
            instance == TASKS || crate::store::Prefs::load(host).is_ok_and(|p| p.takes_part(TASKS));
        let wanted = u32::from(tasks_on);
        notes.extend(found.iter().filter_map(|e| clock(host, e, wanted)));
    }
    notes
}

/// True if `ext` may show or draw what `clash` names.
fn shows(host: &Host<'_>, ext: &Extension, clash: Clash) -> bool {
    let scripts = scripts(host, &ext.dir);
    match clash {
        Clash::Media => scripts
            .iter()
            .any(|js| js.contains("org.mpris.MediaPlayer2")),
        // Network UPS Tools: its command, or the port of its server.
        Clash::Ups => scripts
            .iter()
            .any(|js| has_word(js, "upsc") || has_word(js, "3493")),
        Clash::Middle => scripts.iter().any(|js| to_middle(js)),
        Clash::TopBar => {
            crate::icon_set::read_capped(host, &ext.dir.join("stylesheet.css"), 1024 * 1024)
                .is_some_and(|css| {
                    let css = String::from_utf8_lossy(&css);
                    targets::colors_rules(&css, |selector| styles_bar(selector, &scripts))
                })
        }
        Clash::Outline => {
            scripts.iter().any(|js| {
                js.contains("focus-window") && (js.contains("border") || js.contains("outline"))
            }) && !switched_off(host, ext)
        }
    }
}

/// The scripts the Shell runs of an extension, read as text. Its
/// preferences window runs elsewhere and is left out; links are not
/// followed.
fn scripts(host: &Host<'_>, dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![(dir.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        for name in host.fs.list(&folder).unwrap_or_default() {
            let name = name.to_string_lossy();
            let path = folder.join(name.as_ref());
            match host.fs.kind(&path) {
                Ok(Some(Kind::Dir))
                    if depth < MAX_DEPTH
                        && !matches!(name.as_ref(), "prefs" | "preferences" | "node_modules")
                        && !name.starts_with('.') =>
                {
                    folders.push((path, depth + 1));
                }
                Ok(Some(Kind::File)) if name.ends_with(".js") && name != "prefs.js" => {
                    if found.len() == MAX_SCRIPTS {
                        return found;
                    }
                    if let Some(js) = crate::icon_set::read_capped(host, &path, MAX_SCRIPT_BYTES) {
                        found.push(String::from_utf8_lossy(&js).into_owned());
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// True if `word` is in `text` with no letter, digit or `_` either side.
fn has_word(text: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + word.len()..].chars().next().is_some_and(part)
    })
}

/// True if a script puts a button of its own in the middle of the top bar,
/// next to the clock: `addToStatusArea(…, 'center')`.
fn to_middle(js: &str) -> bool {
    js.match_indices("addToStatusArea(").any(|(at, call)| {
        let args = &js[at + call.len()..];
        let args = &args[..args.find(';').unwrap_or(args.len()).min(400)];
        args.contains("'center'") || args.contains("\"center\"")
    })
}

/// True if a rule's selector is for the top bar or all its buttons: the
/// bar itself, or under a class an extension always puts on it. A class
/// it adds only for an option, from a list, does not count.
fn styles_bar(selector: &str, scripts: &[String]) -> bool {
    let buttons = |rest: &str| {
        rest.is_empty()
            || rest.starts_with(':')
            || rest
                .trim_start()
                .strip_prefix(".panel-button")
                .is_some_and(|after| {
                    !after.starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')
                })
    };
    if let Some(rest) = selector.strip_prefix(".panel-button") {
        return !rest.starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_');
    }
    let Some(rest) = selector.strip_prefix("#panel") else {
        return false;
    };
    let Some(class) = rest.strip_prefix('.') else {
        return !rest.starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')
            && buttons(rest);
    };
    let end = class
        .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(class.len());
    let (class, rest) = class.split_at(end);
    !class.is_empty()
        && buttons(rest)
        && scripts.iter().any(|js| {
            ['\'', '"'].iter().any(|q| {
                js.contains(&format!("add_style_class_name({q}{class}{q}"))
                    || js.contains(&format!("= {q}{class}{q};"))
            })
        })
}

/// True if the extension's own switch for a line around the window in
/// use is off: a setting named for it set to `false` or `0`.
fn switched_off(host: &Host<'_>, ext: &Extension) -> bool {
    let Some(schema) = &ext.schema else {
        return false;
    };
    host.settings.keys(schema).iter().any(|key| {
        let switch = ["focus-hint", "outline", "outlines"]
            .iter()
            .any(|end| key.ends_with(end));
        switch
            && matches!(host.settings.read(schema, key),
                Ok(Some(s)) if matches!(s.effective.trim(), "false" | "0"))
    })
}

/// A note if `ext` keeps the clock somewhere other than at `wanted` in
/// the middle of the top bar: a clock position changed from its default,
/// or a clock offset set to another place.
fn clock(host: &Host<'_>, ext: &Extension, wanted: u32) -> Option<Note> {
    let schema = ext.schema.as_ref()?;
    let keys = host.settings.keys(schema);
    let read = |key: &str| host.settings.read(schema, key).ok().flatten();
    let moved = keys
        .iter()
        .filter(|k| k.contains("clock") && k.contains("position") && !k.contains("offset"))
        .filter_map(|k| read(k))
        .any(|s| s.effective != s.default);
    if moved {
        return Some(Note::ClockPinned {
            name: ext.name.clone(),
            offset: None,
        });
    }
    let elsewhere = keys
        .iter()
        .filter(|k| k.contains("clock") && k.contains("offset"))
        .filter_map(|k| read(k))
        .any(|s| {
            s.effective
                .trim()
                .parse::<u32>()
                .is_ok_and(|at| at != wanted)
        });
    elsewhere.then(|| Note::ClockPinned {
        name: ext.name.clone(),
        offset: Some(wanted),
    })
}

#[cfg(test)]
mod tests {
    use super::{has_word, styles_bar, to_middle};

    #[test]
    fn words_are_found_whole() {
        assert!(has_word("run('upsc ups@localhost')", "upsc"));
        assert!(has_word("const PORT = 3493;", "3493"));
        assert!(!has_word("upscale(texture)", "upsc"));
        assert!(!has_word("id = 134935", "3493"));
    }

    #[test]
    fn a_button_in_the_middle_is_told_from_one_elsewhere() {
        assert!(to_middle(
            "Main.panel.addToStatusArea('x-tasks', this._tasks, 0, 'center');"
        ));
        assert!(to_middle("panel.addToStatusArea(\"x\", b, -1, \"center\")"));
        assert!(!to_middle(
            "Main.panel.addToStatusArea('x', b, pos[1], pos[0]);"
        ));
        assert!(!to_middle(
            "Main.panel.addToStatusArea('x', b); const a = 'center';"
        ));
    }

    #[test]
    fn the_bar_is_told_from_an_extensions_own_widgets() {
        let always = [
            "const PANEL_CLASS = 'ext-panel';\nMain.panel.add_style_class_name(PANEL_CLASS);"
                .to_owned(),
        ];
        let listed = ["const STYLES = [\n  \"ext-panel\",\n];".to_owned()];
        for yes in [
            "#panel",
            "#panel:overview",
            "#panel .panel-button",
            ".panel-button:hover",
        ] {
            assert!(styles_bar(yes, &[]), "{yes}");
        }
        assert!(styles_bar("#panel.ext-panel .panel-button", &always));
        assert!(styles_bar("#panel.ext-panel:overview", &always));
        for no in [
            "#panel .ext-button",
            "#panelish",
            ".panel-buttons",
            "#panel.ext-panel .ext-label",
        ] {
            assert!(!styles_bar(no, &always), "{no}");
        }
        assert!(!styles_bar("#panel.ext-panel", &listed));
    }
}
