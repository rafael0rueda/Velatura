//! Every sentence about a plan or its outcome, in one place, shared by the
//! command and the app.
//!
//! The engine returns typed values; this module turns them into text. Source
//! text is US English and follows the GNOME writing guidelines: short,
//! neutral, the same words as the app (theme, backup, settings file).

use std::path::Path;

use velatura_engine::library::Why;
use velatura_engine::plan::{Change, Note, Skip};
use velatura_engine::targets::OtherTool;
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

/// The function that looks a sentence up in the translation catalog. The
/// command and the app each install theirs at start; without one the
/// English source text is used.
static TRANSLATE: std::sync::OnceLock<fn(&str) -> String> = std::sync::OnceLock::new();

pub fn set_translator(lookup: fn(&str) -> String) {
    let _ = TRANSLATE.set(lookup);
}

fn t(text: &str) -> String {
    TRANSLATE.get().map_or_else(|| text.to_owned(), |f| f(text))
}

/// Translate a sentence, then put the arguments in place of `{1}`, `{2}`
/// and so on. Numbered, so a translation can reorder them.
macro_rules! tf {
    ($text:literal $(, $arg:expr)* $(,)?) => {{
        let mut out = t($text);
        let mut n = 0;
        $(
            n += 1;
            out = out.replace(&format!("{{{n}}}"), &$arg.to_string());
        )*
        let _ = n;
        out
    }};
}

pub fn instance(id: &str) -> String {
    t(match id {
        "kitty:default" => "Kitty",
        "ghostty:default" => "Ghostty",
        "gtk4:default" => "Apps (GTK 4 and libadwaita)",
        "gtk3:default" => "Older Apps (GTK 3)",
        "gnome:shell" => "Top Bar and Overview",
        "gnome:appearance" => "Style and Accent Color",
        "" => "Changes",
        other => return other.to_owned(),
    })
}

fn setting_name(key: &str) -> String {
    t(match key {
        "color-scheme" => "Style",
        "accent-color" => "Accent Color",
        "gtk-theme" => "Legacy Application Theme",
        "name" => "Shell Theme",
        other => return other.to_owned(),
    })
}

fn setting_value(key: &str, value: Option<&str>) -> String {
    match (key, value) {
        (_, None) => t("the default"),
        ("gtk-theme" | "name", Some(v)) => v.to_owned(),
        ("color-scheme", Some("prefer-dark")) => t("Dark"),
        ("color-scheme", Some("default")) => t("Default"),
        ("color-scheme", Some("prefer-light")) => t("Light"),
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
        Change::CreateDir { path: p } => tf!("Create the folder {1}", path(p, home)),
        Change::RemoveDir { path: p } => {
            tf!("Remove the folder {1} if it is empty", path(p, home))
        }
        Change::Owned {
            path: p,
            content,
            pre,
        } => match (content, pre) {
            (Some(_), None) => tf!("Create {1}", path(p, home)),
            (Some(_), Some(_)) => tf!("Update {1}", path(p, home)),
            (None, _) => tf!("Remove {1} (created by Velatura)", path(p, home)),
        },
        Change::Block {
            path: p,
            content,
            body_before,
            body_after,
            ..
        } => match (content, body_before, body_after) {
            (None, _, _) => tf!(
                "Remove {1} (created by Velatura, holds nothing else)",
                path(p, home)
            ),
            (_, None, Some(_)) => tf!(
                "Add a marked block with one line at the end of {1}",
                path(p, home)
            ),
            (_, Some(_), None) => tf!("Take the Velatura block out of {1}", path(p, home)),
            _ => tf!("Update the Velatura block in {1}", path(p, home)),
        },
        Change::Setting { key, value, .. } => tf!(
            "Set {1} to {2}",
            setting_name(key),
            setting_value(key, value.as_deref())
        ),
    }
}

pub fn skip(why: &Skip, home: &Path) -> String {
    match why {
        Skip::NotInstalled => t("Not installed"),
        Skip::NotSelected => t("Left out"),
        Skip::NoConsent => t("Left out: adding a line to the settings file was not agreed to"),
        Skip::OutsideHome(p) => {
            tf!("Left out: {1} leads outside the home folder", path(p, home))
        }
        Skip::NotAFile(p) => tf!(
            "Left out: {1} is not a regular file or folder",
            path(p, home)
        ),
        Skip::Unreadable(p) => tf!("Left out: {1} cannot be read", path(p, home)),
        Skip::MalformedBlock(p) => tf!(
            "Left out: the Velatura markers in {1} are damaged. Remove both marker lines, then apply again",
            path(p, home)
        ),
        Skip::ForeignFile(p) => tf!(
            "Left out: {1} already exists and was not created by Velatura",
            path(p, home)
        ),
        Skip::ChangedOutside(p) => tf!(
            "Left out: {1} was changed outside Velatura. Use --replace-changed to overwrite it",
            path(p, home)
        ),
        Skip::SettingsUnavailable(what) => tf!(
            "Left out: desktop settings are not available ({1})",
            clean_text(what, 120)
        ),
        Skip::HardLinked(p) => tf!(
            "Left out: {1} has more than one name (a hard link), and replacing it would split them",
            path(p, home)
        ),
        Skip::HighContrast => {
            t("Left out: High Contrast is on, and recoloring would lower contrast")
        }
        Skip::NeedsUserThemes => t(
            "Left out: the top bar and overview can only be recolored through the User Themes extension, which is not installed. It is in the gnome-shell-extension-user-theme package and on extensions.gnome.org",
        ),
        Skip::NeedsAdwGtk3 => t(
            "Left out: older apps can only be recolored with the adw-gtk3 theme, which is not installed. It can be installed without administrator rights into ~/.local/share/themes, or from the adw-gtk3-theme package",
        ),
    }
}

pub fn note(n: &Note, home: &Path) -> String {
    match n {
        Note::Override { path: p, lines } => tf!(
            "{1} has lines after the Velatura block that win over the theme (line {2})",
            path(p, home),
            lines
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Note::KittyAutoTheme { path: p } => tf!(
            "{1} exists. Kitty applies it over every other color setting, so the theme does not show until that file is renamed",
            path(p, home)
        ),
        Note::KittyReload => {
            t("If open Kitty windows keep the old colors, press Ctrl+Shift+F5 in each")
        }
        Note::GhosttyReload => {
            t("To update open Ghostty windows, press Ctrl+Shift+Comma in one of them")
        }
        Note::GtkRestart => t("Apps that are already open keep their colors until reopened"),
        Note::LinkedFile { path: p, real } => tf!(
            "{1} is a link. The change is written to {2}",
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
            tf!(
                "{1} could not be put back without a desktop session. After signing in, run: {2}",
                setting_name(key),
                command
            )
        }
        Note::UserThemesOff => t(
            "The User Themes extension is turned off, so the top bar keeps its colors. Turn it on in the Extensions app",
        ),
        Note::ShellExtensionStyles { names } => tf!(
            "These extensions bring their own colors, which win over the theme where they apply: {1}",
            clean_text(&names.join(", "), 400)
        ),
        Note::Gtk3DarkForced { path: p } => tf!(
            "{1} asks older apps for dark and can work against this light theme",
            path(p, home)
        ),
        Note::AccentNearest { wanted, used } => tf!(
            "The theme's accent is {1}. GNOME offers nine accents; the nearest is {2}",
            wanted,
            setting_value("accent-color", Some(used))
        ),
        Note::AccentUnsupported => t("This GNOME version has no accent color setting"),
        Note::SettingLocked { key } => tf!(
            "{1} is locked by the system administrator and was not changed",
            setting_name(key)
        ),
        Note::KeptFile { path: p } => tf!(
            "{1} was changed outside Velatura and is left as it is",
            path(p, home)
        ),
        Note::KeptSetting { key } => tf!(
            "{1} was changed outside Velatura and is left as it is",
            setting_name(key.rsplit(' ').next().unwrap_or(key))
        ),
        Note::LowContrast => {
            t("Text on background is below a contrast of 4.5 to 1 and may be hard to read")
        }
    }
}

/// Another tool on this machine that changes the same colours or files.
pub fn other_tool(tool: &OtherTool, home: &Path) -> String {
    match tool {
        OtherTool::Known { name, path: p } => tf!(
            "Another theming tool, {1}, is present ({2}). It may write the same colors",
            name,
            path(p, home)
        ),
        OtherTool::Timer { unit } => tf!(
            "A timer that may change the theme is turned on: {1}",
            clean_text(unit, 120)
        ),
        OtherTool::Dotfiles { name, path: p } => tf!(
            "Settings files may be managed by {1} ({2}). It can overwrite changes made here",
            name,
            path(p, home)
        ),
    }
}

pub fn rollback(r: &Rollback) -> String {
    let mut out = String::new();
    for kept in &r.kept {
        out.push_str(&tf!(
            "  Kept, because it was changed afterwards: {1}\n",
            clean_text(kept, 400)
        ));
    }
    for (what, why) in &r.errors {
        out.push_str(&tf!(
            "  Could not be put back: {1} ({2})\n",
            clean_text(what, 400),
            clean_text(why, 200)
        ));
    }
    out
}

pub fn failure(f: &Failure) -> String {
    match f {
        Failure::Stale(what) => tf!(
            "Nothing was changed: {1} changed after the preview. Run the command again.",
            clean_text(what, 400)
        ),
        Failure::Prepare(why) => tf!(
            "Nothing was changed: the backup could not be written ({1}).",
            clean_text(why, 200)
        ),
        Failure::Step {
            what,
            error,
            rollback: r,
        } => {
            let mut out = tf!(
                "The change failed at {1} ({2}).\n",
                clean_text(what, 400),
                clean_text(error, 200)
            );
            if r.is_clean() && r.kept.is_empty() {
                out.push_str(&t("Everything was put back as it was."));
            } else if r.is_clean() {
                out.push_str(&t("Everything else was put back as it was.\n"));
                out.push_str(&rollback(r));
            } else {
                out.push_str(&t("Rolling back did not finish:\n"));
                out.push_str(&rollback(r));
                out.push_str(&t("Backups are in ~/.local/share/velatura/backups. To try again: velatura recover"));
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
