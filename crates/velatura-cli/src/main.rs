//! The `velatura` command.
//!
//! Arguments in, typed engine calls, text or JSON out. No logic that the
//! GUI would have to repeat lives here.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use velatura_engine::engine::{self, ApplyOpts, Error, Health, Report};
use velatura_engine::host::{Dirs, Fs, Host, Settings, StdFs};
use velatura_engine::library;
use velatura_engine::plan::{Cause, Change, Note, Plan, Skip};
use velatura_engine::store::DayNight;
use velatura_engine::targets;
use velatura_text as text;
use velatura_theme::model::{ANSI_NAMES, clean_text};
use velatura_theme::{Resolved, nearest_accent, parse_theme};

const OK: u8 = 0;
const FAILED: u8 = 1;
const USAGE: u8 = 2;
const NEEDS_AGREEMENT: u8 = 3;
const BUSY: u8 = 4;
const PARTIAL: u8 = 5;

/// Version of the JSON output. Keys are only ever added within a version.
const JSON_FORMAT: u32 = 1;

#[derive(Default)]
struct Args {
    words: Vec<String>,
    preview: bool,
    yes: bool,
    replace_changed: bool,
    strict: bool,
    json: bool,
    original: bool,
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
        ["switch"] => cx.switch_show(),
        ["switch", "set", which @ ("day" | "night"), theme] => cx.switch_set(which, theme),
        ["switch", which @ ("day" | "night" | "toggle")] => cx.switch(which),
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
        let (code, message) = match e {
            Error::Busy => (
                BUSY,
                "Another Velatura is making changes. Try again in a moment.".to_owned(),
            ),
            Error::NothingToUndo => (FAILED, "There is nothing to undo.".to_owned()),
            Error::Io(why) => (FAILED, clean_text(&why, 300)),
            Error::Store(e) => (
                FAILED,
                format!(
                    "Velatura's own records cannot be used ({}). Nothing was changed.",
                    clean_text(&format!("{e:?}"), 300)
                ),
            ),
            Error::Recovery(r) => (
                FAILED,
                format!(
                    "An earlier interrupted change could not be fully rolled back, so nothing new was changed.\n{}To try again: velatura recover",
                    text::rollback(&r)
                ),
            ),
            Error::Failed(f) => (FAILED, text::failure(&f)),
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
    }
}

fn note_code(n: &Note) -> &'static str {
    match n {
        Note::Override { .. } => "override",
        Note::GhosttyReload => "ghostty-reload",
        Note::GtkRestart => "gtk-restart",
        Note::LinkedFile { .. } => "linked-file",
        Note::Gtk3DarkForced { .. } => "gtk3-dark-forced",
        Note::SettingNeedsSession { .. } => "setting-needs-session",
        Note::UserThemesOff => "user-themes-off",
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
        let resolved = Resolved::new(&entry.theme, variant).ok_or_else(not_found)?;
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
        for (id, name, support, place) in rows {
            println!("{id:<17} {name:<28} {support:<11} {place}");
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
        if with_hint {
            println!("\nTo see the commands: velatura --help");
        }
        Ok(OK)
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
            for c in &t.changes {
                println!("  {}", text::change(c, self.home));
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

    /// Show the plan, ask if needed, carry it out, and report.
    fn run(&self, plan: &Plan, undo_hint: bool) -> Outcome {
        let args = self.args;
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
                    print!(
                        "\nAdd the line to the {} settings file? [y/N] ",
                        text::instance(instance)
                    );
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
                    message: "Nothing was changed. This adds a line to a settings file for the first time; run again with --yes to agree.".to_owned(),
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
                    (Cause::Apply, Some((label, _))) => println!("Applied {label}."),
                    (Cause::Undo, _) => println!("Undone."),
                    _ => println!("Everything Velatura added was removed."),
                }
                for c in &report.changes {
                    println!("  {}", text::change(c, self.home));
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

    fn apply(&self, spec: &str) -> Outcome {
        let opts = ApplyOpts {
            only: self.args.only.clone(),
            except: self.args.except.clone(),
            replace_changed: self.args.replace_changed,
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
            if until.is_none() || self.args.preview {
                return Ok(worst);
            }
        }
    }

    fn restore_original(&self) -> Outcome {
        let plan = engine::plan_restore_original(&self.host)?;
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
                    clean_text(&format!("{e:?}"), 120)
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
            match tool {
                targets::OtherTool::Known { name, path } => println!(
                    "Another theming tool: {name} ({}). It may write the same colors",
                    text::path(&path, self.home)
                ),
                targets::OtherTool::Timer { unit } => println!(
                    "A timer that may change the theme is enabled: {}",
                    clean_text(&unit, 120)
                ),
                targets::OtherTool::Dotfiles { name, path } => println!(
                    "Settings files may be managed by {name} ({}). It can overwrite changes made here",
                    text::path(&path, self.home)
                ),
            }
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
