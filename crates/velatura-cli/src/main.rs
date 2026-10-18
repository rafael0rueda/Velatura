//! The `velatura` command.
//!
//! Arguments in, typed engine calls, text or JSON out. No logic that the
//! GUI would have to repeat lives here.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use velatura_engine::engine::{self, ApplyOpts, Error, Health, PartState, Report};
use velatura_engine::firefox;
use velatura_engine::host::{Dirs, Fs, Host, Settings, StdFs, StdSystemd, self_command};
use velatura_engine::library;
use velatura_engine::look;
use velatura_engine::plan::{Cause, Change, Note, Plan, Skip};
use velatura_engine::schedule::{self, Auto, Period, Refused, Times};
use velatura_engine::store::DayNight;
use velatura_engine::targets;
use velatura_engine::templates;
use velatura_text as text;
use velatura_theme::model::{ANSI_NAMES, clean_text};
use velatura_theme::shell::BarStyle;
use velatura_theme::{Color, Resolved, nearest_accent, parse_theme};

const OK: u8 = 0;
const FAILED: u8 = 1;
const USAGE: u8 = 2;
const NEEDS_AGREEMENT: u8 = 3;
const BUSY: u8 = 4;
const PARTIAL: u8 = 5;

/// Version of the JSON output. Keys are only ever added within a version.
const JSON_FORMAT: u32 = 1;

/// Standard output, minus the panic when the reader has gone away, as in
/// `velatura status | head -1`: the rest of the text is dropped and the
/// command finishes what it was doing. Any other failure panics, as the
/// standard macros do.
fn out(args: std::fmt::Arguments<'_>) {
    if let Err(e) = std::io::stdout().write_fmt(args)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        panic!("failed printing to stdout: {e}");
    }
}

// These shadow the standard macros for the whole of this file.
macro_rules! print {
    ($($arg:tt)*) => { out(format_args!($($arg)*)) };
}
macro_rules! println {
    () => { out(format_args!("\n")) };
    ($($arg:tt)*) => { out(format_args!("{}\n", format_args!($($arg)*))) };
}

#[derive(Default)]
struct Args {
    words: Vec<String>,
    preview: bool,
    yes: bool,
    replace_changed: bool,
    strict: bool,
    json: bool,
    original: bool,
    again: bool,
    replace: bool,
    rename: Option<String>,
    accent: Option<String>,
    day: Option<String>,
    night: Option<String>,
    only: Vec<String>,
    except: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--preview" | "--dry-run" => a.preview = true,
            "--yes" | "-y" => a.yes = true,
            "--replace-changed" => a.replace_changed = true,
            "--strict" => a.strict = true,
            "--json" => a.json = true,
            "--original" => a.original = true,
            "--again" => a.again = true,
            "--replace" => a.replace = true,
            "--as" => a.rename = Some(it.next().ok_or("--as needs an id")?),
            "--accent" => {
                a.accent = Some(
                    it.next()
                        .ok_or("--accent needs a color's name or #rrggbb")?,
                );
            }
            "--day" | "--night" => {
                let value = it.next().ok_or(format!("{arg} needs a time, as HH:MM"))?;
                if arg == "--day" {
                    a.day = Some(value);
                } else {
                    a.night = Some(value);
                }
            }
            "--only" | "--except" => {
                let value = it.next().ok_or(format!("{arg} needs a target name"))?;
                if arg == "--only" {
                    a.only.push(value);
                } else {
                    a.except.push(value);
                }
            }
            "-h" | "--help" => a.words.insert(0, "help".to_owned()),
            "-V" | "--version" => a.words.insert(0, "version".to_owned()),
            s if s.starts_with('-') => return Err(format!("unknown option {}", clean_text(s, 40))),
            _ => a.words.push(arg),
        }
    }
    Ok(a)
}

#[cfg(feature = "gio")]
fn settings() -> Box<dyn Settings> {
    Box::new(velatura_engine::gio_settings::GioSettings::new())
}

#[cfg(not(feature = "gio"))]
fn settings() -> Box<dyn Settings> {
    Box::new(velatura_engine::host::NoSettings)
}

fn env(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("velatura: {e}\nTry: velatura --help");
            return ExitCode::from(USAGE);
        }
    };
    let words: Vec<&str> = args.words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["help", ..] => {
            print!("{}", text::HELP);
            return ExitCode::from(OK);
        }
        ["version", ..] => {
            println!("velatura {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::from(OK);
        }
        ["validate", file] => return ExitCode::from(validate(file, &args)),
        _ => {}
    }

    let Some(dirs) = Dirs::from_env(env) else {
        eprintln!("velatura: HOME is not set to an absolute path");
        return ExitCode::from(FAILED);
    };
    let settings = settings();
    let host = Host {
        dirs: &dirs,
        fs: &StdFs,
        settings: settings.as_ref(),
        now: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    let cx = Cx {
        host,
        home: &dirs.home,
        args: &args,
    };
    let code = match words.as_slice() {
        [] | ["status"] => cx.status(words.is_empty()),
        ["list"] => cx.list(),
        ["show", theme] => cx.show(theme),
        ["targets"] => cx.targets(),
        ["apply", theme] => cx.apply(theme),
        ["apply"] => Err(usage(
            "apply needs a theme, for example: velatura apply ember",
        )),
        ["switch"] => cx.switch_show(),
        ["switch", "set", which @ ("day" | "night"), theme] => cx.switch_set(which, theme),
        ["switch", which @ ("day" | "night" | "toggle")] => cx.switch(which),
        ["switch", "auto"] => cx.switch_auto(),
        ["schedule"] | ["schedule", "status"] => cx.schedule_show(),
        ["schedule", "set"] => cx.schedule_set(),
        ["schedule", "off"] => cx.schedule_off(),
        ["schedule", "follow"] => cx.schedule_follow(),
        ["top-bar"] => cx.top_bar_show(),
        ["top-bar", "style", style] => cx.top_bar_set(Some(style), None),
        ["top-bar", "opacity", percent] => cx.top_bar_set(None, Some(percent)),
        ["extension"] | ["extension", "status"] => cx.extension_show(),
        ["extension", "install"] => cx.extension_install(),
        ["extension", "remove"] => cx.extension_remove(),
        ["theme", "import", file] => cx.theme_import(file),
        ["theme", "export", theme] => cx.theme_export(theme, None),
        ["theme", "export", theme, file] => cx.theme_export(theme, Some(file)),
        ["theme", "resolve", theme] => cx.theme_resolve(theme),
        ["theme", "remove", theme] => cx.theme_remove(theme),
        ["template"] | ["template", "list"] => cx.template_list(),
        ["template", "add", file] => cx.template_add(file),
        ["template", "remove", id] => cx.template_remove(id),
        ["look"] => cx.look_show(),
        ["look", "list", what] => cx.look_list(what),
        ["look", "set", what, value] => cx.look_set(what, Some(value)),
        ["look", "reset", what] => cx.look_set(what, None),
        ["undo"] | ["revert"] => cx.undo(None),
        ["history"] => cx.history(),
        ["restore"] if args.original => cx.restore_original(),
        ["restore", id] => match id.parse::<u64>() {
            Ok(id) => cx.undo(Some(id)),
            Err(_) => Err(usage(
                "restore needs a backup number from: velatura history",
            )),
        },
        ["recover"] => cx.recover(),
        ["doctor"] => cx.doctor(),
        _ => Err(usage("unknown command")),
    };
    ExitCode::from(match code {
        Ok(code) => code,
        Err(Exit { code, message }) => {
            eprintln!("velatura: {message}");
            code
        }
    })
}

struct Exit {
    code: u8,
    message: String,
}

fn usage(message: &str) -> Exit {
    Exit {
        code: USAGE,
        message: format!("{message}\nTry: velatura --help"),
    }
}

impl From<Error> for Exit {
    fn from(e: Error) -> Self {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let mut message = text::engine_error(&e, &home);
        // From a terminal, the way on is a command.
        if let Error::Recovery(_) = e {
            message.push_str("To try again: velatura recover");
        }
        let code = match e {
            Error::Busy => BUSY,
            _ => FAILED,
        };
        Self { code, message }
    }
}

fn validate(file: &str, args: &Args) -> u8 {
    // Read through the host layer like everything else, and size-capped by
    // the parser before any of it is interpreted.
    let src = match StdFs
        .read(Path::new(file))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("velatura: {}: {e}", clean_text(file, 300));
            return FAILED;
        }
    };
    match parse_theme(&src) {
        Err(invalid) => {
            eprintln!("{invalid}");
            FAILED
        }
        Ok(loaded) => {
            for w in &loaded.warnings {
                eprintln!("warning: {w}");
            }
            if args.strict && !loaded.warnings.is_empty() {
                return FAILED;
            }
            println!(
                "Valid: {} ({}), {} variant(s)",
                loaded.theme.name,
                loaded.theme.id,
                loaded.theme.variants.len()
            );
            OK
        }
    }
}

/// True if an applied `theme/variant` is the theme a choice names. A choice
/// without a variant means the theme's default one.
fn same_theme(label: &str, spec: &str) -> bool {
    label == spec || (!spec.contains('/') && label.split('/').next() == Some(spec))
}

fn skip_code(s: &Skip) -> &'static str {
    match s {
        Skip::NotInstalled => "not-installed",
        Skip::NotSelected => "not-selected",
        Skip::NoConsent => "no-agreement",
        Skip::OutsideHome(_) => "outside-home",
        Skip::NotAFile(_) => "not-a-file",
        Skip::Unreadable(_) => "unreadable",
        Skip::MalformedBlock(_) => "damaged-markers",
        Skip::ForeignFile(_) => "foreign-file",
        Skip::ChangedOutside(_) => "changed-outside",
        Skip::SettingsUnavailable(_) => "settings-unavailable",
        Skip::HardLinked(_) => "hard-linked",
        Skip::HighContrast => "high-contrast",
        Skip::NeedsAdwGtk3 => "needs-adw-gtk3",
        Skip::NeedsUserThemes => "needs-user-themes",
        Skip::NeedsVelaturaExtension => "needs-velatura-extension",
        Skip::VelaturaExtensionOutdated => "velatura-extension-outdated",
        Skip::UntestedShell(_) => "untested-shell",
        Skip::UnusualPath(_) => "unusual-path",
        Skip::IconsNotRecolorable(_) => "icons-not-recolorable",
        Skip::IconsTooMany(_) => "icons-too-many",
        Skip::Invalid(_) => "invalid-file",
        Skip::OwnPalette(_) => "own-palette",
        Skip::FirefoxNoProfile => "firefox-no-profile",
        Skip::TemplateInvalid(_) => "template-invalid",
        Skip::CursorsNotRecolorable(_) => "cursors-not-recolorable",
    }
}

fn note_code(n: &Note) -> &'static str {
    match n {
        Note::Override { .. } => "override",
        Note::GhosttyReload => "ghostty-reload",
        Note::TmuxReload { .. } => "tmux-reload",
        Note::NeovimOtherScheme { .. } => "neovim-other-scheme",
        Note::NeovimReload => "neovim-reload",
        Note::StarshipNames => "starship-names",
        Note::PtyxisChoose => "ptyxis-choose",
        Note::PtyxisReopen => "ptyxis-reopen",
        Note::GtkRestart => "gtk-restart",
        Note::FirefoxRestart => "firefox-restart",
        Note::IconsReopen => "icons-reopen",
        Note::CursorReopen => "cursor-reopen",
        Note::FirefoxOtherProfiles => "firefox-other-profiles",
        Note::FirefoxPrefStays => "firefox-pref-stays",
        Note::LinkedFile { .. } => "linked-file",
        Note::Gtk3DarkForced { .. } => "gtk3-dark-forced",
        Note::SettingNeedsSession { .. } => "setting-needs-session",
        Note::UserThemesOff => "user-themes-off",
        Note::ShellShortList => "shell-short-list",
        Note::ShellExtensionStyles { .. } => "shell-extension-styles",
        Note::KittyAutoTheme { .. } => "kitty-auto-theme",
        Note::KittyReload => "kitty-reload",
        Note::AccentNearest { .. } => "accent-nearest",
        Note::AccentUnsupported => "accent-unsupported",
        Note::SettingLocked { .. } => "setting-locked",
        Note::KeptFile { .. } => "kept-file",
        Note::KeptSetting { .. } => "kept-setting",
        Note::LowContrast => "low-contrast",
    }
}

/// The time of day where this machine is, in minutes after midnight.
#[cfg(feature = "gio")]
fn local_minute(now: u64) -> u16 {
    velatura_engine::gio_settings::local_minute(now).unwrap_or((now / 60 % 1440) as u16)
}

/// Without the desktop libraries there is no time zone to ask: UTC.
#[cfg(not(feature = "gio"))]
fn local_minute(now: u64) -> u16 {
    (now / 60 % 1440) as u16
}

impl From<Refused> for Exit {
    fn from(why: Refused) -> Self {
        Self {
            code: match why {
                Refused::Busy => BUSY,
                Refused::Io(_) | Refused::Systemd(_) => FAILED,
                _ => USAGE,
            },
            message: String::new(),
        }
    }
}

fn change_json(c: &Change) -> Value {
    let p = |p: &Path| p.to_string_lossy().into_owned();
    match c {
        Change::CreateDir { path } => json!({"op": "create-dir", "path": p(path)}),
        Change::RemoveDir { path } => json!({"op": "remove-dir", "path": p(path)}),
        Change::Owned { path, content, .. } => json!({
            "op": if content.is_some() { "write-owned" } else { "remove-owned" }, "path": p(path),
        }),
        Change::Block {
            path, body_after, ..
        } => json!({
            "op": if body_after.is_some() { "set-block" } else { "remove-block" }, "path": p(path),
        }),
        Change::Setting {
            schema, key, value, ..
        } => json!({"op": "setting", "schema": schema, "key": key, "value": value}),
    }
}

struct Cx<'a> {
    host: Host<'a>,
    home: &'a Path,
    args: &'a Args,
}

type Outcome = Result<u8, Exit>;

impl Cx<'_> {
    fn emit(&self, value: Value) {
        let mut v = value;
        v["format"] = json!(JSON_FORMAT);
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    fn list(&self) -> Outcome {
        let (entries, rejected) = library::load(&self.host);
        if self.args.json {
            self.emit(json!({
                "themes": entries.iter().map(|e| json!({
                    "id": e.theme.id, "name": e.theme.name, "bundled": e.bundled,
                    "variants": e.theme.variants.iter().map(|(k, v)| json!({
                        "id": k, "name": v.name, "appearance": v.appearance.as_str(),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "rejected": rejected.iter().map(|r| r.file.clone()).collect::<Vec<_>>(),
            }));
            return Ok(OK);
        }
        for e in &entries {
            for (key, v) in &e.theme.variants {
                let id = if key == e.theme.default_variant() {
                    e.theme.id.clone()
                } else {
                    format!("{}/{key}", e.theme.id)
                };
                println!(
                    "{id:<24} {:<6} {}{}",
                    v.appearance.as_str(),
                    v.name,
                    if e.bundled { "" } else { "  (yours)" }
                );
            }
        }
        for r in &rejected {
            eprintln!("Not loaded: {}: {}", r.file, text::rejected(&r.why));
        }
        Ok(OK)
    }

    fn with_theme<T>(
        &self,
        spec: &str,
        f: impl FnOnce(&Resolved<'_>, &str) -> T,
    ) -> Result<T, Exit> {
        let (id, variant) = match spec.split_once('/') {
            Some((id, v)) => (id, Some(v)),
            None => (spec, None),
        };
        let (entries, _) = library::load(&self.host);
        let not_found = || Exit {
            code: USAGE,
            message: format!(
                "No theme named \"{}\". To see the themes: velatura list",
                clean_text(spec, 80)
            ),
        };
        let entry = entries
            .iter()
            .find(|e| e.theme.id == id)
            .ok_or_else(not_found)?;
        let resolved = Resolved::new(&entry.theme, variant).ok_or_else(|| Exit {
            code: USAGE,
            message: format!(
                "The theme \"{}\" has no variant \"{}\". Its variants: {}",
                clean_text(id, 80),
                clean_text(variant.unwrap_or_default(), 80),
                entry
                    .theme
                    .variants
                    .keys()
                    .map(|v| clean_text(v, 80))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })?;
        Ok(f(&resolved, &entry.theme.name))
    }

    fn show(&self, spec: &str) -> Outcome {
        self.with_theme(spec, |r, name| {
            let v = r.colors;
            println!("{name} ({}), {}", r.label(), v.appearance.as_str());
            let accent = nearest_accent(v.ui.accent);
            println!("System accent: {}", accent.setting_value());
            println!(
                "Contrast, text on background: {:.1} to 1",
                v.ui.fg.contrast(v.ui.bg)
            );
            println!("\nInterface");
            for (role, color) in v.ui.roles() {
                let mark = if v.derived.contains(&role) {
                    "  (derived)"
                } else {
                    ""
                };
                println!("  {role:<14} {}{mark}", color.hex());
            }
            println!("\nTerminal");
            for (role, color) in ANSI_NAMES.iter().zip(v.ansi) {
                println!("  {role:<14} {}", color.hex());
            }
            OK
        })
    }

    fn targets(&self) -> Outcome {
        let h = &self.host;
        let p = |path: std::path::PathBuf| text::path(&path, self.home);
        let rows = [
            (
                "kitty",
                "Kitty",
                "official",
                p(targets::kitty_dir(h, &env).join("kitty.conf")),
            ),
            (
                "ghostty",
                "Ghostty",
                "official",
                p(targets::ghostty_dir(h).join(targets::ghostty_conf(h))),
            ),
            (
                "gtk4",
                "Apps (GTK 4 and libadwaita)",
                "unofficial",
                p(h.dirs.config.join("gtk-4.0/gtk.css")),
            ),
            (
                "gtk3",
                "Older Apps (GTK 3)",
                "unofficial",
                p(h.dirs.config.join("gtk-3.0/gtk.css")),
            ),
            ("tmux", "tmux", "official", p(targets::tmux_conf(h))),
            (
                "nvim",
                "Neovim",
                "official",
                p(h.dirs.config.join("nvim/colors/velatura.lua")),
            ),
            (
                "starship",
                "Starship",
                "official",
                p(targets::starship_conf(h, &env)),
            ),
            (
                "ptyxis",
                "Ptyxis",
                "official",
                p(h.dirs
                    .data
                    .join("org.gnome.Ptyxis/palettes/velatura.palette")),
            ),
            (
                "texteditor",
                "Text Editor",
                "official",
                p(h.dirs.data.join("gtksourceview-5/styles/velatura.xml")),
            ),
            (
                "firefox",
                "Firefox (off until turned on)",
                "unofficial",
                firefox::roots(h)
                    .into_iter()
                    .find_map(|root| firefox::default_profile(h, &root))
                    .map_or_else(
                        || "its default profile".to_owned(),
                        |(profile, _)| p(profile.join("chrome/userChrome.css")),
                    ),
            ),
            (
                "icons:recolor",
                "Recolored Icons (off until turned on)",
                "official",
                p(h.dirs.data.join("icons")),
            ),
            (
                "cursors:recolor",
                "Recolored Cursor (off until turned on)",
                "official",
                p(h.dirs.data.join("icons")),
            ),
            (
                "gnome:top-bar",
                "Top Bar Style (off until turned on)",
                "official",
                p(velatura_engine::companion::top_bar_stylesheet(h)),
            ),
            (
                "gnome:ups",
                "UPS Indicator (off until turned on)",
                "official",
                p(velatura_engine::companion::ups_stylesheet(h)),
            ),
            (
                "gnome:outline",
                "Window Outline (off until turned on)",
                "official",
                p(velatura_engine::companion::outline_stylesheet(h)),
            ),
            (
                "flatpak:global",
                "Flatpak Apps (off until turned on)",
                "unofficial",
                p(h.dirs.data.join("flatpak/overrides/global")),
            ),
            (
                "gnome:shell",
                "Top Bar and Overview",
                "unofficial",
                "User Themes extension".to_owned(),
            ),
            (
                "gnome:appearance",
                "Style and Accent Color",
                "official",
                "desktop settings".to_owned(),
            ),
        ];
        // As wide as the longest name, so the columns stay in line.
        let wide = rows.iter().map(|r| r.1.chars().count()).max().unwrap_or(0);
        for (id, name, support, place) in rows {
            println!("{id:<17} {name:<wide$} {support:<11} {place}");
        }
        println!("\nUnofficial: not supported by GNOME; may stop working after an update.");
        Ok(OK)
    }

    fn status(&self, with_hint: bool) -> Outcome {
        let s = engine::status(&self.host)?;
        if self.args.json {
            let health = |h: &Health| match h {
                Health::Ok => "ok",
                Health::Changed => "changed",
                Health::Missing => "missing",
            };
            self.emit(json!({
                "applied": s.applied.as_ref().map(|(theme, _)| theme),
                "unfinished": s.unfinished,
                "files": s.files.iter().map(|(p, h)| json!({"path": p, "state": health(h)})).collect::<Vec<_>>(),
                "blocks": s.blocks.iter().map(|(p, h)| json!({"path": p, "state": health(h)})).collect::<Vec<_>>(),
                "settings": s.settings.iter().map(|(k, v, h)| json!({"key": k, "written": v, "state": health(h)})).collect::<Vec<_>>(),
                "schedule": self.schedule_json(&schedule::status(&self.host)),
            }));
            return Ok(OK);
        }
        match &s.applied {
            Some((theme, _)) => println!("Theme in use: {theme}"),
            None => println!("No theme applied."),
        }
        if s.unfinished {
            println!("An earlier change was interrupted. To roll it back: velatura recover");
        }
        let word = |h: &Health| match h {
            Health::Ok => "in place",
            Health::Changed => "changed outside Velatura",
            Health::Missing => "missing",
        };
        for (p, h) in s.files.iter().chain(&s.blocks) {
            println!("  {:<44} {}", text::path(Path::new(p), self.home), word(h));
        }
        for (key, _, h) in &s.settings {
            println!(
                "  {:<44} {}",
                key.rsplit(' ').next().unwrap_or(key),
                word(h)
            );
        }
        // Each part, in words: themed, ready, hidden by something, or why not.
        let (themes, _) = library::load(&self.host);
        let parts = engine::parts(&self.host, &env, &themes);
        if !parts.is_empty() {
            println!();
        }
        for part in &parts {
            let state = match &part.state {
                PartState::Themed if part.included => "themed".to_owned(),
                PartState::Themed => "turned off, and taken away at the next apply".to_owned(),
                PartState::Ready if part.included => "included in the next apply".to_owned(),
                PartState::Ready => "left out".to_owned(),
                PartState::Shadowed(note) => format!(
                    "set up, but not taking effect. {}",
                    text::note(note, self.home)
                ),
                PartState::Unavailable(why) => text::skip(why, self.home)
                    .trim_start_matches("Left out: ")
                    .to_owned(),
            };
            println!("{}: {state}", text::instance(&part.instance));
        }
        let sched = schedule::status(&self.host);
        if sched.on {
            println!("\n{}.", text::schedule(&sched));
        }
        if let Some(line) = sched.last.as_ref().and_then(text::last_run) {
            println!("{line}.");
        }
        if with_hint {
            println!("\nTo see the commands: velatura --help");
        }
        Ok(OK)
    }

    fn library_exit(&self, why: library::Refused) -> Exit {
        Exit {
            code: match why {
                library::Refused::Io(_) => FAILED,
                library::Refused::Invalid(_)
                | library::Refused::Incomplete(_)
                | library::Refused::NotAThemeFile => FAILED,
                _ => USAGE,
            },
            message: text::library_refused(&why, self.home),
        }
    }

    /// Add a theme file to the library. The file is data: it is checked,
    /// never run, and nothing in it is applied by importing.
    fn theme_import(&self, file: &str) -> Outcome {
        let path = std::path::absolute(file).map_err(|e| Exit {
            code: FAILED,
            message: clean_text(&e.to_string(), 200),
        })?;
        let mut found =
            library::inspect(&self.host, &path).map_err(|why| self.library_exit(why))?;
        if let Some(format) = found.from {
            println!("{}.", text::imported_from(format));
            self.import_accent(&mut found)?;
        } else if self.args.accent.is_some() {
            return Err(Exit {
                code: USAGE,
                message: "--accent is for a base16, base24 or Kitty file. A theme file names its own accent".to_owned(),
            });
        }
        let library::Inspected { loaded, source, .. } = found;
        for w in &loaded.warnings {
            eprintln!("warning: {w}");
        }
        let mut theme = loaded.theme;
        if let Some(id) = &self.args.rename {
            theme.id.clone_from(id);
        }
        if self.args.preview {
            println!(
                "{} ({}), {} variant(s). {}.",
                theme.name,
                theme.id,
                theme.variants.len(),
                text::import_promise()
            );
            return Ok(OK);
        }
        let saved =
            library::save(&self.host, &theme, Some(&source), self.args.replace).map_err(|why| {
                let hint = matches!(
                    why,
                    library::Refused::Exists(_) | library::Refused::Reserved(_)
                );
                let mut exit = self.library_exit(why);
                if hint {
                    exit.message
                        .push_str(". To keep both, import it under another id with --as <id>");
                }
                exit
            })?;
        println!(
            "Imported {} as {}. {}.\nTo apply it: velatura apply {}",
            theme.name,
            text::path(&saved, self.home),
            text::import_promise(),
            theme.id
        );
        Ok(OK)
    }

    /// These formats name no accent, so one of the file's colours is
    /// taken: the one named with `--accent`, the one answered on a
    /// terminal, or the usual one.
    fn import_accent(&self, found: &mut library::Inspected) -> Result<(), Exit> {
        let pick = |word: &str| -> Option<Color> {
            let word = word.trim();
            found
                .accents
                .iter()
                .enumerate()
                .find(|(i, (name, _))| name.eq_ignore_ascii_case(word) || word.parse() == Ok(i + 1))
                .map(|(_, (_, color))| *color)
                .or_else(|| Color::parse(word).ok().filter(|c| c.is_opaque()))
        };
        let names = found
            .accents
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let usual = found.accent;
        let chosen = if let Some(word) = &self.args.accent {
            Some(pick(word).ok_or_else(|| Exit {
                code: USAGE,
                message: format!(
                    "--accent takes one of this file's colors ({names}) or a color as #rrggbb"
                ),
            })?)
        } else if !self.args.yes
            && !self.args.json
            && !self.args.preview
            && std::io::stdin().is_terminal()
        {
            println!("\nThe file names no accent color. One of its colors can be it:");
            for (i, (name, color)) in found.accents.iter().enumerate() {
                let mark = if i == usual { "  (usual)" } else { "" };
                println!("  {:>2}  {name:<20} {}{mark}", i + 1, color.hex());
            }
            loop {
                print!("Accent color: a number, a name or #rrggbb [{}] ", usual + 1);
                let _ = std::io::stdout().flush();
                let mut answer = String::new();
                // Nothing to read, or nothing typed: the usual one.
                if std::io::stdin().read_line(&mut answer).unwrap_or(0) == 0
                    || answer.trim().is_empty()
                {
                    break None;
                }
                match pick(&answer) {
                    Some(color) => break Some(color),
                    None => println!("Not one of them, and not a color."),
                }
            }
        } else {
            None
        };
        let asked = self.args.accent.is_some() || chosen.is_some();
        if let Some(color) = chosen
            && !found.choose_accent(color)
        {
            return Err(Exit {
                code: FAILED,
                message: "The theme could not be given that accent color".to_owned(),
            });
        }
        let accent = found
            .loaded
            .theme
            .variants
            .values()
            .next()
            .map(|v| v.ui.accent.hex())
            .unwrap_or_default();
        match found.accents.get(found.accent) {
            Some((name, _)) if asked => println!("Accent color: {name} {accent}."),
            Some((name, _)) => println!(
                "Accent color: {name} {accent}, the usual one. For another: --accent <name or #rrggbb>, one of {names}."
            ),
            None => println!("Accent color: {accent}."),
        }
        Ok(())
    }

    fn theme_export(&self, id: &str, file: Option<&str>) -> Outcome {
        match file {
            None => {
                let text = library::source(&self.host, id).map_err(|why| self.library_exit(why))?;
                print!("{text}");
            }
            Some(file) => {
                let path = std::path::absolute(file).map_err(|e| Exit {
                    code: FAILED,
                    message: clean_text(&e.to_string(), 200),
                })?;
                library::export(&self.host, id, &path, self.args.replace).map_err(|why| {
                    let there = matches!(why, library::Refused::FileInTheWay(_));
                    let mut exit = self.library_exit(why);
                    if there {
                        exit.message = format!(
                            "{} already exists. To replace it, add --replace",
                            text::path(&path, self.home)
                        );
                    }
                    exit
                })?;
                println!("Written to {}.", text::path(&path, self.home));
            }
        }
        Ok(OK)
    }

    fn theme_resolve(&self, spec: &str) -> Outcome {
        let id = spec.split('/').next().unwrap_or(spec);
        let (entries, _) = library::load(&self.host);
        let Some(entry) = entries.iter().find(|e| e.theme.id == id) else {
            return Err(self.library_exit(library::Refused::NotFound(id.to_owned())));
        };
        print!("{}", velatura_theme::model::to_toml(&entry.theme));
        Ok(OK)
    }

    fn theme_remove(&self, id: &str) -> Outcome {
        // The file's text goes to a place of its own first, so removing
        // can be undone by importing it again.
        let text = library::source(&self.host, id)
            .ok()
            .filter(|_| !velatura_theme::bundled::is_reserved(id));
        let kept = self
            .host
            .data_dir()
            .join("removed")
            .join(format!("{id}.toml"));
        if let (Some(text), Some(dir)) = (&text, kept.parent()) {
            velatura_engine::store::ensure_dir(&self.host, dir)
                .and_then(|()| self.host.fs.write_atomic(&kept, text.as_bytes(), 0o644))
                .map_err(|e| Exit {
                    code: FAILED,
                    message: format!(
                        "Nothing was removed: a copy could not be kept first ({})",
                        clean_text(&e.to_string(), 200)
                    ),
                })?;
        }
        library::remove(&self.host, id).map_err(|why| self.library_exit(why))?;
        println!(
            "Removed {} from the library. A copy is kept.\nTo put it back: velatura theme import {}",
            clean_text(id, 64),
            text::path(&kept, self.home)
        );
        Ok(OK)
    }

    fn template_exit(&self, why: templates::Refused) -> Exit {
        use templates::Refused;
        let (code, message) = match why {
            Refused::BadId(id) => (
                USAGE,
                format!(
                    "{id} cannot be a template's id: use lower-case letters, digits and dashes"
                ),
            ),
            Refused::Invalid(why) => (
                USAGE,
                format!(
                    "The template cannot be used: {}",
                    text::template_problem(&why, self.home)
                ),
            ),
            Refused::Exists(id) => (
                USAGE,
                format!(
                    "A template called {id} is there already. To replace it: --replace. To keep both: --as <id>"
                ),
            ),
            Refused::NotFound(id) => (USAGE, format!("No template is called {id}")),
            Refused::Io(e) => (FAILED, e),
        };
        Exit { code, message }
    }

    fn template_list(&self) -> Outcome {
        let all = templates::load(&self.host);
        let off = engine::excluded(&self.host).unwrap_or_default();
        if self.args.json {
            self.emit(json!({
                "templates": all.iter().map(|(id, entry)| {
                    let instance = format!("{}{id}", templates::PREFIX);
                    match entry {
                        Ok(e) => json!({
                            "id": id, "name": e.name, "output": e.written,
                            "included": !off.contains(&instance),
                        }),
                        Err(why) => json!({
                            "id": id, "problem": text::template_problem(why, self.home),
                        }),
                    }
                }).collect::<Vec<_>>(),
            }));
            return Ok(OK);
        }
        if all.is_empty() {
            println!(
                "No templates. Add one with: velatura template add <file>
They are kept in {}",
                text::path(&templates::dir(&self.host), self.home)
            );
            return Ok(OK);
        }
        for (id, entry) in &all {
            let instance = format!("{}{id}", templates::PREFIX);
            match entry {
                Ok(e) => println!(
                    "{id:<20} {:<24} {}{}",
                    e.name,
                    e.written,
                    if off.contains(&instance) {
                        " (left out)"
                    } else {
                        ""
                    }
                ),
                Err(why) => println!(
                    "{id:<20} cannot be used: {}",
                    text::template_problem(why, self.home)
                ),
            }
        }
        Ok(OK)
    }

    fn template_add(&self, file: &str) -> Outcome {
        let failed = |e: std::io::Error| Exit {
            code: FAILED,
            message: clean_text(&e.to_string(), 200),
        };
        let path = std::path::absolute(file).map_err(failed)?;
        let size = std::fs::metadata(&path).map_err(failed)?.len();
        if size > 512 * 1024 {
            return Err(
                self.template_exit(templates::Refused::Invalid(templates::Invalid::TooLarge))
            );
        }
        let text = std::fs::read_to_string(&path).map_err(failed)?;
        let id = match &self.args.rename {
            Some(id) => id.clone(),
            None => velatura_theme::editor::slug(
                &path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        };
        if self.args.preview {
            let entry = templates::read(&self.host, &id, &text)
                .map_err(|why| self.template_exit(templates::Refused::Invalid(why)))?;
            println!(
                "{} ({id}) would write {}.",
                entry.name,
                text::path(&entry.output, self.home)
            );
            return Ok(OK);
        }
        let entry = templates::add(&self.host, &id, &text, self.args.replace)
            .map_err(|why| self.template_exit(why))?;
        println!(
            "Added the template {} ({id}). It writes {} from the next apply on; the first time, it asks.",
            entry.name,
            text::path(&entry.output, self.home)
        );
        Ok(OK)
    }

    fn template_remove(&self, id: &str) -> Outcome {
        let kept = templates::remove(&self.host, id).map_err(|why| self.template_exit(why))?;
        println!(
            "Removed the template {}. A copy is kept.\nThe file it wrote stays until Undo or Remove Everything takes it away.\nTo put it back: velatura template add {}",
            clean_text(id, 64),
            text::path(&kept, self.home)
        );
        Ok(OK)
    }

    fn look_kind(&self, what: &str) -> Result<look::Kind, Exit> {
        look::Kind::parse(what).ok_or_else(|| {
            usage("choose one of: icons, cursor, wallpaper, font, document-font, monospace-font")
        })
    }

    fn look_show(&self) -> Outcome {
        let current: Vec<(look::Kind, Option<String>)> = look::KINDS
            .into_iter()
            .map(|k| (k, look::current(&self.host, k)))
            .collect();
        if self.args.json {
            let map: serde_json::Map<String, Value> = current
                .iter()
                .map(|(k, v)| (k.id().to_owned(), json!(v)))
                .collect();
            self.emit(json!({"look": map}));
            return Ok(OK);
        }
        for (kind, value) in &current {
            let shown = match value {
                Some(v) => text::look_value(*kind, v),
                None => "not available on this desktop".to_owned(),
            };
            println!("{:<16} {shown}", format!("{}:", text::look_name(*kind)));
        }
        println!(
            "\nTo see what is installed: velatura look list icons\nTo choose: velatura look set icons <name>"
        );
        Ok(OK)
    }

    fn look_list(&self, what: &str) -> Outcome {
        let kind = self.look_kind(what)?;
        let found = look::available(&self.host, &env, kind);
        if self.args.json {
            self.emit(json!({
                "choices": found.iter().map(|c| json!({"value": c.value, "name": c.name})).collect::<Vec<_>>(),
            }));
            return Ok(OK);
        }
        if found.is_empty() {
            println!("Nothing to list. A font is chosen by its name and size.");
        }
        let now = look::current(&self.host, kind);
        for c in &found {
            let mark = if now.as_deref() == Some(c.value.as_str()) {
                "  (in use)"
            } else {
                ""
            };
            if c.name == c.value || kind == look::Kind::Wallpaper {
                println!("{}{mark}", c.name);
            } else {
                println!("{:<28} {}{mark}", c.value, c.name);
            }
        }
        Ok(OK)
    }

    fn look_set(&self, what: &str, value: Option<&str>) -> Outcome {
        let kind = self.look_kind(what)?;
        let plan = look::plan_select(&self.host, &env, kind, value).map_err(|why| {
            match text::rejected_choice(kind, &why) {
                Some(message) => Exit {
                    code: USAGE,
                    message: format!("{message}. To see the choices: velatura look list {what}"),
                },
                None => match why {
                    look::Rejected::Engine(e) => Exit::from(e),
                    _ => usage("that cannot be chosen"),
                },
            }
        })?;
        self.run(&plan, true)
    }

    fn schedule_json(&self, s: &schedule::Status) -> Value {
        json!({
            "on": s.on,
            "follows_style": s.follows_style,
            "damaged": s.damaged,
            "day_at": schedule::format_time(s.times.day),
            "night_at": schedule::format_time(s.times.night),
            "last": s.last.as_ref().map(|l| json!({
                "time": l.at, "period": l.period.as_str(), "theme": l.theme,
                "left_out": l.left_out, "failed": l.problem.is_some(),
            })),
        })
    }

    fn refuse(&self, why: Refused) -> Exit {
        let message = text::refused(&why, self.home);
        Exit {
            message: match why {
                Refused::OtherTimers(_) => {
                    format!("{message}. To turn it on all the same, add --yes")
                }
                _ => message,
            },
            ..Exit::from(why)
        }
    }

    fn schedule_show(&self) -> Outcome {
        let s = schedule::status(&self.host);
        if self.args.json {
            self.emit(json!({"schedule": self.schedule_json(&s)}));
            return Ok(OK);
        }
        println!("{}.", text::schedule(&s));
        let chosen = DayNight::load(&self.host);
        let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "not chosen".to_owned());
        println!(
            "Daytime theme:   {} from {}",
            show(&chosen.day),
            schedule::format_time(s.times.day)
        );
        println!(
            "Nighttime theme: {} from {}",
            show(&chosen.night),
            schedule::format_time(s.times.night)
        );
        if let Some(line) = s.last.as_ref().and_then(text::last_run) {
            println!("{line}.");
        }
        if !s.on {
            println!("\nTo turn it on: velatura schedule set --day 07:00 --night 20:00");
        }
        Ok(OK)
    }

    /// Turn the schedule on, or change its times. A time not given keeps
    /// its present value.
    fn schedule_set(&self) -> Outcome {
        let mut times = Times::load(&self.host);
        for (given, slot) in [
            (&self.args.day, &mut times.day),
            (&self.args.night, &mut times.night),
        ] {
            if let Some(text) = given {
                *slot = schedule::parse_time(text).ok_or_else(|| {
                    usage(&format!(
                        "\"{}\" is not a time. Write it as HH:MM, for example 07:00",
                        clean_text(text, 20)
                    ))
                })?;
            }
        }
        let Some(exe) = self_command(None) else {
            return Err(Exit {
                code: FAILED,
                message: "Velatura cannot tell where its own command is.".to_owned(),
            });
        };
        if self.args.preview {
            println!(
                "Would write {} and {} in {}, and turn the timer on with systemctl --user.",
                schedule::SERVICE,
                schedule::TIMER,
                text::path(&self.host.dirs.config.join("systemd/user"), self.home)
            );
            return Ok(OK);
        }
        schedule::enable(&self.host, &StdSystemd, &exe, times, self.args.yes)
            .map_err(|why| self.refuse(why))?;
        println!("{}.", text::schedule(&schedule::status(&self.host)));
        println!(
            "Apps that are already open keep their colors until reopened.\nTo switch now: velatura switch auto\nTo turn it off: velatura schedule off"
        );
        Ok(OK)
    }

    /// Follow the desktop's light and dark style in place of the clock.
    fn schedule_follow(&self) -> Outcome {
        let Some(exe) = self_command(None) else {
            return Err(Exit {
                code: FAILED,
                message: "Velatura cannot tell where its own command is.".to_owned(),
            });
        };
        schedule::enable_follow(&self.host, &StdSystemd, &exe, self.args.yes)
            .map_err(|why| self.refuse(why))?;
        println!("{}.", text::schedule(&schedule::status(&self.host)));
        println!(
            "From now on, applying a theme leaves the light or dark style as it is.\nTo turn it off: velatura schedule off"
        );
        Ok(OK)
    }

    fn top_bar_show(&self) -> Outcome {
        let bar = engine::top_bar(&self.host)?;
        let on = !engine::excluded(&self.host)?
            .iter()
            .any(|i| i == targets::TOP_BAR);
        if self.args.json {
            self.emit(json!({"top_bar": {
                "on": on, "style": bar.style.id(), "opacity": bar.opacity,
            }}));
            return Ok(OK);
        }
        let style = match bar.style {
            BarStyle::Bar => "a full bar",
            BarStyle::Pills => "pills",
        };
        println!("The top bar is drawn as {style}, {}% opaque.", bar.opacity);
        if !on {
            println!(
                "The part is off: it is turned on in the app, or named for one apply with --only {}",
                targets::TOP_BAR
            );
        }
        println!("To choose: velatura top-bar style bar|pills, velatura top-bar opacity <0-100>");
        Ok(OK)
    }

    fn top_bar_set(&self, style: Option<&str>, opacity: Option<&str>) -> Outcome {
        let mut bar = engine::top_bar(&self.host)?;
        if let Some(style) = style {
            bar.style = BarStyle::from_id(style)
                .ok_or_else(|| usage("the top bar's style is bar or pills"))?;
        }
        if let Some(opacity) = opacity {
            bar.opacity = opacity
                .trim_end_matches('%')
                .parse::<u8>()
                .ok()
                .filter(|o| *o <= 100)
                .ok_or_else(|| usage("the top bar's opacity is a percentage, from 0 to 100"))?;
        }
        engine::set_top_bar(&self.host, bar)?;
        println!("Chosen. It takes effect with the next apply while the part is on.");
        Ok(OK)
    }

    fn extension_show(&self) -> Outcome {
        let s = velatura_engine::companion::status(&self.host, &env);
        if self.args.json {
            self.emit(json!({"extension": {
                "uuid": velatura_engine::companion::UUID,
                "installed": s.installed.as_ref().map(|p| p.to_string_lossy()),
                "by_velatura": s.by_velatura, "differs": s.differs, "outdated": s.outdated, "enabled": s.enabled,
            }}));
            return Ok(OK);
        }
        println!("{}.", text::extension(&s));
        if let Some(dir) = &s.installed {
            println!("It is in {}.", text::path(dir, self.home));
        } else {
            println!("To install it: velatura extension install");
        }
        Ok(OK)
    }

    fn extension_install(&self) -> Outcome {
        let changed =
            velatura_engine::companion::install(&self.host).map_err(|why| self.refuse(why))?;
        let s = velatura_engine::companion::status(&self.host, &env);
        if changed {
            println!(
                "Installed in {}.",
                text::path(&s.installed.clone().unwrap_or_default(), self.home)
            );
            println!("GNOME Shell sees a new or changed extension after signing out and in.");
        }
        println!("{}.", text::extension(&s));
        Ok(OK)
    }

    fn extension_remove(&self) -> Outcome {
        match velatura_engine::companion::remove(&self.host).map_err(|why| self.refuse(why))? {
            None => println!("Velatura has not installed its extension here."),
            Some(kept) => {
                println!("Velatura's extension was removed.");
                for p in &kept {
                    println!(
                        "Note: {}",
                        text::note(&Note::KeptFile { path: p.clone() }, self.home)
                    );
                }
            }
        }
        Ok(OK)
    }

    fn schedule_off(&self) -> Outcome {
        match schedule::disable(&self.host, &StdSystemd).map_err(|why| self.refuse(why))? {
            None => println!("Scheduled switching was not on."),
            Some(kept) => {
                println!("Scheduled switching is off. Its timer was removed.");
                for p in &kept {
                    println!(
                        "Note: {}",
                        text::note(&Note::KeptFile { path: p.clone() }, self.home)
                    );
                }
            }
        }
        Ok(OK)
    }

    /// What the timer runs: the theme for the present time of day, with no
    /// one there to answer a question.
    fn switch_auto(&self) -> Outcome {
        let minute = local_minute(self.host.now);
        let done = schedule::run_auto(&self.host, &env, minute, self.args.again)?;
        let word = |p: Period| match p {
            Period::Day => "daytime",
            Period::Night => "nighttime",
        };
        match done {
            Auto::NotDue(period) => {
                println!(
                    "Nothing to do: the {} theme was already seen to. To apply it again: velatura switch auto --again",
                    word(period)
                );
                Ok(OK)
            }
            Auto::NoTheme(period) => Err(Exit {
                code: FAILED,
                message: format!(
                    "No {0} theme is chosen, or it no longer exists. To choose one: velatura switch set {1} <theme>",
                    word(period),
                    period.as_str()
                ),
            }),
            Auto::Done { theme, report, .. } => {
                if report.changes.is_empty() {
                    println!("The {theme} theme is already applied.");
                } else {
                    println!("Applied {theme}.");
                    for line in text::changes(&report.changes, self.home) {
                        println!("  {line}");
                    }
                }
                let last = schedule::status(&self.host).last;
                match last.as_ref().and_then(text::last_run) {
                    Some(line) => {
                        println!("{line}.");
                        Ok(PARTIAL)
                    }
                    None => Ok(OK),
                }
            }
        }
    }

    fn plan_json(&self, plan: &Plan, report: Option<&Report>) -> Value {
        json!({
            "cause": plan.cause.as_str(),
            "theme": plan.theme.as_ref().map(|(label, _)| label),
            "applied": report.is_some(),
            "backup": report.and_then(|r| r.backup),
            "targets": plan.targets.iter().map(|t| {
                let skipped = report
                    .and_then(|r| r.skipped.iter().find(|(i, _)| *i == t.instance).map(|(_, s)| s))
                    .or(t.skipped.as_ref());
                json!({
                    "instance": t.instance,
                    "skipped": skipped.map(skip_code),
                    "needs_agreement": t.needs_consent,
                    "changes": t.changes.iter().map(change_json).collect::<Vec<_>>(),
                    "notes": t.notes.iter().map(note_code).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }

    fn print_plan(&self, plan: &Plan) {
        for t in &plan.targets {
            if matches!(t.skipped, Some(Skip::NotSelected)) {
                continue;
            }
            if t.changes.is_empty() && t.notes.is_empty() && t.skipped.is_none() {
                continue;
            }
            println!("{}", text::instance(&t.instance));
            if let Some(why) = &t.skipped {
                println!("  {}", text::skip(why, self.home));
                continue;
            }
            for line in text::changes(&t.changes, self.home) {
                println!("  {line}");
            }
            for n in &t.notes {
                println!("  Note: {}", text::note(n, self.home));
            }
        }
        if !plan.is_empty() {
            let removes = plan.changes().any(|c| {
                matches!(
                    c,
                    Change::Owned { content: None, .. } | Change::Block { content: None, .. }
                )
            });
            if !removes {
                println!("\nNothing is deleted. A backup is made first.");
            }
        }
    }

    /// Before the very first apply, say what else on this machine changes
    /// the same colours.
    fn print_other_tools(&self, plan: &Plan) {
        let first = plan.cause == Cause::Apply
            && engine::status(&self.host).is_ok_and(|s| s.applied.is_none());
        if !first {
            return;
        }
        for tool in targets::other_tools(&self.host) {
            println!("Note: {}", text::other_tool(&tool, self.home));
        }
    }

    /// Show the plan, ask if needed, carry it out, and report.
    fn run(&self, plan: &Plan, undo_hint: bool) -> Outcome {
        let args = self.args;
        if !args.json {
            self.print_other_tools(plan);
        }
        if args.preview {
            if args.json {
                self.emit(self.plan_json(plan, None));
            } else {
                self.print_plan(plan);
                if plan.is_empty() {
                    println!("Nothing would change.");
                }
            }
            return Ok(OK);
        }

        let mut agreed: Vec<String> = Vec::new();
        let asking = plan.needing_consent();
        if !asking.is_empty() {
            if args.yes {
                agreed = asking.iter().map(|s| (*s).to_owned()).collect();
            } else if !args.json && std::io::stdin().is_terminal() {
                self.print_plan(plan);
                for instance in asking {
                    if instance == targets::NVIM {
                        print!("\nAdd Velatura's two files to the Neovim settings folder? [y/N] ");
                    } else if let Some(id) = instance.strip_prefix(templates::PREFIX) {
                        print!(
                            "\nWrite the output of the template {} for the first time? [y/N] ",
                            clean_text(id, 64)
                        );
                    } else if instance == targets::FIREFOX {
                        print!(
                            "\nAdd Velatura's stylesheet to the Firefox profile, with a line in its userChrome.css and one in its user.js? [y/N] "
                        );
                    } else if matches!(instance, targets::PTYXIS | targets::TEXT_EDITOR) {
                        print!(
                            "\nGive {} the theme's colors, as a file of Velatura's and one setting? [y/N] ",
                            text::instance(instance)
                        );
                    } else {
                        print!(
                            "\nAdd the line to the {} settings file? [y/N] ",
                            text::instance(instance)
                        );
                    }
                    let _ = std::io::stdout().flush();
                    let mut answer = String::new();
                    let _ = std::io::stdin().read_line(&mut answer);
                    if matches!(answer.trim(), "y" | "Y" | "yes") {
                        agreed.push(instance.to_owned());
                    }
                }
                println!();
            } else {
                // No terminal and no --yes: change nothing at all.
                if args.json {
                    self.emit(self.plan_json(plan, None));
                } else {
                    self.print_plan(plan);
                }
                return Err(Exit {
                    code: NEEDS_AGREEMENT,
                    message: "Nothing was changed. This adds to the settings of an app for the first time; run again with --yes to agree.".to_owned(),
                });
            }
        }

        let report = engine::execute(&self.host, plan, &agreed)?;
        if args.json {
            self.emit(self.plan_json(plan, Some(&report)));
        } else {
            if let Some(r) = &report.recovered {
                println!("An earlier interrupted change was rolled back first.");
                print!("{}", text::rollback(r));
            }
            for (instance, why) in &report.skipped {
                if !matches!(
                    why,
                    Skip::NotSelected
                        | Skip::NotInstalled
                        | Skip::NeedsAdwGtk3
                        | Skip::NeedsUserThemes
                ) {
                    println!(
                        "{}: {}",
                        text::instance(instance),
                        text::skip(why, self.home)
                    );
                }
            }
            if report.changes.is_empty() && plan.cause != Cause::Undo {
                println!("Nothing to change.");
            } else {
                match (plan.cause, &plan.theme) {
                    (Cause::Apply | Cause::Scheduled, Some((label, _))) => {
                        println!("Applied {label}.");
                    }
                    (Cause::Undo, _) => println!("Undone."),
                    (Cause::Select, _) => println!("Changed."),
                    _ => println!("Everything Velatura added was removed."),
                }
                for line in text::changes(&report.changes, self.home) {
                    println!("  {line}");
                }
            }
            for (_, n) in &report.notes {
                println!("Note: {}", text::note(n, self.home));
            }
            if undo_hint && report.backup.is_some() {
                println!("To undo: velatura undo");
            }
        }
        let left_out = report.skipped.iter().any(|(_, why)| {
            !matches!(
                why,
                Skip::NotSelected | Skip::NotInstalled | Skip::NeedsAdwGtk3 | Skip::NeedsUserThemes
            )
        }) || report.notes.iter().any(|(_, n)| {
            matches!(
                n,
                Note::KeptFile { .. } | Note::KeptSetting { .. } | Note::SettingNeedsSession { .. }
            )
        });
        Ok(if left_out { PARTIAL } else { OK })
    }

    /// Every name given to --only or --except names a part: a typo would
    /// otherwise leave the apply doing nothing, or everything.
    fn check_parts(&self) -> Result<(), Exit> {
        let all = engine::instances(&self.host);
        let known = |name: &str| {
            all.iter()
                .any(|i| i == name || i.split(':').next() == Some(name))
        };
        let unknown: Vec<&String> = self
            .args
            .only
            .iter()
            .chain(&self.args.except)
            .filter(|name| !known(name))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        // As `velatura targets` names them.
        let short: Vec<&str> = all
            .iter()
            .map(|i| i.strip_suffix(":default").unwrap_or(i))
            .collect();
        Err(usage(&format!(
            "No part named {}. Parts: {}",
            unknown
                .iter()
                .map(|n| format!("\"{}\"", clean_text(n, 40)))
                .collect::<Vec<_>>()
                .join(", "),
            short.join(", ")
        )))
    }

    fn apply(&self, spec: &str) -> Outcome {
        self.check_parts()?;
        let opts = ApplyOpts {
            only: self.args.only.clone(),
            except: self.args.except.clone(),
            replace_changed: self.args.replace_changed,
            unattended: false,
            keep_style: false,
            probe: false,
            // Naming the part outright is asking for it.
            untested_shell: self
                .args
                .only
                .iter()
                .any(|name| name == "gnome" || name == targets::SHELL),
        };
        let plan =
            self.with_theme(spec, |r, _| engine::plan_apply(&self.host, &env, r, &opts))??;
        self.run(&plan, true)
    }

    fn switch_show(&self) -> Outcome {
        let chosen = DayNight::load(&self.host);
        let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "not chosen".to_owned());
        println!("Daytime theme:   {}", show(&chosen.day));
        println!("Nighttime theme: {}", show(&chosen.night));
        println!(
            "\nTo choose: velatura switch set day <theme>\nTo switch: velatura switch day, night or toggle"
        );
        Ok(OK)
    }

    fn switch_set(&self, which: &str, spec: &str) -> Outcome {
        // Only a theme that exists can be chosen.
        self.with_theme(spec, |_, _| ())?;
        let mut chosen = DayNight::load(&self.host);
        if which == "day" {
            chosen.day = Some(spec.to_owned());
        } else {
            chosen.night = Some(spec.to_owned());
        }
        chosen.save(&self.host).map_err(|e| Exit {
            code: FAILED,
            message: clean_text(&e.to_string(), 300),
        })?;
        println!("The {which}time theme is now {}.", clean_text(spec, 80));
        Ok(OK)
    }

    /// Apply the daytime or the nighttime theme, or whichever of the two is
    /// not in use.
    fn switch(&self, which: &str) -> Outcome {
        let chosen = DayNight::load(&self.host);
        let which = if which == "toggle" {
            let applied = engine::status(&self.host)?.applied.map(|(label, _)| label);
            let is_day = match (&applied, &chosen.day) {
                (Some(label), Some(day)) => same_theme(label, day),
                _ => false,
            };
            if is_day { "night" } else { "day" }
        } else {
            which
        };
        let spec = if which == "day" {
            chosen.day
        } else {
            chosen.night
        };
        let Some(spec) = spec else {
            return Err(Exit {
                code: USAGE,
                message: format!(
                    "No {which}time theme is chosen yet. To choose one: velatura switch set {which} <theme>"
                ),
            });
        };
        self.apply(&spec)
    }

    /// Undo once, or with `until` keep undoing down to and including that
    /// backup, which leaves the state from before it.
    fn undo(&self, until: Option<u64>) -> Outcome {
        let mut worst = OK;
        let mut any = false;
        loop {
            let plan = match engine::plan_undo(&self.host) {
                Err(Error::NothingToUndo) if any => return Ok(worst),
                other => other?,
            };
            let id = plan.undo_of.unwrap_or(0);
            if until.is_some_and(|u| id < u) {
                if any {
                    return Ok(worst);
                }
                return Err(Exit {
                    code: FAILED,
                    message: "No such backup, or it was already undone. See: velatura history"
                        .to_owned(),
                });
            }
            worst = worst.max(self.run(&plan, false)?);
            any = true;
            // A switch the schedule made is not what the user means by the
            // last change: the one before it goes too.
            if self.args.preview || (until.is_none() && !plan.scheduled) {
                return Ok(worst);
            }
        }
    }

    fn restore_original(&self) -> Outcome {
        let plan = engine::plan_restore_original(&self.host)?;
        // The timer first, or it would bring a theme back.
        if !self.args.preview {
            match schedule::disable(&self.host, &StdSystemd) {
                Ok(Some(_)) if !self.args.json => {
                    println!("Scheduled switching is off. Its timer was removed.");
                }
                Ok(_) => {}
                Err(why) => return Err(self.refuse(why)),
            }
        } else if schedule::status(&self.host).on && !self.args.json {
            println!("Scheduled switching would be turned off and its timer removed.");
        }
        self.run(&plan, true)
    }

    fn history(&self) -> Outcome {
        let (all, damaged) = engine::history(&self.host)?;
        if self.args.json {
            self.emit(json!({
                "backups": all.iter().map(|m| json!({
                    "id": m.id, "time": m.time, "cause": m.cause, "theme": m.theme,
                    "undone": m.undone, "changes": m.records.len(),
                })).collect::<Vec<_>>(),
                "damaged": damaged.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            }));
            return Ok(OK);
        }
        if all.is_empty() {
            println!("No backups yet.");
        }
        for m in &all {
            println!(
                "{:>4}  {}  {:<17} {:<24} {}",
                m.id,
                text::time(m.time),
                m.cause,
                m.theme.as_deref().unwrap_or("-"),
                if m.undone { "undone" } else { "" }
            );
        }
        for p in &damaged {
            eprintln!("Damaged backup, ignored: {}", text::path(p, self.home));
        }
        Ok(OK)
    }

    fn recover(&self) -> Outcome {
        match engine::recover(&self.host)? {
            None => println!("Nothing to recover."),
            Some(r) => {
                if r.is_clean() {
                    println!("The interrupted change was rolled back.");
                } else {
                    println!("Rolling back did not finish:");
                }
                print!("{}", text::rollback(&r));
                if !r.is_clean() {
                    return Ok(FAILED);
                }
            }
        }
        Ok(OK)
    }

    fn doctor(&self) -> Outcome {
        println!("velatura {}", env!("CARGO_PKG_VERSION"));
        let mut code = OK;
        let read = |key| self.host.settings.read(targets::INTERFACE_SCHEMA, key);
        match read("color-scheme") {
            Ok(Some(s)) => println!("Desktop settings: available (style is {})", s.effective),
            Ok(None) => println!("Desktop settings: this desktop has no style setting"),
            Err(e) => {
                println!(
                    "Desktop settings: not available ({})",
                    clean_text(&e.to_string(), 120)
                );
                code = PARTIAL;
            }
        }
        match read("accent-color") {
            Ok(Some(s)) => println!("Accent color: available (now {})", s.effective),
            Ok(None) => println!("Accent color: not offered by this GNOME version"),
            Err(_) => {}
        }
        let s = engine::status(&self.host)?;
        if s.unfinished {
            println!("An earlier change was interrupted. To roll it back: velatura recover");
            code = PARTIAL;
        }
        let (_, damaged) = engine::history(&self.host)?;
        for p in &damaged {
            println!("Damaged backup: {}", text::path(p, self.home));
            code = PARTIAL;
        }
        for tool in targets::other_tools(&self.host) {
            println!("{}", text::other_tool(&tool, self.home));
        }
        let sched = schedule::status(&self.host);
        println!("{}", text::schedule(&sched));
        if sched.damaged {
            code = PARTIAL;
        }
        if let Some(line) = sched.last.as_ref().and_then(text::last_run) {
            println!("{line}");
        }
        // What applying would run into, using a bundled theme as the probe.
        let themes = velatura_theme::bundled::themes();
        if let Some(r) = themes.first().and_then(|t| Resolved::new(t, None)) {
            let plan = engine::plan_apply(&self.host, &env, &r, &ApplyOpts::default())?;
            for t in &plan.targets {
                let line = match &t.skipped {
                    Some(why) => text::skip(why, self.home),
                    None => "can be themed".to_owned(),
                };
                println!("{}: {line}", text::instance(&t.instance));
                for n in &t.notes {
                    if matches!(
                        n,
                        Note::Override { .. }
                            | Note::LinkedFile { .. }
                            | Note::UserThemesOff
                            | Note::ShellExtensionStyles { .. }
                            | Note::KittyAutoTheme { .. }
                            | Note::SettingLocked { .. }
                    ) {
                        println!("  Note: {}", text::note(n, self.home));
                    }
                }
            }
        }
        Ok(code)
    }
}
