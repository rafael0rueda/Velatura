//! Firefox: the colours of its own window, through the stylesheet a profile
//! may keep in `chrome/userChrome.css`.
//!
//! Three things in the profile Firefox opens by default: a stylesheet of
//! Velatura's own, `chrome/velatura.css`; a marked block at the top of
//! `chrome/userChrome.css` that imports it; and a marked block at the end
//! of `user.js` that turns on the preference that makes Firefox read
//! `userChrome.css` at all. Other profiles are left as they are.
//!
//! Firefox reads these files when it starts and never writes them, so
//! they are written while it runs too, and take effect on its next start.
//! Its own settings file, `prefs.js`, is never touched. Mozilla does not
//! support this way in, and it may stop working after a Firefox update.

use std::path::{Component, Path, PathBuf};

use velatura_theme::{Resolved, firefox};

use crate::block::{FIREFOX as BLOCK, FIREFOX_PREFS};
use crate::host::Host;
use crate::plan::{Note, Skip, TargetPlan};
use crate::store::{State, read_opt};
use crate::targets::{Env, connect, exists, first_time, on_path, override_lines, own, place};

pub const FIREFOX: &str = "firefox:default";

/// The preference that makes Firefox load `userChrome.css`.
pub const PREF: &str = "toolkit.legacyUserProfileCustomizations.stylesheets";

/// What Velatura's block in `user.js` holds.
pub fn pref_line() -> String {
    format!("user_pref(\"{PREF}\", true);")
}

const IMPORT: &str = "@import url(\"velatura.css\");";

/// `installs.ini` and `profiles.ini` are a few lines each; anything much
/// larger is not one Firefox wrote.
const MAX_INI_BYTES: usize = 256 * 1024;

/// The folders Firefox may keep its profiles in, in the order they are
/// looked at: its own folder in the home folder, which it keeps using once
/// it has one, the one under the settings folder that newer versions start
/// with, then the Flatpak's and the Snap's.
pub fn roots(host: &Host<'_>) -> Vec<PathBuf> {
    let home = &host.dirs.home;
    let flatpak = home.join(".var/app/org.mozilla.firefox");
    vec![
        home.join(".mozilla/firefox"),
        host.dirs.config.join("mozilla/firefox"),
        flatpak.join(".mozilla/firefox"),
        flatpak.join("config/mozilla/firefox"),
        home.join("snap/firefox/common/.mozilla/firefox"),
    ]
}

/// The groups of an ini file, each with its keys, in file order. Lines
/// that are neither a group nor a key are passed over.
fn ini(text: &str) -> Vec<(&str, Vec<(&str, &str)>)> {
    let mut groups: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            groups.push((name, Vec::new()));
        } else if let Some((key, value)) = line.split_once('=')
            && let Some((_, keys)) = groups.last_mut()
        {
            keys.push((key.trim(), value.trim()));
        }
    }
    groups
}

fn read_ini(host: &Host<'_>, path: &Path) -> Option<String> {
    let bytes = read_opt(host, path).ok().flatten()?;
    (bytes.len() <= MAX_INI_BYTES).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// A profile's folder from the way Firefox writes it: relative to `root`,
/// or whole. A relative one may only go down.
fn profile_path(root: &Path, written: &str, relative: bool) -> Option<PathBuf> {
    let path = Path::new(written);
    if written.is_empty() || written.chars().any(char::is_control) {
        return None;
    }
    if !relative {
        return path.is_absolute().then(|| path.to_path_buf());
    }
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| root.join(path))
}

fn get<'a>(keys: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    keys.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The profile Firefox opens by default under `root`, and how many
/// profiles there are in all. Firefox keeps the default of each of its
/// installations in `installs.ini` (and since version 67 also in
/// `profiles.ini`); before that, the profile marked `Default=1` was it.
pub fn default_profile(host: &Host<'_>, root: &Path) -> Option<(PathBuf, usize)> {
    let profiles = read_ini(host, &root.join("profiles.ini"))?;
    let profiles = ini(&profiles);
    let listed: Vec<&Vec<(&str, &str)>> = profiles
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .map(|(_, keys)| keys)
        .collect();
    let relative = |path: &str| {
        listed
            .iter()
            .find(|keys| get(keys, "Path") == Some(path))
            .is_none_or(|keys| get(keys, "IsRelative") != Some("0"))
    };
    let installs = read_ini(host, &root.join("installs.ini")).unwrap_or_default();
    let installs = ini(&installs);
    let chosen = installs
        .iter()
        .chain(
            profiles
                .iter()
                .filter(|(name, _)| name.starts_with("Install")),
        )
        .find_map(|(_, keys)| get(keys, "Default"))
        .or_else(|| {
            listed
                .iter()
                .find(|keys| get(keys, "Default") == Some("1"))
                .or(listed.first().filter(|_| listed.len() == 1))
                .and_then(|keys| get(keys, "Path"))
        })?;
    let path = profile_path(root, chosen, relative(chosen))?;
    Some((path, listed.len().max(1)))
}

/// True if `line` declares the custom property `name`, not one whose name
/// only starts the same.
fn declares(line: &str, name: &str) -> bool {
    line.match_indices(name)
        .any(|(at, _)| line[at + name.len()..].trim_start().starts_with(':'))
}

pub fn plan_firefox(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let Some(root) = roots(host)
        .into_iter()
        .find(|r| exists(host, &r.join("profiles.ini")))
    else {
        // Installed but never started, or not here at all.
        return TargetPlan::skip(
            FIREFOX,
            if on_path(host, env, "firefox") {
                Skip::FirefoxNoProfile
            } else {
                Skip::NotInstalled
            },
        );
    };
    let Some((profile, profiles)) = default_profile(host, &root).filter(|(p, _)| exists(host, p))
    else {
        return TargetPlan::skip(FIREFOX, Skip::FirefoxNoProfile);
    };
    let chrome = profile.join("chrome");
    let names = firefox::names(theme);
    let mut plan = TargetPlan::new(FIREFOX);
    let mut fill = || {
        place(host, &[&profile, &chrome], &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            chrome.join("velatura.css"),
            firefox::render(theme).as_bytes(),
            &mut plan,
        )?;
        // The import has to come before every rule, so its block goes at
        // the top; rules of the user's after it that set the same
        // variables win over it.
        let sheet = chrome.join("userChrome.css");
        let (_, after) = connect(host, state, (FIREFOX, BLOCK), &sheet, IMPORT, &mut plan)?;
        let wins = override_lines(&after, BLOCK, &|line| {
            names.iter().any(|name| declares(line, name))
        });
        if !wins.is_empty() {
            plan.notes.push(Note::Override {
                path: sheet,
                lines: wins,
            });
        }
        // Firefox reads user.js from top to bottom, and the last value
        // given to a preference is the one it keeps.
        let prefs = profile.join("user.js");
        let (_, after) = connect(
            host,
            state,
            (FIREFOX, FIREFOX_PREFS),
            &prefs,
            &pref_line(),
            &mut plan,
        )?;
        let wins = override_lines(&after, FIREFOX_PREFS, &|line| {
            line.contains(PREF) && line.contains("false")
        });
        if !wins.is_empty() {
            plan.notes.push(Note::Override {
                path: prefs,
                lines: wins,
            });
        }
        Ok(())
    };
    match fill() {
        Ok(()) => {
            if !plan.changes.is_empty() {
                // The profile is the user's: the first files go into it
                // only when that is agreed to, so never in a run with no
                // one present.
                plan.needs_consent = first_time(state, FIREFOX, &profile);
                plan.notes.push(Note::FirefoxRestart);
            }
            if profiles > 1 {
                plan.notes.push(Note::FirefoxOtherProfiles);
            }
            plan
        }
        Err(why) => TargetPlan::skip(FIREFOX, why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_groups_and_keys() {
        let text =
            "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=a b\nPath = x.default\n; note\n";
        let groups = ini(text);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[1],
            ("Profile0", vec![("Name", "a b"), ("Path", "x.default")])
        );
    }

    #[test]
    fn a_relative_profile_may_only_go_down() {
        let root = Path::new("/home/u/.mozilla/firefox");
        assert_eq!(
            profile_path(root, "ab.default", true),
            Some(root.join("ab.default"))
        );
        assert_eq!(
            profile_path(root, "Profiles/ab", true),
            Some(root.join("Profiles/ab"))
        );
        assert_eq!(profile_path(root, "../ab", true), None);
        assert_eq!(profile_path(root, "/etc", true), None);
        assert_eq!(profile_path(root, "", true), None);
        assert_eq!(
            profile_path(root, "/home/u/p", false),
            Some(PathBuf::from("/home/u/p"))
        );
        assert_eq!(profile_path(root, "p", false), None);
    }

    #[test]
    fn declarations_are_told_from_longer_names() {
        assert!(declares(
            "  --toolbar-text-color: red;",
            "--toolbar-text-color"
        ));
        assert!(declares(
            "a{--toolbar-text-color :red}",
            "--toolbar-text-color"
        ));
        assert!(!declares(
            "  --toolbar-text-color-focus: red;",
            "--toolbar-text-color"
        ));
        assert!(!declares(
            "color: var(--toolbar-text-color);",
            "--toolbar-text-color"
        ));
    }
}
