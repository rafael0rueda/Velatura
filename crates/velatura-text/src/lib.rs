//! Every sentence about a plan or its outcome, in one place, shared by the
//! command and the app.
//!
//! The engine returns typed values; this module turns them into text. Source
//! text is US English and follows the GNOME writing guidelines: short,
//! neutral, the same words as the app (theme, backup, settings file).

pub mod catalog;

use std::path::Path;

use velatura_engine::companion;
use velatura_engine::library::{Refused as LibraryRefused, Why};
use velatura_engine::look::{Kind, Rejected};
use velatura_engine::plan::{Change, Note, Skip};
use velatura_engine::schedule::{LastRun, Period, Problem, Refused, Status, format_time};
use velatura_engine::targets::OtherTool;
use velatura_engine::txn::{Failure, Rollback};
use velatura_theme::import::Format;
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
  velatura switch auto             apply the one that belongs to the time of day
  velatura schedule                whether the themes change by the clock
  velatura schedule set            turn that on; with --day and --night, set the times
  velatura schedule off            turn it off and remove its timer
  velatura schedule follow         change with the desktop's light and dark style instead
  velatura top-bar                 how the Top Bar Style part draws the top bar
  velatura top-bar style <style>   bar for a full bar, pills for a pill per button
  velatura top-bar opacity <0-100> how opaque, as a percentage
  velatura extension               whether Velatura's own Shell extension is installed
  velatura extension install       install it for this user; turn it on in the Extensions app
  velatura extension remove        remove the copy Velatura installed
  velatura theme import <file>     add a theme file to the library; base16,
                                   base24 and Kitty themes are read too
  velatura theme export <theme> [<file>]   write a theme's file, or print it
  velatura theme resolve <theme>   print a theme with every color written out
  velatura theme remove <theme>    take an added theme out of the library
  velatura template                the templates added, and where each one writes
  velatura template add <file>     check a template and add it
  velatura template remove <id>    take a template out (a copy is kept)
  velatura look                    icons, cursor, wallpaper and fonts in use
  velatura look list <what>        what is installed: icons, cursor or wallpaper
  velatura look set <what> <name>  choose one; also font, document-font, monospace-font
  velatura look reset <what>       go back to the desktop's default
  velatura undo                    reverse the most recent change
  velatura history                 backups, newest first
  velatura restore <id>            go back to the state before backup <id>
  velatura restore --original      remove everything Velatura added
  velatura recover                 finish rolling back an interrupted apply
  velatura doctor                  check this system

Options:
  --preview             show what would change and change nothing
  --yes                 agree to adding to the settings of an app
  --only <target>       apply to this target only (may be repeated)
  --except <target>     leave this target out (may be repeated)
  --replace-changed     overwrite Velatura files that were changed by hand
  --strict              with validate: treat warnings as errors
  --day <HH:MM>         with schedule set: when the daytime theme starts
  --night <HH:MM>       with schedule set: when the nighttime theme starts
  --as <id>             with theme import or template add: store it under another id
  --accent <color>      with theme import of a base16, base24 or Kitty file:
                        which of its colors is the accent, by name or #rrggbb
  --replace             with theme import or export, or template add: replace what is there
  --again               with switch auto: act even if this part of the day was done
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

/// The language everything in this crate is said in. Chosen once, at
/// start; until then, and for a program that never chooses, the source
/// text is used.
static LANGUAGE: std::sync::OnceLock<Option<&'static catalog::Catalog>> =
    std::sync::OnceLock::new();

/// Speak the language the session asks for, if there is a catalog for it.
/// `var` looks up an environment variable.
pub fn set_language(var: &dyn Fn(&str) -> Option<String>) {
    let _ = LANGUAGE.set(catalog::from_environment(var));
}

/// Look a string up in the catalog of the chosen language.
pub fn tr(text: &str) -> String {
    match LANGUAGE.get().copied().flatten() {
        Some(catalog) => catalog.find(text).to_owned(),
        None => text.to_owned(),
    }
}

/// The form of a counted phrase for `n`, with the number in place of `{n}`.
pub fn plural(one: &str, many: &str, n: usize) -> String {
    let form = match LANGUAGE.get().copied().flatten() {
        Some(catalog) => catalog.find_plural(one, many, n),
        None if n == 1 => one,
        None => many,
    };
    form.replace("{n}", &n.to_string())
}

fn t(text: &str) -> String {
    tr(text)
}

/// What every reason for leaving something out begins with. The app
/// takes it off where the row already says that the part is left out.
pub fn left_out_prefix() -> String {
    t("Left out: ")
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
    match id {
        "kitty:default" => t("Kitty"),
        "ghostty:default" => t("Ghostty"),
        "tmux:default" => t("tmux"),
        "nvim:default" => t("Neovim"),
        "starship:default" => t("Starship"),
        "ptyxis:default" => t("Ptyxis"),
        "texteditor:default" => t("Text Editor"),
        "firefox:default" => t("Firefox"),
        "gtk4:default" => t("Apps (GTK 4 and libadwaita)"),
        "gtk3:default" => t("Older Apps (GTK 3)"),
        "gnome:shell" => t("Top Bar and Overview"),
        "gnome:appearance" => t("Style and Accent Color"),
        "gnome:top-bar" => t("Top Bar Style"),
        "gnome:ups" => t("UPS Indicator"),
        "gnome:outline" => t("Window Outline"),
        "flatpak:global" => t("Flatpak Apps"),
        "gnome:look" => t("Icons, Cursor, Wallpaper and Fonts"),
        "icons:recolor" => t("Recolored Icons"),
        "cursors:recolor" => t("Recolored Cursor"),
        "" => t("Changes"),
        other => match other.strip_prefix(velatura_engine::templates::PREFIX) {
            Some(id) => tf!("{1} (template)", clean_text(id, 64)),
            None => other.to_owned(),
        },
    }
}

fn setting_name(key: &str) -> String {
    match key {
        "color-scheme" => t("Style"),
        "accent-color" => t("Accent Color"),
        "gtk-theme" => t("Legacy Application Theme"),
        "name" => t("Shell Theme"),
        "palette" => t("Ptyxis Palette"),
        "style-scheme" => t("Text Editor Style"),
        "icon-theme" => t("Icons"),
        "cursor-theme" => t("Cursor"),
        "picture-uri" => t("Wallpaper"),
        "picture-uri-dark" => t("Wallpaper for the Dark Style"),
        "font-name" => t("Interface Font"),
        "document-font-name" => t("Document Font"),
        "monospace-font-name" => t("Monospace Font"),
        other => other.to_owned(),
    }
}

fn setting_value(key: &str, value: Option<&str>) -> String {
    match (key, value) {
        (_, None) => t("the default"),
        ("picture-uri" | "picture-uri-dark", Some(v)) => clean_text(&picture_name(v), 80),
        ("color-scheme" | "accent-color", Some(v)) if !v.is_empty() => match v {
            "prefer-dark" => t("Dark"),
            "default" => t("Default"),
            "prefer-light" => t("Light"),
            v => {
                let mut c = v.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            }
        },
        // A name: a theme, a font. Shown as it is.
        (_, Some(v)) => clean_text(v, 120),
    }
}

/// The file name at the end of a picture's address, without `%XX` codes.
fn picture_name(uri: &str) -> String {
    let last = uri.rsplit('/').next().unwrap_or(uri);
    let mut out = Vec::new();
    let bytes = last.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a desktop setting is called, from its key.
pub fn setting_title(key: &str) -> String {
    setting_name(key)
}

/// What one of the desktop choices is called.
pub fn look_name(kind: Kind) -> String {
    setting_name(kind.keys()[0].1)
}

/// A current value of one of the desktop choices, for display.
pub fn look_value(kind: Kind, value: &str) -> String {
    setting_value(kind.keys()[0].1, Some(value))
}

/// Why a choice of icons, cursor, wallpaper or font was not accepted.
pub fn rejected_choice(kind: Kind, why: &Rejected) -> Option<String> {
    match why {
        Rejected::NotInstalled if kind == Kind::Wallpaper => Some(t(
            "Not one of the installed wallpapers, and not the full path of a picture",
        )),
        Rejected::NotInstalled => Some(t("No installed theme has that name")),
        Rejected::NotAFont => Some(t(
            "A font is written as its name and a size, for example Adwaita Sans 11",
        )),
        Rejected::Engine(_) => None,
    }
}

/// The display name of a system accent, from its settings value.
pub fn accent_name(value: &str) -> String {
    setting_value("accent-color", Some(value))
}

/// Every change as a line, with the icons of a recolored set said in one
/// line for the set instead of one line each.
pub fn changes(list: &[Change], home: &Path) -> Vec<String> {
    use velatura_engine::icon_set::{set_of, sets_in};
    let sets = sets_in(list);
    let mut said = Vec::new();
    let mut out = Vec::new();
    for c in list {
        let path = match c {
            Change::CreateDir { path }
            | Change::RemoveDir { path }
            | Change::Owned { path, .. } => Some(path),
            _ => None,
        };
        let Some(set) = path.and_then(|p| set_of(p)) else {
            out.push(change(c, home));
            continue;
        };
        let written = !matches!(
            c,
            Change::Owned { content: None, .. } | Change::RemoveDir { .. }
        );
        if said.contains(&(set.clone(), written)) {
            continue;
        }
        said.push((set.clone(), written));
        let n = sets
            .iter()
            .find(|(s, _, w)| *s == set && *w == written)
            .map_or(0, |(_, n, _)| *n);
        let name = clean_text(&set.file_name().unwrap_or_default().to_string_lossy(), 80);
        let cursors = velatura_engine::icon_set::is_cursor_set(&name);
        out.push(match (written, cursors) {
            (true, false) => tf!("Create the icon set {1} with {2} recolored icons", name, n),
            (true, true) => tf!(
                "Create the cursor set {1} with {2} recolored cursors",
                name,
                n
            ),
            (false, false) => tf!(
                "Remove the icon set {1}, made by Velatura for other colors ({2} icons)",
                name,
                n
            ),
            (false, true) => tf!(
                "Remove the cursor set {1}, made by Velatura for other colors ({2} cursors)",
                name,
                n
            ),
        });
    }
    out
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
            target,
            content,
            body_after,
            ..
        } if target == velatura_engine::targets::FLATPAK_SPAN => match (content, body_after) {
            (None, _) => tf!(
                "Remove {1} (created by Velatura, holds nothing else)",
                path(p, home)
            ),
            (_, Some(_)) => tf!(
                "Let Flatpak apps read the two GTK settings folders, by adding to a list in {1}",
                path(p, home)
            ),
            (_, None) => tf!(
                "Take what Velatura added out of the list in {1}",
                path(p, home)
            ),
        },
        Change::Block {
            path: p,
            target,
            content,
            body_before,
            body_after,
            ..
        } => match (content, body_before, body_after) {
            (None, _, _) => tf!(
                "Remove {1} (created by Velatura, holds nothing else)",
                path(p, home)
            ),
            (_, None, Some(body)) if body.lines().count() > 1 => tf!(
                "Add a marked block with Velatura's palette at the top of {1}",
                path(p, home)
            ),
            (_, None, Some(_)) if velatura_engine::block::at_top(target) => tf!(
                "Add a marked block with one line at the top of {1}",
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
        Skip::NoConsent => t("Left out: adding to its settings was not agreed to"),
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
            "Left out: the top bar and overview are recolored through an extension, and none is turned on. Velatura's own can be installed with: velatura extension install. The User Themes extension works too",
        ),
        Skip::NeedsVelaturaExtension => t(
            "Left out: this part is shown by Velatura's own extension, which is not turned on. It can be installed with: velatura extension install, and turned on after signing out and in",
        ),
        Skip::VelaturaExtensionOutdated => t(
            "Left out: Velatura's extension is an older copy that cannot show this part. It is brought up to date with: velatura extension install, and shows it after signing out and in",
        ),
        Skip::UntestedShell(version) => tf!(
            "Left out: Velatura has not been checked with GNOME Shell {1}, so the top bar and overview are left alone. To theme them all the same: velatura apply <theme> --only gnome:shell",
            clean_text(version, 20)
        ),
        Skip::IconsNotRecolorable(name) => tf!(
            "Left out: the icons of {1} cannot be recolored. They have no color of their own to replace, or are pictures, not drawings",
            clean_text(name, 64)
        ),
        Skip::IconsTooMany(name) => tf!(
            "Left out: the icon theme {1} has more marked icons than a recolored set may hold",
            clean_text(name, 64)
        ),
        Skip::UnusualPath(p) => tf!(
            "Left out: {1} has characters in its name that cannot be written into a settings file",
            path(p, home)
        ),
        Skip::Invalid(p) => tf!(
            "Left out: {1} has an error in it, so nothing is added to it",
            path(p, home)
        ),
        Skip::OwnPalette(p) => tf!(
            "Left out: {1} already chooses a palette, and its prompt is written for the names of that one. Velatura leaves that choice alone",
            path(p, home)
        ),
        Skip::FirefoxNoProfile => {
            t("Left out: Firefox has no profile yet. Start Firefox once, then apply again")
        }
        Skip::CursorsNotRecolorable(name) => tf!(
            "Left out: the cursor theme {1} cannot be recolored. It is not installed here, or has no cursor in a form Velatura reads",
            clean_text(name, 64)
        ),
        Skip::TemplateInvalid(why) => tf!(
            "Left out: the template cannot be used: {1}",
            template_problem(why, home)
        ),
        Skip::NeedsAdwGtk3 => t(
            "Left out: older apps can only be recolored with the adw-gtk3 theme, which is not installed. It can be installed without administrator rights into ~/.local/share/themes, or from the adw-gtk3-theme package",
        ),
    }
}

/// Why a template of the user's cannot be used, after "cannot be used:".
pub fn template_problem(why: &velatura_engine::templates::Invalid, home: &Path) -> String {
    use velatura_engine::templates::Invalid;
    use velatura_theme::template::Problem;
    let _ = home;
    match why {
        Invalid::TooLarge | Invalid::Template(Problem::TooLarge) => {
            t("the file is too large for a template")
        }
        Invalid::NotToml(why) => tf!("it is not a template file ({1})", clean_text(why, 200)),
        Invalid::NewerSchema => t("it was written for a newer Velatura"),
        Invalid::MissingKey(key) => tf!("it has no {1}", key),
        Invalid::OutputPlace(output) => tf!(
            "its output, {1}, is not a file under ~/.config/ or ~/.local/share/",
            clean_text(output, 120)
        ),
        Invalid::OutputForbidden(output) => tf!(
            "its output, {1}, is in a folder that programs are started from, or is named like a file of Velatura's own",
            clean_text(output, 120)
        ),
        Invalid::OutputTaken(output) => tf!(
            "another template already writes {1}",
            clean_text(output, 120)
        ),
        Invalid::Template(Problem::UnknownName { line, name }) => tf!(
            "line {1}: no color is called {2}",
            line,
            clean_text(name, 60)
        ),
        Invalid::Template(Problem::UnknownFormat { line, format }) => tf!(
            "line {1}: no format is called {2}. The formats are hex, bare, rgb and float",
            line,
            clean_text(format, 60)
        ),
        Invalid::Template(Problem::FormatOnWord { line }) => tf!(
            "line {1}: appearance is a word, dark or light, and takes no format",
            line
        ),
        Invalid::Template(Problem::NoPlaceholder) => t("it names no color"),
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
        Note::TmuxReload { conf } => tf!(
            "tmux sessions that are running keep their colors until this is run in one of them: tmux source-file {1}",
            path(conf, home)
        ),
        Note::NeovimOtherScheme {
            name: Some(name),
            path: p,
        } => tf!(
            "Neovim's settings choose the colorscheme {1}. The line Velatura adds at the end of {2}, once agreed to, chooses the theme's after Neovim starts. The rest of the settings stay as they are",
            clean_text(name, 64),
            path(p, home)
        ),
        Note::NeovimOtherScheme {
            name: None,
            path: p,
        } => tf!(
            "Neovim's settings do not choose the theme's colorscheme. The line Velatura adds at the end of {1}, once agreed to, chooses it after Neovim starts. The rest of the settings stay as they are",
            path(p, home)
        ),
        Note::NeovimReload => t(
            "Open Neovim windows take the new colors the next time they are focused, or with :colorscheme velatura",
        ),
        Note::StarshipNames => t(
            "The prompt shows the theme's colors where Starship's settings name them, such as accent, surface or directory",
        ),
        Note::PtyxisChoose => t(
            "Only the default profile of Ptyxis is given the palette. For another, choose Velatura under Preferences in Ptyxis",
        ),
        Note::PtyxisReopen => {
            t("If open Ptyxis windows keep the old colors, close and reopen them")
        }
        Note::GtkRestart => t("Apps that are already open keep their colors until reopened"),
        Note::FirefoxRestart => t("Firefox shows the change after it is restarted"),
        Note::IconsReopen => t(
            "Open apps show the recolored icons once they are reopened. Signing out and in shows them everywhere. Icons that apps keep in the top bar, such as Solaar's, change once the AppIndicator extension is turned off and on again, or after signing out and in",
        ),
        Note::CursorReopen => t(
            "Open apps may keep the old cursor until they are reopened. Signing out and in shows the new one everywhere",
        ),
        Note::FirefoxOtherProfiles => t(
            "Only the profile Firefox opens by default is given the theme. Its other profiles are left as they are",
        ),
        Note::FirefoxPrefStays => t(
            "Firefox goes on reading the userChrome.css file of the profile. To turn that off, set toolkit.legacyUserProfileCustomizations.stylesheets to false in about:config",
        ),
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
        Note::ExtensionWaiting => t(
            "Velatura's extension was updated, and the Shell runs the earlier copy until signing out and in. This part shows after that",
        ),
        Note::ShellShortList => t(
            "The top bar's own stylesheet could not be read, so only its main surfaces are recolored, and it keeps the nearest system accent",
        ),
        Note::UserThemesOff => t(
            "The User Themes extension is turned off, so the top bar keeps its colors. Turn it on in the Extensions app",
        ),
        Note::ShellExtensionStyles { names } => tf!(
            "These extensions color the top bar or its menus themselves, which wins over the theme there: {1}. Velatura's own extension wins over them: velatura extension install",
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
        OtherTool::PinnedColors { name, keys } => tf!(
            "The extension {1} keeps colors of its own in its settings ({2}). They stay the same whatever theme is applied",
            clean_text(name, 80),
            clean_text(&keys.join(", "), 300)
        ),
    }
}

/// Whether the themes change by the clock, and when.
pub fn schedule(s: &Status) -> String {
    if !s.on {
        return t("Scheduled switching is off");
    }
    let line = if s.follows_style {
        t(
            "The themes follow the dark style: the daytime theme with the light style, the nighttime theme with the dark style",
        )
    } else {
        tf!(
            "Scheduled switching is on: the daytime theme from {1}, the nighttime theme from {2}",
            format_time(s.times.day),
            format_time(s.times.night)
        )
    };
    if s.damaged {
        format!(
            "{line}. {}",
            t(
                "Its timer was changed or removed outside Velatura. To repair it: velatura schedule set"
            )
        )
    } else {
        line
    }
}

/// What the last scheduled switch left for the user, if anything.
pub fn last_run(last: &LastRun) -> Option<String> {
    match last.problem {
        Some(Problem::NoTheme) => Some(match last.period {
            Period::Day => t("The last scheduled switch did nothing: no daytime theme is chosen"),
            Period::Night => {
                t("The last scheduled switch did nothing: no nighttime theme is chosen")
            }
        }),
        Some(Problem::Failed) => Some(t(
            "The last scheduled switch failed. Applying the theme by hand shows why",
        )),
        None if last.left_out.is_empty() => None,
        None => Some(tf!(
            "The last scheduled switch left out: {1}. Applying the theme by hand shows why",
            last.left_out
                .iter()
                .map(|i| instance(i))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Where Velatura's own Shell extension stands, and what is left to do.
pub fn extension(s: &companion::Status) -> String {
    match (&s.installed, s.enabled) {
        (None, _) => t("Velatura's extension is not installed"),
        (Some(_), Some(true)) if s.differs => t(
            "Velatura's extension is on. A newer copy comes with this version: install it again, then sign out and in",
        ),
        (Some(_), Some(true)) if s.waiting => t(
            "Velatura's extension is on, and a newer copy is installed. The Shell takes it after signing out and in",
        ),
        (Some(_), Some(true)) => {
            t("Velatura's extension is on: the top bar and overview are themed through it")
        }
        (Some(_), None) => t(
            "Velatura's extension is installed. Whether it is on can only be read in a desktop session",
        ),
        (Some(_), Some(false)) => tf!(
            "Velatura's extension is installed and turned off. After signing out and in once, turn it on in the Extensions app, or with: gnome-extensions enable {1}",
            companion::UUID
        ),
    }
}

/// Why the schedule was not turned on or off.
pub fn refused(why: &Refused, home: &Path) -> String {
    match why {
        Refused::NoThemes => t("Choose a daytime and a nighttime theme first"),
        Refused::SameTime => t("The two themes cannot start at the same time"),
        Refused::NotAPair => t(
            "Following the dark style needs a light theme for the day and a dark theme for the night",
        ),
        Refused::NotInstalled(p) => tf!(
            "A schedule needs the velatura command installed where it stays, such as ~/.local/bin. This copy is at {1}",
            path(p, home)
        ),
        Refused::OtherTimers(units) => tf!(
            "Another timer that may change the theme is turned on: {1}. Two schedules would work against each other",
            clean_text(&units.join(", "), 200)
        ),
        Refused::Foreign(p) => tf!(
            "{1} already exists and was not created by Velatura",
            path(p, home)
        ),
        Refused::OutsideHome(p) => tf!("{1} leads outside the home folder", path(p, home)),
        Refused::Busy => t("Another Velatura is making changes. Try again in a moment"),
        Refused::Io(why) => clean_text(why, 300),
        Refused::Systemd(why) => tf!(
            "The timer could not be turned on or off ({1})",
            clean_text(why, 200)
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

/// What is wrong with one of Velatura's own files.
pub fn store_error(e: &velatura_engine::store::StoreError, home: &Path) -> String {
    use velatura_engine::store::StoreError;
    match e {
        StoreError::Io(p, why) => tf!(
            "{1} cannot be read ({2})",
            path(p, home),
            clean_text(&why.to_string(), 200)
        ),
        StoreError::Damaged(p) => tf!("{1} is damaged", path(p, home)),
        StoreError::Newer(p) => tf!(
            "{1} was written by a newer version of Velatura",
            path(p, home)
        ),
    }
}

/// Velatura's own records, when they cannot be used.
pub fn records_problem(e: &velatura_engine::store::StoreError, home: &Path) -> String {
    tf!(
        "Velatura's own records cannot be used: {1}",
        store_error(e, home)
    )
}

/// Why an operation did not happen or did not finish, for the command and
/// the app alike. The command adds how to go on from a terminal.
pub fn engine_error(e: &velatura_engine::engine::Error, home: &Path) -> String {
    use velatura_engine::engine::Error;
    match e {
        Error::Busy => t("Another Velatura is making changes. Try again in a moment."),
        Error::NothingToUndo => t("There is nothing to undo."),
        Error::UnreadableBackup(_) | Error::ForeignBackup { .. } => {
            backup_refused(e).unwrap_or_default()
        }
        Error::Store(why) => format!(
            "{}. {}",
            records_problem(why, home),
            t("Nothing was changed.")
        ),
        Error::Io(why) => clean_text(why, 300),
        Error::Recovery(r) => format!(
            "{}\n{}",
            t(
                "An earlier interrupted change could not be fully rolled back, so nothing new was changed."
            ),
            rollback(r)
        ),
        Error::Failed(f) => failure(f),
    }
}

/// Why an undo was refused before anything was changed: a backup in the
/// way that cannot be trusted. `None` for any other error.
pub fn backup_refused(e: &velatura_engine::engine::Error) -> Option<String> {
    use velatura_engine::engine::Error;
    let shown = |p: &Path| clean_text(&p.to_string_lossy(), 400);
    match e {
        Error::UnreadableBackup(backup) => Some(tf!(
            "Nothing was undone: the backup {1} cannot be read, and it may hold the change to undo first. Moving that file out of the backups folder lets older changes be undone.",
            shown(backup)
        )),
        Error::ForeignBackup { backup, names } => Some(tf!(
            "Nothing was undone: the backup {1} names {2}, which is outside this home folder, so it was made for another one. Moving that file out of the backups folder lets older changes be undone.",
            shown(backup),
            shown(names)
        )),
        _ => None,
    }
}

/// Why a theme was not saved, imported, exported or removed.
pub fn library_refused(why: &LibraryRefused, home: &Path) -> String {
    match why {
        LibraryRefused::Reserved(id) => tf!(
            "\"{1}\" is a theme that comes with Velatura and cannot be replaced or removed",
            clean_text(id, 64)
        ),
        LibraryRefused::Exists(id) => {
            tf!(
                "A theme with the id \"{1}\" already exists",
                clean_text(id, 64)
            )
        }
        LibraryRefused::BadId(id) => tf!(
            "\"{1}\" cannot be a theme's id. An id is made of lower-case letters, digits and hyphens",
            clean_text(id, 64)
        ),
        LibraryRefused::NotFound(id) => {
            tf!(
                "No theme of this user has the id \"{1}\"",
                clean_text(id, 64)
            )
        }
        LibraryRefused::FileInTheWay(p) => tf!(
            "{1} already exists and is not that theme's file",
            path(p, home)
        ),
        LibraryRefused::NotAThemeFile => t("Not a theme file"),
        LibraryRefused::Invalid(invalid) => tf!(
            "Not a valid theme file: {1}",
            clean_text(&invalid.to_string().replace('\n', "; "), 400)
        ),
        LibraryRefused::Incomplete(lacking) if lacking.missing.is_empty() => tf!(
            "{1} that could not be made into a theme",
            format_name(lacking.format)
        ),
        LibraryRefused::Incomplete(lacking) => tf!(
            "{1} without all the colors a theme needs. Missing: {2}",
            format_name(lacking.format),
            clean_text(&lacking.missing.join(", "), 300)
        ),
        LibraryRefused::Io(why) => clean_text(why, 300),
    }
}

fn format_name(format: Format) -> String {
    match format {
        Format::Base16 => t("A base16 scheme"),
        Format::Base24 => t("A base24 scheme"),
        Format::Kitty => t("A Kitty theme"),
    }
}

/// What an import says of a file that was not a theme file of Velatura's.
pub fn imported_from(format: Format) -> String {
    tf!(
        "{1}, read as a theme. Colors it does not give were worked out from the ones it does",
        format_name(format)
    )
}

/// What every import says: a theme file is data, not a program.
pub fn import_promise() -> String {
    t("Theme files hold only colors and names. They cannot run anything")
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

    const TEMPLATE: &str = include_str!("../../../po/velatura.pot");

    /// The words in braces of a text: `{1}`, `{n}`, `{name}`.
    fn placeholders(text: &str) -> Vec<&str> {
        let mut found: Vec<&str> = text
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect();
        found.sort_unstable();
        found
    }

    /// Every sentence has a Spanish one, with the same things to fill in,
    /// the same line endings, an access key where the source has one, and
    /// the same beginning for every reason for leaving something out.
    #[test]
    fn the_spanish_catalog_is_complete_and_keeps_what_must_be_kept() {
        let source = catalog::Catalog::parse(
            &TEMPLATE
                .replace("msgstr \"\"", "msgstr \"x\"")
                .replace("] \"\"", "] \"x\""),
        );
        let es = catalog::for_language("es").unwrap();
        assert_eq!(
            es.len(),
            source.len(),
            "po/es.po and po/velatura.pot differ in size"
        );
        assert!(es.len() > 300);
        let ids: Vec<String> = TEMPLATE
            .split("\n\n")
            .skip(1)
            .filter_map(|block| {
                let start = block.find("msgid \"")? + 7;
                let end = block[start..].find("\"\n").map(|i| start + i)?;
                Some(block[start..end].replace("\\\"", "\"").replace("\\n", "\n"))
            })
            .collect();
        assert_eq!(ids.len(), source.len());
        let prefix = es.find("Left out: ");
        for id in &ids {
            for (text, said) in [
                (es.find(id), id.as_str()),
                (es.find_plural(id, id, 1), id.as_str()),
            ] {
                assert_eq!(placeholders(text), placeholders(said), "{id}");
                assert_eq!(text.ends_with('\n'), said.ends_with('\n'), "{id}");
                assert_eq!(text.starts_with("  "), said.starts_with("  "), "{id}");
                assert_eq!(text.contains('_'), said.contains('_'), "access key: {id}");
                assert_eq!(text.ends_with('.'), said.ends_with('.'), "full stop: {id}");
                assert_eq!(text.ends_with('…'), said.ends_with('…'), "{id}");
            }
            if id.starts_with("Left out: ") {
                assert!(es.find(id).starts_with(prefix), "{id}");
            }
            // Nothing is left in English by oversight: names are the same
            // in both languages, sentences are not.
            let same_on_purpose = [
                "Kitty",
                "Ghostty",
                "tmux",
                "Neovim",
                "Starship",
                "Ptyxis",
                "Firefox",
                "General",
                "Cursor",
                "Error",
                "Variable",
                "Magenta",
                "_Terminal",
            ];
            if !same_on_purpose.contains(&id.as_str()) {
                assert!(
                    es.find(id) != id.as_str() || es.find_plural(id, id, 1) != id.as_str(),
                    "not translated: {id}"
                );
            }
        }
        assert_eq!(
            es.find_plural("Creates {n} file.", "Creates {n} files.", 1),
            "Crea {n} archivo."
        );
        assert_eq!(
            es.find_plural("Creates {n} file.", "Creates {n} files.", 3),
            "Crea {n} archivos."
        );
        assert_eq!(es.find_plural("no such", "no such ones", 2), "no such ones");
        assert_eq!(es.find("no such sentence"), "no such sentence");
    }

    #[test]
    fn the_language_is_taken_from_the_session_the_way_gettext_does() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let spanish = |pairs| catalog::from_environment(&env(pairs)).is_some();
        assert!(spanish(&[("LANG", "es_ES.UTF-8")]));
        assert!(spanish(&[("LANG", "es_MX.UTF-8@euro")]));
        assert!(spanish(&[
            ("LANG", "en_US.UTF-8"),
            ("LC_MESSAGES", "es_AR.UTF-8")
        ]));
        assert!(spanish(&[
            ("LANG", "en_US.UTF-8"),
            ("LANGUAGE", "fr:es:en")
        ]));
        assert!(!spanish(&[("LANG", "en_US.UTF-8")]));
        assert!(!spanish(&[]));
        assert!(!spanish(&[("LANG", "es_ES.UTF-8"), ("LC_ALL", "C.UTF-8")]));
        assert!(!spanish(&[("LANG", "C"), ("LANGUAGE", "es")]));
        assert!(!spanish(&[("LANG", "es_ES.UTF-8"), ("LANGUAGE", "de:fr")]));
        assert!(catalog::for_language("pt_BR").is_none());
    }

    #[test]
    fn a_catalog_is_read_with_its_escapes_plurals_and_long_lines() {
        let c = catalog::Catalog::parse(
            "msgid \"\"\nmsgstr \"\"\n\"Language: xx\\n\"\n\n# a comment\nmsgid \"a \\\"b\\\"\\n\"\nmsgstr \"\"\n\"A \"\n\"«b»\\n\"\n\n#, fuzzy\nmsgid \"unsure\"\nmsgstr \"guess\"\n\nmsgid \"empty\"\nmsgstr \"\"\n\nmsgid \"one\"\nmsgid_plural \"many\"\nmsgstr[0] \"uno\"\nmsgstr[1] \"muchos\"\n",
        );
        assert_eq!(c.find("a \"b\"\n"), "A «b»\n");
        assert_eq!(c.find("unsure"), "unsure", "a fuzzy entry is not used");
        assert_eq!(c.find("empty"), "empty");
        assert_eq!(
            (
                c.find_plural("one", "many", 1),
                c.find_plural("one", "many", 0)
            ),
            ("uno", "muchos")
        );
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn the_extension_says_what_is_left_to_do() {
        let on = companion::Status {
            installed: Some(std::path::PathBuf::from("/x")),
            by_velatura: true,
            enabled: Some(true),
            ..companion::Status::default()
        };
        assert!(extension(&on).contains("themed through it"));
        let waiting = companion::Status {
            waiting: true,
            ..on.clone()
        };
        assert!(extension(&waiting).contains("after signing out and in"));
        // Settings that cannot be read say nothing about on or off.
        let unknown = companion::Status {
            enabled: None,
            ..on.clone()
        };
        assert!(extension(&unknown).contains("can only be read in a desktop session"));
        assert!(!extension(&unknown).contains("turned off"));
        let off = companion::Status {
            enabled: Some(false),
            ..on
        };
        assert!(extension(&off).contains("turned off"));
    }

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
