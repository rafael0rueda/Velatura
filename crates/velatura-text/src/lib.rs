//! Every sentence about a plan or its outcome, in one place, shared by the
//! command and the app.
//!
//! The engine returns typed values; this module turns them into text. Source
//! text is US English and follows the GNOME writing guidelines: short,
//! neutral, the same words as the app (theme, backup, settings file).

use std::path::Path;

use velatura_engine::library::Why;
use velatura_engine::plan::{Change, Note, Skip};
use velatura_engine::txn::{Failure, Rollback};
use velatura_theme::model::clean_text;

pub const HELP: &str = "\
Velatura applies one color theme to the whole desktop.

Usage:
  velatura                         show what is applied
  velatura list                    themes that can be applied
  velatura show <theme>            a theme's colors
  velatura validate <file>         check a theme file
  velatura targets                 apps and desktop parts that can be themed
  velatura status                  theme in use and the state of each part
  velatura apply <theme>[/variant] apply a theme
  velatura switch day|night|toggle apply the daytime or the nighttime theme
  velatura switch set day <theme>  choose the daytime (or night) theme
  velatura undo                    reverse the most recent change
  velatura history                 backups, newest first
  velatura restore <id>            go back to the state before backup <id>
  velatura restore --original      remove everything Velatura added
  velatura recover                 finish rolling back an interrupted apply
  velatura doctor                  check this system

Options:
  --preview             show what would change and change nothing
  --yes                 agree to adding a line to a settings file
  --only <target>       apply to this target only (may be repeated)
  --except <target>     leave this target out (may be repeated)
  --replace-changed     overwrite Velatura files that were changed by hand
  --strict              with validate: treat warnings as errors
  --json                machine-readable output
  -h, --help            this text
  -V, --version         version

If the desktop is unreadable, from a text console (Ctrl+Alt+F3):
  velatura undo                    reverse the last theme
  velatura restore --original      remove everything Velatura added
Backups are plain files in ~/.local/share/velatura/backups.

Exit codes: 0 done, 1 failed, 2 wrong usage, 3 agreement needed and nothing
changed, 4 another Velatura is running, 5 done but something was left out.
";

/// A path for display: home shortened to `~`, control characters removed.
pub fn path(p: &Path, home: &Path) -> String {
    let shown = match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => p.to_string_lossy().into_owned(),
    };
    clean_text(&shown, 400)
}

pub fn instance(id: &str) -> &str {
    match id {
        "kitty:default" => "Kitty",
        "ghostty:default" => "Ghostty",
        "gtk4:default" => "Apps (GTK 4 and libadwaita)",
        "gtk3:default" => "Older Apps (GTK 3)",
        "gnome:shell" => "Top Bar and Overview",
        "gnome:appearance" => "Style and Accent Color",
        "" => "Changes",
        other => other,
    }
}

fn setting_name(key: &str) -> &str {
    match key {
        "color-scheme" => "Style",
        "accent-color" => "Accent Color",
        "gtk-theme" => "Legacy Application Theme",
        "name" => "Shell Theme",
        other => other,
    }
}

fn setting_value(key: &str, value: Option<&str>) -> String {
    match (key, value) {
        (_, None) => "the default".to_owned(),
        ("gtk-theme" | "name", Some(v)) => v.to_owned(),
        ("color-scheme", Some("prefer-dark")) => "Dark".to_owned(),
        ("color-scheme", Some("default")) => "Default".to_owned(),
        ("color-scheme", Some("prefer-light")) => "Light".to_owned(),
        (_, Some(v)) => {
            let mut c = v.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// The display name of a system accent, from its settings value.
pub fn accent_name(value: &str) -> String {
    setting_value("accent-color", Some(value))
}

pub fn change(c: &Change, home: &Path) -> String {
    match c {
        Change::CreateDir { path: p } => format!("Create the folder {}", path(p, home)),
        Change::RemoveDir { path: p } => {
            format!("Remove the folder {} if it is empty", path(p, home))
        }
        Change::Owned {
            path: p,
            content,
            pre,
        } => match (content, pre) {
            (Some(_), None) => format!("Create {}", path(p, home)),
            (Some(_), Some(_)) => format!("Update {}", path(p, home)),
            (None, _) => format!("Remove {} (created by Velatura)", path(p, home)),
        },
        Change::Block {
            path: p,
            content,
            body_before,
            body_after,
            ..
        } => match (content, body_before, body_after) {
            (None, _, _) => format!(
                "Remove {} (created by Velatura, holds nothing else)",
                path(p, home)
            ),
            (_, None, Some(_)) => format!(
                "Add a marked block with one line at the end of {}",
                path(p, home)
            ),
            (_, Some(_), None) => format!("Take the Velatura block out of {}", path(p, home)),
            _ => format!("Update the Velatura block in {}", path(p, home)),
        },
        Change::Setting { key, value, .. } => format!(
            "Set {} to {}",
            setting_name(key),
            setting_value(key, value.as_deref())
        ),
    }
}

pub fn skip(why: &Skip, home: &Path) -> String {
    match why {
        Skip::NotInstalled => "Not installed".to_owned(),
        Skip::NotSelected => "Left out".to_owned(),
        Skip::NoConsent => {
            "Left out: adding a line to the settings file was not agreed to".to_owned()
        }
        Skip::OutsideHome(p) => {
            format!("Left out: {} leads outside the home folder", path(p, home))
        }
        Skip::NotAFile(p) => format!(
            "Left out: {} is not a regular file or folder",
            path(p, home)
        ),
        Skip::Unreadable(p) => format!("Left out: {} cannot be read", path(p, home)),
        Skip::MalformedBlock(p) => format!(
            "Left out: the Velatura markers in {} are damaged. Remove both marker lines, then apply again",
            path(p, home)
        ),
        Skip::ForeignFile(p) => format!(
            "Left out: {} already exists and was not created by Velatura",
            path(p, home)
        ),
        Skip::ChangedOutside(p) => format!(
            "Left out: {} was changed outside Velatura. Use --replace-changed to overwrite it",
            path(p, home)
        ),
        Skip::SettingsUnavailable(what) => format!(
            "Left out: desktop settings are not available ({})",
            clean_text(what, 120)
        ),
        Skip::HardLinked(p) => format!(
            "Left out: {} has more than one name (a hard link), and replacing it would split them",
            path(p, home)
        ),
        Skip::HighContrast => {
            "Left out: High Contrast is on, and recoloring would lower contrast".to_owned()
        }
        Skip::NeedsUserThemes => "Left out: the top bar and overview can only be recolored through the User Themes extension, which is not installed. It is in the gnome-shell-extension-user-theme package and on extensions.gnome.org".to_owned(),
        Skip::NeedsAdwGtk3 => "Left out: older apps can only be recolored with the adw-gtk3 theme, which is not installed. It can be installed without administrator rights into ~/.local/share/themes, or from the adw-gtk3-theme package".to_owned(),
    }
}

pub fn note(n: &Note, home: &Path) -> String {
    match n {
        Note::Override { path: p, lines } => format!(
            "{} has lines after the Velatura block that win over the theme (line {})",
            path(p, home),
            lines
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Note::KittyAutoTheme { path: p } => format!(
            "{} exists. Kitty applies it over every other color setting, so the theme does not show until that file is renamed",
            path(p, home)
        ),
        Note::KittyReload => {
            "If open Kitty windows keep the old colors, press Ctrl+Shift+F5 in each".to_owned()
        }
        Note::GhosttyReload => {
            "To update open Ghostty windows, press Ctrl+Shift+Comma in one of them".to_owned()
        }
        Note::GtkRestart => {
            "Apps that are already open keep their colors until reopened".to_owned()
        }
        Note::LinkedFile { path: p, real } => format!(
            "{} is a link. The change is written to {}",
            path(p, home),
            path(real, home)
        ),
        Note::SettingNeedsSession { schema, key, value } => {
            let quote = |v: &str| format!("'{}'", clean_text(v, 200).replace('\'', "'\\''"));
            let (id, dir) = match schema.split_once('@') {
                Some((id, dir)) => (id, format!("--schemadir {} ", quote(dir))),
                None => (schema.as_str(), String::new()),
            };
            let command = match value {
                Some(v) => format!("gsettings {dir}set {id} {key} {}", quote(v)),
                None => format!("gsettings {dir}reset {id} {key}"),
            };
            format!(
                "{} could not be put back without a desktop session. After signing in, run: {command}",
                setting_name(key)
            )
        }
        Note::UserThemesOff => "The User Themes extension is turned off, so the top bar keeps its colors. Turn it on in the Extensions app".to_owned(),
        Note::ShellExtensionStyles { names } => format!(
            "These extensions bring their own colors, which win over the theme where they apply: {}",
            clean_text(&names.join(", "), 400)
        ),
        Note::Gtk3DarkForced { path: p } => format!(
            "{} asks older apps for dark and can work against this light theme",
            path(p, home)
        ),
        Note::AccentNearest { wanted, used } => format!(
            "The theme's accent is {wanted}. GNOME offers nine accents; the nearest is {}",
            setting_value("accent-color", Some(used))
        ),
        Note::AccentUnsupported => "This GNOME version has no accent color setting".to_owned(),
        Note::SettingLocked { key } => format!(
            "{} is locked by the system administrator and was not changed",
            setting_name(key)
        ),
        Note::KeptFile { path: p } => format!(
            "{} was changed outside Velatura and is left as it is",
            path(p, home)
        ),
        Note::KeptSetting { key } => format!(
            "{} was changed outside Velatura and is left as it is",
            setting_name(key.rsplit(' ').next().unwrap_or(key))
        ),
        Note::LowContrast => {
            "Text on background is below a contrast of 4.5 to 1 and may be hard to read".to_owned()
        }
    }
}

pub fn rollback(r: &Rollback) -> String {
    let mut out = String::new();
    for kept in &r.kept {
        out.push_str(&format!(
            "  Kept, because it was changed afterwards: {}\n",
            clean_text(kept, 400)
        ));
    }
    for (what, why) in &r.errors {
        out.push_str(&format!(
            "  Could not be put back: {} ({})\n",
            clean_text(what, 400),
            clean_text(why, 200)
        ));
    }
    out
}

pub fn failure(f: &Failure) -> String {
    match f {
        Failure::Stale(what) => format!(
            "Nothing was changed: {} changed after the preview. Run the command again.",
            clean_text(what, 400)
        ),
        Failure::Prepare(why) => format!(
            "Nothing was changed: the backup could not be written ({}).",
            clean_text(why, 200)
        ),
        Failure::Step {
            what,
            error,
            rollback: r,
        } => {
            let mut out = format!(
                "The change failed at {} ({}).\n",
                clean_text(what, 400),
                clean_text(error, 200)
            );
            if r.is_clean() && r.kept.is_empty() {
                out.push_str("Everything was put back as it was.");
            } else if r.is_clean() {
                out.push_str("Everything else was put back as it was.\n");
                out.push_str(&rollback(r));
            } else {
                out.push_str("Rolling back did not finish:\n");
                out.push_str(&rollback(r));
                out.push_str("Backups are in ~/.local/share/velatura/backups. To try again: velatura recover");
            }
            out
        }
    }
}

pub fn rejected(why: &Why) -> String {
    match why {
        Why::Unreadable => "cannot be read as text".to_owned(),
        Why::Invalid(invalid) => invalid.to_string().replace('\n', "; "),
        Why::IdTaken(id) => format!("its id \"{id}\" is already used by another theme"),
    }
}

/// Seconds since the Unix epoch as `YYYY-MM-DD HH:MM UTC`.
pub fn time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{d:02} {h:02}:{m:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(time(0), "1970-01-01 00:00 UTC");
        assert_eq!(time(1_800_000_000), "2027-01-15 08:00 UTC");
        assert_eq!(time(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn paths_are_shortened_and_cleaned() {
        let home = Path::new("/home/u");
        assert_eq!(
            path(Path::new("/home/u/.config/kitty/kitty.conf"), home),
            "~/.config/kitty/kitty.conf"
        );
        assert_eq!(path(Path::new("/etc/x\u{1b}[31m"), home), "/etc/x[31m");
    }
}
