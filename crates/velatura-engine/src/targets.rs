//! Connectors: where a theme's rendered output goes.
//!
//! A connector only reads. It looks at the machine and returns the changes
//! it would make; the engine carries them out.
//!
//! Most targets that write files have the same shape: a file Velatura creates
//! and owns, connected by one line inside a marked block at the end of a
//! file the user maintains. [`FileTarget`] describes one and
//! [`plan_files`] plans it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use velatura_theme::{
    Appearance, Resolved, ghostty, gtk, kitty, nearest_accent, nvim, ptyxis, sha256_hex, shell,
    sourceview, starship, tmux,
};

use crate::block;
use crate::host::{Host, Kind, SettingsError};
use crate::plan::{Change, Note, Skip, TargetPlan};
use crate::store::{State, path_str, read_opt};

/// Environment lookup, supplied by the caller.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub const KITTY: &str = "kitty:default";
pub const GHOSTTY: &str = "ghostty:default";
pub const GTK4: &str = "gtk4:default";
pub const GTK3: &str = "gtk3:default";
pub const SHELL: &str = "gnome:shell";
pub const APPEARANCE: &str = "gnome:appearance";
pub const OUTLINE: &str = "gnome:outline";
pub const TOP_BAR: &str = "gnome:top-bar";
pub const UPS: &str = "gnome:ups";
pub const TASKS: &str = "gnome:tasks";
pub const MEDIA: &str = "gnome:media";
pub const FLATPAK: &str = "flatpak:global";
pub const TMUX: &str = "tmux:default";
pub const NVIM: &str = "nvim:default";
pub const STARSHIP: &str = "starship:default";
pub const PTYXIS: &str = "ptyxis:default";
pub const TEXT_EDITOR: &str = "texteditor:default";
pub use crate::cursor_set::CURSORS;
pub use crate::firefox::FIREFOX;
pub use crate::icon_set::ICONS;

/// Every instance this build knows, in the order they are applied.
pub const INSTANCES: [&str; 20] = [
    KITTY,
    GHOSTTY,
    GTK4,
    GTK3,
    TMUX,
    NVIM,
    STARSHIP,
    PTYXIS,
    TEXT_EDITOR,
    FIREFOX,
    FLATPAK,
    SHELL,
    TOP_BAR,
    UPS,
    TASKS,
    MEDIA,
    OUTLINE,
    APPEARANCE,
    ICONS,
    CURSORS,
];

/// Instances that are left out until the user turns them on, because they
/// change more than colours, or use a way in that is not supported.
pub const OPT_IN: [&str; 9] = [
    FLATPAK, ICONS, FIREFOX, CURSORS, TOP_BAR, UPS, TASKS, MEDIA, OUTLINE,
];

/// Parts that are one stylesheet only Velatura's extension shows. Like
/// every part turned off, they are taken away at the next apply by hand
/// (see [`crate::off`]); theirs is found by its place even while the
/// extension is off.
pub const SHEETS: [&str; 5] = [TOP_BAR, UPS, TASKS, MEDIA, OUTLINE];

/// The name of Velatura's span in Flatpak's overrides file. Unlike the
/// others it is not a marked block: see [`crate::keyfile`].
pub const FLATPAK_SPAN: &str = "flatpak";
pub const FLATPAK_GROUP: &str = "Context";
pub const FLATPAK_KEY: &str = "filesystems";
/// Read-only access to the two folders the GTK targets write to.
const FLATPAK_ACCESS: [&str; 2] = ["xdg-config/gtk-3.0:ro", "xdg-config/gtk-4.0:ro"];

pub const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const A11Y_SCHEMA: &str = "org.gnome.desktop.a11y.interface";

/// True if `path`, after following links, is inside the home folder.
pub(crate) fn inside_home(host: &Host<'_>, path: &Path) -> bool {
    match (
        host.fs.canonicalize(path),
        host.fs.canonicalize(&host.dirs.home),
    ) {
        (Ok(real), Ok(home)) => real.starts_with(home),
        _ => false,
    }
}

pub(crate) fn exists(host: &Host<'_>, path: &Path) -> bool {
    host.fs.kind(path).ok().flatten().is_some()
}

pub(crate) fn on_path(host: &Host<'_>, env: Env<'_>, program: &str) -> bool {
    env("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .any(|dir| exists(host, &dir.join(program)))
    })
}

pub fn kitty_dir(host: &Host<'_>, env: Env<'_>) -> PathBuf {
    env("KITTY_CONFIG_DIRECTORY")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| host.dirs.config.join("kitty"))
}

pub fn ghostty_dir(host: &Host<'_>) -> PathBuf {
    host.dirs.config.join("ghostty")
}

/// Ghostty reads `config.ghostty` if it exists and `config` otherwise.
pub fn ghostty_conf(host: &Host<'_>) -> &'static str {
    if exists(host, &ghostty_dir(host).join("config.ghostty")) {
        "config.ghostty"
    } else {
        "config"
    }
}

/// One owned file plus one marked block in a file of the user's.
struct FileTarget<'a> {
    instance: &'a str,
    /// The name in the block's marker lines.
    block: &'a str,
    /// The folder the owned file goes into.
    dir: PathBuf,
    /// If set, the target counts as installed when its folder exists or
    /// this program is on the path. If not set it is always available.
    program: Option<&'a str>,
    owned: &'a str,
    content: Vec<u8>,
    /// The user's file the block goes into.
    conf: PathBuf,
    /// The single line placed inside the block.
    body: &'a str,
    /// True for a line that, placed after the block, wins over the theme.
    overrides: &'a dyn Fn(&str) -> bool,
}

/// 1-based numbers of the lines after the block for which `wins` is true.
pub(crate) fn override_lines(
    content: &[u8],
    block: &str,
    wins: &dyn Fn(&str) -> bool,
) -> Vec<usize> {
    let block::Found::Present { end, .. } = block::find(content, block) else {
        return Vec::new();
    };
    let first = content[..end].iter().filter(|b| **b == b'\n').count() + 1;
    String::from_utf8_lossy(&content[end..])
        .lines()
        .enumerate()
        .filter(|(_, line)| wins(line))
        .map(|(i, _)| first + i)
        .collect()
}

/// The folders Velatura's files go into, outermost first: each is there
/// already, inside the home folder, or is planned. Only folders under one
/// that exists are made; a missing parent of the first means this is not
/// a place the app has ever used.
pub(crate) fn place(host: &Host<'_>, levels: &[&Path], plan: &mut TargetPlan) -> Result<(), Skip> {
    for dir in levels {
        let dir = dir.to_path_buf();
        let under_planned = plan.changes.iter().any(
            |c| matches!(c, Change::CreateDir { path } if Some(path.as_path()) == dir.parent()),
        );
        match host.fs.kind(&dir).ok().flatten() {
            None if under_planned => plan.changes.push(Change::CreateDir { path: dir }),
            None => match dir.parent() {
                Some(parent) if inside_home(host, parent) => {
                    plan.changes.push(Change::CreateDir { path: dir });
                }
                _ => return Err(Skip::OutsideHome(dir)),
            },
            Some(Kind::Dir | Kind::Symlink) if inside_home(host, &dir) => {}
            Some(Kind::Dir | Kind::Symlink) => return Err(Skip::OutsideHome(dir)),
            Some(_) => return Err(Skip::NotAFile(dir)),
        }
    }
    Ok(())
}

/// A file Velatura creates and owns, with `content` in it.
pub(crate) fn own(
    host: &Host<'_>,
    state: &State,
    replace_changed: bool,
    owned: PathBuf,
    content: &[u8],
    plan: &mut TargetPlan,
) -> Result<(), Skip> {
    let current = match host.fs.kind(&owned) {
        Ok(None) => None,
        Ok(Some(Kind::File)) => match host.fs.read(&owned) {
            Ok(bytes) => Some(bytes),
            Err(_) => return Err(Skip::Unreadable(owned)),
        },
        _ => return Err(Skip::ForeignFile(owned)),
    };
    let pre = current.as_deref().map(sha256_hex);
    match (&pre, state.owned.get(&path_str(&owned))) {
        (None, _) => {}
        (Some(_), None) => return Err(Skip::ForeignFile(owned)),
        (Some(now), Some(written)) if now != written && !replace_changed => {
            return Err(Skip::ChangedOutside(owned));
        }
        _ => {}
    }
    if current.as_deref() != Some(content) {
        plan.changes.push(Change::Owned {
            path: owned,
            content: Some(content.to_vec()),
            pre,
        });
    }
    Ok(())
}

/// The user's file `conf`, with Velatura's marked block holding `body`.
/// Gives the file as it stands and as it would be after.
pub(crate) fn connect(
    host: &Host<'_>,
    state: &State,
    (instance, block): (&str, &str),
    conf: &Path,
    body: &str,
    plan: &mut TargetPlan,
) -> Result<(Option<Vec<u8>>, Vec<u8>), Skip> {
    let conf = conf.to_path_buf();
    let current = match host.fs.kind(&conf) {
        Ok(None) => None,
        Ok(Some(Kind::File)) => read_opt(host, &conf).ok().flatten(),
        Ok(Some(Kind::Symlink)) => {
            let real = host.fs.canonicalize(&conf).ok();
            let is_file =
                real.as_ref().and_then(|r| host.fs.kind(r).ok().flatten()) == Some(Kind::File);
            let Some(real) = real.filter(|_| is_file) else {
                return Err(Skip::NotAFile(conf));
            };
            if !inside_home(host, &conf) {
                return Err(Skip::OutsideHome(conf));
            }
            plan.notes.push(Note::LinkedFile {
                path: conf.clone(),
                real,
            });
            read_opt(host, &conf).ok().flatten()
        }
        _ => return Err(Skip::NotAFile(conf)),
    };
    if current.is_none() && exists(host, &conf) {
        return Err(Skip::Unreadable(conf));
    }
    if current.is_some() && host.fs.links(&conf).unwrap_or(1) > 1 {
        return Err(Skip::HardLinked(conf));
    }
    let before = current.clone().unwrap_or_default();
    let Some(up) = block::upsert(&before, block, body) else {
        return Err(Skip::MalformedBlock(conf));
    };
    let record = state.blocks.get(&path_str(&conf));
    if up.content != before {
        // Agreement is for adding to a file the user already has.
        plan.needs_consent =
            current.is_some() && up.before.is_none() && !state.consents.contains(instance);
        plan.changes.push(Change::Block {
            path: conf,
            target: block.to_owned(),
            pre: current.as_deref().map(sha256_hex),
            body_before: up.before.clone(),
            body_after: Some(body.to_owned()),
            added_eol: match (&up.before, record) {
                (Some(_), Some(r)) => r.added_eol,
                _ => up.added_eol,
            },
            created_file: match (&up.before, record) {
                (Some(_), Some(r)) => r.created_file,
                _ => current.is_none(),
            },
            content: Some(up.content.clone()),
        });
    }
    Ok((current, up.content))
}

fn plan_files(
    host: &Host<'_>,
    env: Env<'_>,
    state: &State,
    replace_changed: bool,
    t: &FileTarget<'_>,
) -> TargetPlan {
    if let Some(program) = t.program
        && !exists(host, &t.dir)
        && !on_path(host, env, program)
    {
        return TargetPlan::skip(t.instance, Skip::NotInstalled);
    }
    let mut plan = TargetPlan::new(t.instance);
    let mut fill = || {
        place(host, &[&t.dir], &mut plan)?;
        // The colour file: created and owned by Velatura.
        let owned = t.dir.join(t.owned);
        own(host, state, replace_changed, owned, &t.content, &mut plan)?;
        // The user's file: one marked block holding the connecting line.
        let (_, after) = connect(
            host,
            state,
            (t.instance, t.block),
            &t.conf,
            t.body,
            &mut plan,
        )?;
        let overrides = override_lines(&after, t.block, t.overrides);
        if !overrides.is_empty() {
            plan.notes.push(Note::Override {
                path: t.conf.clone(),
                lines: overrides,
            });
        }
        Ok(())
    };
    match fill() {
        Ok(()) => plan,
        Err(why) => TargetPlan::skip(t.instance, why),
    }
}

fn first_word(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

pub fn plan_kitty(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let dir = kitty_dir(host, env);
    let mut plan = plan_files(
        host,
        env,
        state,
        replace_changed,
        &FileTarget {
            instance: KITTY,
            block: "kitty",
            dir: dir.clone(),
            program: Some("kitty"),
            owned: "velatura.conf",
            content: kitty::render(theme).into_bytes(),
            conf: dir.join("kitty.conf"),
            body: "include velatura.conf",
            overrides: &|line| {
                let key = first_word(line);
                kitty::KEYS.contains(&key)
                    || matches!(key, "include" | "globinclude" | "envinclude" | "geninclude")
            },
        },
    );
    if plan.skipped.is_some() {
        return plan;
    }
    for name in [
        "dark-theme.auto.conf",
        "light-theme.auto.conf",
        "no-preference-theme.auto.conf",
    ] {
        let path = dir.join(name);
        if exists(host, &path) {
            plan.notes.push(Note::KittyAutoTheme { path });
        }
    }
    if !plan.changes.is_empty() {
        plan.notes.push(Note::KittyReload);
    }
    plan
}

/// Ghostty loads a `config-file` after the whole file that names it, so the
/// colours in it win over a `theme` line or colour keys anywhere in the
/// user's own file. Only another `config-file` after the block can override.
pub fn plan_ghostty(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let mut plan = plan_files(
        host,
        env,
        state,
        replace_changed,
        &FileTarget {
            instance: GHOSTTY,
            block: "ghostty",
            dir: ghostty_dir(host),
            program: Some("ghostty"),
            owned: "velatura.conf",
            content: ghostty::render(theme).into_bytes(),
            conf: ghostty_dir(host).join(ghostty_conf(host)),
            body: "config-file = velatura.conf",
            overrides: &|line| line.split('=').next().unwrap_or("").trim() == "config-file",
        },
    );
    if plan.skipped.is_none() && !plan.changes.is_empty() {
        plan.notes.push(Note::GhosttyReload);
    }
    plan
}

pub fn tmux_dir(host: &Host<'_>) -> PathBuf {
    host.dirs.config.join("tmux")
}

/// The file of tmux's settings that the block goes into. tmux loads
/// `~/.tmux.conf` and then the one in its settings folder, so the second
/// has the last word when it is there.
pub fn tmux_conf(host: &Host<'_>) -> PathBuf {
    let own = tmux_dir(host).join("tmux.conf");
    let legacy = host.dirs.home.join(".tmux.conf");
    if !exists(host, &own) && exists(host, &legacy) {
        legacy
    } else {
        own
    }
}

/// `path` as tmux reads it in a settings file: from the home folder where
/// that needs no quoting, so the line means the same on another machine,
/// and otherwise in single quotes, inside which tmux reads nothing but the
/// quote itself. `None` for a path that cannot be written that way.
fn tmux_path(host: &Host<'_>, path: &Path) -> Option<String> {
    let plain = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    };
    if let Ok(rest) = path.strip_prefix(&host.dirs.home)
        && let Some(rest) = rest.to_str()
        && plain(rest)
    {
        return Some(format!("~/{rest}"));
    }
    let whole = path.to_str()?;
    (!whole.contains('\'') && !whole.chars().any(char::is_control)).then(|| format!("'{whole}'"))
}

/// tmux: a file of options, loaded by one `source-file` line at the end of
/// the user's settings. `-q` keeps tmux quiet where the file is not there,
/// as on another machine that shares the settings.
pub fn plan_tmux(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let dir = tmux_dir(host);
    let conf = tmux_conf(host);
    if !exists(host, &conf) && !exists(host, &dir) && !on_path(host, env, "tmux") {
        return TargetPlan::skip(TMUX, Skip::NotInstalled);
    }
    let owned = dir.join("velatura.conf");
    let Some(shown) = tmux_path(host, &owned) else {
        return TargetPlan::skip(TMUX, Skip::UnusualPath(owned));
    };
    let mut plan = plan_files(
        host,
        env,
        state,
        replace_changed,
        &FileTarget {
            instance: TMUX,
            block: "tmux",
            dir,
            program: None,
            owned: "velatura.conf",
            content: tmux::render(theme).into_bytes(),
            conf: conf.clone(),
            body: &format!("source-file -q {shown}"),
            overrides: &|line| {
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("source-file" | "source") => true,
                    Some("set" | "set-option" | "setw" | "set-window-option") => {
                        words.any(|w| tmux::OPTIONS.contains(&w))
                    }
                    _ => false,
                }
            },
        },
    );
    if plan.skipped.is_none() && !plan.changes.is_empty() {
        plan.notes.push(Note::TmuxReload { conf });
    }
    plan
}

/// True until `instance` has been agreed to once, or has a file of
/// Velatura's under `dir`. An app that gets no line in a file of the
/// user's still gets its first change only when that is agreed to, so a
/// run with no one present never starts theming an app by itself.
pub(crate) fn first_time(state: &State, instance: &str, dir: &Path) -> bool {
    let ours = state
        .owned
        .keys()
        .any(|path| Path::new(path).starts_with(dir));
    !ours && !state.consents.contains(instance)
}

/// One text setting brought to `wanted`, or the note that it is locked.
pub(crate) fn set_text(
    host: &Host<'_>,
    (schema, key): (&str, &str),
    wanted: &str,
    plan: &mut TargetPlan,
) -> Result<(), Skip> {
    match host.settings.read(schema, key) {
        Ok(Some(now)) if now.effective == wanted => {}
        Ok(Some(now)) if now.writable => plan.changes.push(Change::Setting {
            schema: schema.to_owned(),
            key: key.to_owned(),
            value: Some(wanted.to_owned()),
            pre: now.user,
        }),
        Ok(Some(_)) => plan.notes.push(Note::SettingLocked {
            key: key.to_owned(),
        }),
        Ok(None) => return Err(Skip::SettingsUnavailable(format!("{schema} {key}"))),
        Err(SettingsError::Unavailable(why)) => return Err(Skip::SettingsUnavailable(why)),
        Err(e) => return Err(Skip::SettingsUnavailable(e.to_string())),
    }
    Ok(())
}

pub const PTYXIS_SCHEMA: &str = "org.gnome.Ptyxis";

/// The settings of one Ptyxis profile, which live at a path made of its id.
pub fn ptyxis_profile(uuid: &str) -> Option<String> {
    let plain = !uuid.is_empty()
        && uuid.len() <= 64
        && uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    plain.then(|| format!("org.gnome.Ptyxis.Profile:/org/gnome/Ptyxis/Profiles/{uuid}/"))
}

/// Ptyxis: a palette of Velatura's own among the user's palettes, and the
/// default profile set to use it. Other profiles are left as they are.
pub fn plan_ptyxis(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let dir = host.dirs.data.join("org.gnome.Ptyxis");
    let default = match host.settings.read(PTYXIS_SCHEMA, "default-profile-uuid") {
        Ok(Some(found)) => found.effective,
        // Without its settings this is not the Ptyxis that reads this
        // folder, if it is here at all.
        Ok(None) => return TargetPlan::skip(PTYXIS, Skip::NotInstalled),
        Err(SettingsError::Unavailable(why)) => {
            return TargetPlan::skip(PTYXIS, Skip::SettingsUnavailable(why));
        }
        Err(e) => return TargetPlan::skip(PTYXIS, Skip::SettingsUnavailable(e.to_string())),
    };
    if !exists(host, &dir) && !on_path(host, env, "ptyxis") {
        return TargetPlan::skip(PTYXIS, Skip::NotInstalled);
    }
    let palettes = dir.join("palettes");
    let file = palettes.join(format!("{}.palette", ptyxis::ID));
    let mut plan = TargetPlan::new(PTYXIS);
    let mut fill = || {
        place(host, &[&dir, &palettes], &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            file.clone(),
            ptyxis::render(theme).as_bytes(),
            &mut plan,
        )?;
        let rewritten = !plan.changes.is_empty();
        // Before its first start Ptyxis has no profile to set.
        match ptyxis_profile(&default) {
            Some(profile) => set_text(host, (&profile, "palette"), ptyxis::ID, &mut plan)?,
            None => plan.notes.push(Note::PtyxisChoose),
        }
        Ok(rewritten)
    };
    match fill() {
        Ok(rewritten) => {
            if !plan.changes.is_empty() {
                plan.needs_consent = first_time(state, PTYXIS, &palettes);
                let profiles = host
                    .settings
                    .read(PTYXIS_SCHEMA, "profile-uuids")
                    .ok()
                    .flatten()
                    .map_or(0, |all| listed(&all.effective).len());
                if profiles > 1 {
                    plan.notes.push(Note::PtyxisChoose);
                }
                if rewritten {
                    plan.notes.push(Note::PtyxisReopen);
                }
                plan.notes.dedup();
            } else {
                plan.notes.clear();
            }
            plan
        }
        Err(why) => TargetPlan::skip(PTYXIS, why),
    }
}

pub const TEXT_EDITOR_SCHEMA: &str = "org.gnome.TextEditor";

/// GNOME Text Editor: a style scheme of Velatura's own where GtkSourceView
/// looks for the user's, and the editor set to use it. Other apps that
/// show code with GtkSourceView find the scheme there too, to choose.
pub fn plan_text_editor(
    host: &Host<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    match host.settings.read(TEXT_EDITOR_SCHEMA, "style-scheme") {
        Ok(Some(_)) => {}
        Ok(None) => return TargetPlan::skip(TEXT_EDITOR, Skip::NotInstalled),
        Err(SettingsError::Unavailable(why)) => {
            return TargetPlan::skip(TEXT_EDITOR, Skip::SettingsUnavailable(why));
        }
        Err(e) => {
            return TargetPlan::skip(TEXT_EDITOR, Skip::SettingsUnavailable(e.to_string()));
        }
    }
    let dir = host.dirs.data.join("gtksourceview-5");
    let styles = dir.join("styles");
    let mut plan = TargetPlan::new(TEXT_EDITOR);
    let mut fill = || {
        place(host, &[&dir, &styles], &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            styles.join(format!("{}.xml", sourceview::ID)),
            sourceview::render(theme).as_bytes(),
            &mut plan,
        )?;
        set_text(
            host,
            (TEXT_EDITOR_SCHEMA, "style-scheme"),
            sourceview::ID,
            &mut plan,
        )
    };
    match fill() {
        Ok(()) => {
            if !plan.changes.is_empty() {
                plan.needs_consent = first_time(state, TEXT_EDITOR, &styles);
                plan.notes.push(Note::GtkRestart);
            }
            plan
        }
        Err(why) => TargetPlan::skip(TEXT_EDITOR, why),
    }
}

/// Neovim: a colourscheme of Velatura's own in the settings folder, the
/// same colours as a table for a colourscheme the user keeps, and, unless
/// the user's settings already choose it, one line at the end of
/// `init.lua` (or `init.vim`) that chooses it once Neovim has started.
/// Everything else in the settings stays as it is, and runs as before.
pub fn plan_nvim(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    let dir = host.dirs.config.join("nvim");
    if !exists(host, &dir) && !on_path(host, env, "nvim") {
        return TargetPlan::skip(NVIM, Skip::NotInstalled);
    }
    let colors = dir.join("colors");
    let lua = dir.join("lua");
    let module = lua.join(nvim::NAME);
    let (init, block, body) = nvim_init(host, &dir);
    let mut plan = TargetPlan::new(NVIM);
    let mut fill = || {
        place(host, &[&dir, &colors], &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            colors.join(format!("{}.lua", nvim::NAME)),
            nvim::render(theme).as_bytes(),
            &mut plan,
        )?;
        place(host, &[&lua, &module], &mut plan)?;
        own(
            host,
            state,
            replace_changed,
            module.join("palette.lua"),
            nvim::render_palette(theme).as_bytes(),
            &mut plan,
        )?;
        let existing = read_opt(host, &init).ok().flatten();
        let ours = existing
            .as_deref()
            .is_some_and(|b| matches!(block::find(b, block), block::Found::Present { .. }));
        let chosen = nvim_chosen(host, &dir);
        if ours || !chosen.velatura {
            let (current, _) = connect(host, state, (NVIM, block), &init, body, &mut plan)?;
            // Choosing the colourscheme in a file the user keeps is asked
            // the first time, even where Neovim was agreed to before.
            let adding = current.is_some() && !ours;
            if adding {
                plan.notes.push(Note::NeovimOtherScheme {
                    name: chosen.other,
                    path: init.clone(),
                });
            }
            return Ok(adding && !state.blocks.contains_key(&path_str(&init)));
        }
        Ok(false)
    };
    match fill() {
        Ok(asks) => {
            if !plan.changes.is_empty() {
                // The folder is the user's: the first files go in only when
                // agreed to, and so never in a run with no one present.
                plan.needs_consent = asks || (exists(host, &dir) && first_time(state, NVIM, &dir));
                plan.notes.push(Note::NeovimReload);
            }
            plan
        }
        Err(why) => TargetPlan::skip(NVIM, why),
    }
}

/// The settings file the line goes in, the block's name there, and the line.
fn nvim_init(host: &Host<'_>, dir: &Path) -> (PathBuf, &'static str, &'static str) {
    let vim = dir.join("init.vim");
    if !exists(host, &dir.join("init.lua")) && exists(host, &vim) {
        return (
            vim,
            block::NVIM_VIM,
            "autocmd VimEnter * ++once ++nested colorscheme velatura",
        );
    }
    (
        dir.join("init.lua"),
        block::NVIM_LUA,
        "vim.api.nvim_create_autocmd(\"VimEnter\", { once = true, nested = true, command = \"colorscheme velatura\" })",
    )
}

/// What Neovim's settings choose, as far as reading them tells.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NvimChosen {
    /// They choose Velatura's colourscheme themselves.
    pub velatura: bool,
    /// The first other colourscheme they choose, for saying so.
    pub other: Option<String>,
}

/// Read the files of Neovim's settings for the colourscheme they choose:
/// `init.lua` or `init.vim` and what `lua`, `plugin` and `after` hold.
/// Velatura's own files and blocks are left out. Bounded, and reads only.
pub fn nvim_chosen(host: &Host<'_>, dir: &Path) -> NvimChosen {
    const MAX_FILES: usize = 2000;
    const MAX_BYTES: usize = 512 * 1024;
    const MAX_DEPTH: usize = 8;
    let mut files = vec![dir.join("init.lua"), dir.join("init.vim")];
    let mut stack: Vec<(PathBuf, usize)> = ["lua", "plugin", "after"]
        .iter()
        .map(|d| (dir.join(d), 0))
        .collect();
    let skip = dir.join("lua").join(nvim::NAME);
    while let Some((folder, depth)) = stack.pop() {
        if folder == skip || depth > MAX_DEPTH || files.len() > MAX_FILES {
            continue;
        }
        let mut names = host.fs.list(&folder).unwrap_or_default();
        names.sort();
        for name in names {
            let path = folder.join(&name);
            match host.fs.kind(&path) {
                Ok(Some(Kind::Dir)) => stack.push((path, depth + 1)),
                // A linked file is read where it leads; a linked folder is
                // not followed, so a loop cannot keep the walk going.
                Ok(Some(Kind::File | Kind::Symlink)) => {
                    let n = name.to_string_lossy();
                    if n.ends_with(".lua") || n.ends_with(".vim") {
                        files.push(path);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = NvimChosen::default();
    for path in files.iter().take(MAX_FILES) {
        let Some(bytes) = read_opt(host, path).ok().flatten() else {
            continue;
        };
        if bytes.len() > MAX_BYTES {
            continue;
        }
        let mut text = bytes;
        for b in [block::NVIM_LUA, block::NVIM_VIM] {
            if let block::Found::Present { start, end, .. } = block::find(&text, b) {
                text.drain(start..end);
            }
        }
        for (name, command) in colorschemes(&String::from_utf8_lossy(&text)) {
            if name == nvim::NAME {
                out.velatura = true;
            } else if command && out.other.is_none() {
                out.other = Some(name);
            }
        }
    }
    out
}

/// The colourschemes a Lua or Vim script names, each with whether it is
/// named in a command (`colorscheme x`, `vim.cmd.colorscheme("x")`) rather
/// than as a setting (`colorscheme = "x"`, as LazyVim takes it). Comments
/// are passed over; `ColorScheme`, the event, is another word.
fn colorschemes(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split_once("--").map_or(line, |(code, _)| code);
        if line.trim_start().starts_with('"') {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("colorscheme") {
            let before = rest[..at].chars().next_back();
            rest = &rest[at + "colorscheme".len()..];
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let after = rest.trim_start_matches([' ', '\t']);
            let setting = after.starts_with(['=', ':']);
            let command = !setting
                && (after.len() < rest.len() || after.starts_with('(') || after.starts_with('!'));
            if !setting && !command {
                continue;
            }
            let name: String = after
                .trim_start_matches(['=', ':', '(', '!', '{', '[', ' ', '\t', '"', '\''])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                .take(64)
                .collect();
            if !name.is_empty() {
                out.push((name, command));
            }
        }
    }
    out
}

/// Starship's settings file: `STARSHIP_CONFIG`, or `starship.toml` in the
/// settings folder.
pub fn starship_conf(host: &Host<'_>, env: Env<'_>) -> PathBuf {
    env("STARSHIP_CONFIG")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| host.dirs.config.join("starship.toml"))
}

fn is_toml(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| text.parse::<toml::Table>().is_ok())
}

/// Starship cannot load a second file, so the palette goes into the user's
/// own, as keys of the file's root inside a marked block at its top, with
/// the one line that selects it. A file that already selects a palette is
/// left alone: its prompt is written for that palette's names.
pub fn plan_starship(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
) -> TargetPlan {
    let conf = starship_conf(host, env);
    if !exists(host, &conf) && !on_path(host, env, "starship") {
        return TargetPlan::skip(STARSHIP, Skip::NotInstalled);
    }
    if !conf.parent().is_some_and(|dir| inside_home(host, dir)) {
        return TargetPlan::skip(STARSHIP, Skip::OutsideHome(conf));
    }
    let mut plan = TargetPlan::new(STARSHIP);
    let body = starship::render(theme);
    match connect(
        host,
        state,
        (STARSHIP, "starship"),
        &conf,
        body.trim_end(),
        &mut plan,
    ) {
        Err(why) => TargetPlan::skip(STARSHIP, why),
        Ok((Some(before), _)) if !is_toml(&before) => {
            TargetPlan::skip(STARSHIP, Skip::Invalid(conf))
        }
        Ok((_, after)) if !is_toml(&after) => TargetPlan::skip(STARSHIP, Skip::OwnPalette(conf)),
        Ok(_) => {
            if !plan.changes.is_empty() {
                plan.notes.push(Note::StarshipNames);
            }
            plan
        }
    }
}

pub(crate) fn high_contrast(host: &Host<'_>) -> bool {
    matches!(
        host.settings.read(A11Y_SCHEMA, "high-contrast"),
        Ok(Some(s)) if s.effective == "true"
    )
}

/// True for a line that defines one of the colours the theme defines.
fn defines_theme_color(line: &str, names: &[&str]) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("@define-color") && words.next().is_some_and(|n| names.contains(&n))
}

fn plan_gtk(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
    (instance, block, folder): (&str, &str, &str),
) -> TargetPlan {
    let names = gtk::names(theme);
    let mut plan = plan_files(
        host,
        env,
        state,
        replace_changed,
        &FileTarget {
            instance,
            block,
            dir: host.dirs.config.join(folder),
            program: None,
            owned: "velatura.css",
            content: gtk::render(theme).into_bytes(),
            conf: host.dirs.config.join(folder).join("gtk.css"),
            body: "@import url(\"velatura.css\");",
            overrides: &|line| defines_theme_color(line, &names),
        },
    );
    if plan.skipped.is_none() && !plan.changes.is_empty() {
        plan.notes.push(Note::GtkRestart);
    }
    plan
}

pub fn plan_gtk4(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    if high_contrast(host) {
        return TargetPlan::skip(GTK4, Skip::HighContrast);
    }
    plan_gtk(
        host,
        env,
        theme,
        state,
        replace_changed,
        (GTK4, "gtk4", "gtk-4.0"),
    )
}

/// Folders GTK3 looks in for themes, the user's first.
fn theme_dirs(host: &Host<'_>, env: Env<'_>) -> Vec<PathBuf> {
    let mut dirs = vec![
        host.dirs.data.join("themes"),
        host.dirs.home.join(".themes"),
    ];
    let system = env("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    dirs.extend(
        std::env::split_paths(&system)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("themes")),
    );
    dirs
}

/// The stock GTK3 theme has its colours compiled in, so GTK3 can only be
/// recoloured through adw-gtk3, which reads the same names as libadwaita.
pub fn plan_gtk3(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    replace_changed: bool,
) -> TargetPlan {
    if high_contrast(host) {
        return TargetPlan::skip(GTK3, Skip::HighContrast);
    }
    let wanted = match theme.colors.appearance {
        Appearance::Dark => "adw-gtk3-dark",
        Appearance::Light => "adw-gtk3",
    };
    let installed = theme_dirs(host, env)
        .iter()
        .any(|dir| exists(host, &dir.join(wanted).join("gtk-3.0").join("gtk.css")));
    if !installed {
        return TargetPlan::skip(GTK3, Skip::NeedsAdwGtk3);
    }
    let mut plan = plan_gtk(
        host,
        env,
        theme,
        state,
        replace_changed,
        (GTK3, "gtk3", "gtk-3.0"),
    );
    if plan.skipped.is_some() {
        return plan;
    }
    match host.settings.read(INTERFACE_SCHEMA, "gtk-theme") {
        Ok(Some(now)) if now.effective == wanted => {}
        Ok(Some(now)) if now.writable => {
            plan.changes.push(Change::Setting {
                schema: INTERFACE_SCHEMA.to_owned(),
                key: "gtk-theme".to_owned(),
                value: Some(wanted.to_owned()),
                pre: now.user,
            });
            if !plan.notes.contains(&Note::GtkRestart) {
                plan.notes.push(Note::GtkRestart);
            }
        }
        Ok(Some(_)) => plan.notes.push(Note::SettingLocked {
            key: "gtk-theme".to_owned(),
        }),
        // Without the setting the colour file is still correct for the day
        // the theme is selected some other way.
        _ => {}
    }
    // A legacy file can force dark and fight a light theme.
    let ini = host.dirs.config.join("gtk-3.0").join("settings.ini");
    if theme.colors.appearance == Appearance::Light
        && let Ok(Some(bytes)) = read_opt(host, &ini)
        && String::from_utf8_lossy(&bytes).lines().any(|l| {
            let l: String = l.split_whitespace().collect();
            l == "gtk-application-prefer-dark-theme=1"
                || l == "gtk-application-prefer-dark-theme=true"
        })
    {
        plan.notes.push(Note::Gtk3DarkForced { path: ini });
    }
    plan
}

/// True if a list member already decides access to the folder `want`
/// names: granted in any mode, or denied. Either way it is the user's.
fn same_folder(have: &str, want: &str) -> bool {
    let folder = |m: &str| {
        m.trim_start_matches('!')
            .split(':')
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_owned()
    };
    folder(have) == folder(want)
}

/// Flatpak apps cannot see the GTK folders in the home folder, so the
/// colour files never reach them. This adds read-only access to those two
/// folders to Flatpak's per-user overrides, for every app, as members of
/// one list. Members that were already there are not Velatura's and are
/// never taken out.
pub fn plan_flatpak(host: &Host<'_>, env: Env<'_>, state: &State) -> TargetPlan {
    let home = host.dirs.data.join("flatpak");
    let system = Path::new("/var/lib/flatpak");
    if !exists(host, &home) && !exists(host, system) && !on_path(host, env, "flatpak") {
        return TargetPlan::skip(FLATPAK, Skip::NotInstalled);
    }
    if !inside_home(host, &host.dirs.data) {
        return TargetPlan::skip(FLATPAK, Skip::OutsideHome(home));
    }
    let mut plan = TargetPlan::new(FLATPAK);
    let dir = home.join("overrides");
    for level in [&home, &dir] {
        match host.fs.kind(level) {
            Ok(None) => plan.changes.push(Change::CreateDir {
                path: level.clone(),
            }),
            Ok(Some(Kind::Dir)) => {}
            _ => return TargetPlan::skip(FLATPAK, Skip::NotAFile(level.clone())),
        }
    }
    let file = dir.join("global");
    let current = match host.fs.kind(&file) {
        Ok(None) => None,
        Ok(Some(Kind::File)) => match host.fs.read(&file) {
            Ok(bytes) => Some(bytes),
            Err(_) => return TargetPlan::skip(FLATPAK, Skip::Unreadable(file)),
        },
        _ => return TargetPlan::skip(FLATPAK, Skip::NotAFile(file)),
    };
    if current.is_some() && host.fs.links(&file).unwrap_or(1) > 1 {
        return TargetPlan::skip(FLATPAK, Skip::HardLinked(file));
    }
    let before = current.clone().unwrap_or_default();
    let Some(edit) = crate::keyfile::add(
        &before,
        FLATPAK_GROUP,
        FLATPAK_KEY,
        &FLATPAK_ACCESS,
        &same_folder,
    ) else {
        return TargetPlan::skip(FLATPAK, Skip::Unreadable(file));
    };
    if edit.added.is_empty() {
        // Already granted, by Velatura earlier or by the user.
        plan.changes.clear();
        return plan;
    }
    // Something was added before and part of it is gone again: the list is
    // the user's to arrange, so it is not second-guessed.
    if state.blocks.contains_key(&path_str(&file)) {
        plan.changes.clear();
        plan.notes.push(Note::KeptFile { path: file });
        return plan;
    }
    plan.needs_consent = current.is_some() && !state.consents.contains(FLATPAK);
    plan.changes.push(Change::Block {
        path: file,
        target: FLATPAK_SPAN.to_owned(),
        pre: current.as_deref().map(sha256_hex),
        content: Some(edit.content),
        body_before: None,
        body_after: Some(edit.receipt),
        added_eol: edit.added_eol,
        created_file: current.is_none(),
    });
    plan.notes.push(Note::GtkRestart);
    plan
}

/// The running Shell's version, asked of the Shell itself.
pub const SHELL_VERSION: (&str, &str) = ("dbus:org.gnome.Shell", "ShellVersion");

/// GNOME Shell versions whose own stylesheet has every selector Velatura
/// overrides, checked by a test against a list taken from each.
pub const TESTED_SHELLS: std::ops::RangeInclusive<u32> = 47..=51;

/// The running Shell's version if it is one Velatura was not checked
/// with. `None` for a tested one, and when no Shell answers.
fn untested_shell(host: &Host<'_>) -> Option<String> {
    let version = host
        .settings
        .read(SHELL_VERSION.0, SHELL_VERSION.1)
        .ok()
        .flatten()?
        .effective;
    let major: u32 = version.split('.').next()?.parse().ok()?;
    (!TESTED_SHELLS.contains(&major)).then_some(version)
}

pub const USER_THEMES: &str = "user-theme@gnome-shell-extensions.gcampax.github.com";
const USER_THEMES_SCHEMA: &str = "org.gnome.shell.extensions.user-theme";

/// Where the system keeps shared data, the user's folder first.
pub(crate) fn data_dirs(host: &Host<'_>, env: Env<'_>) -> Vec<PathBuf> {
    let mut dirs = vec![host.dirs.data.clone()];
    let system = env("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    dirs.extend(std::env::split_paths(&system).filter(|p| p.is_absolute()));
    dirs
}

/// The uuids in the printed form of a list of strings: `['a', 'b']`.
pub(crate) fn listed(printed: &str) -> Vec<&str> {
    printed.split('\'').skip(1).step_by(2).collect()
}

/// The bundle GNOME Shell keeps its own stylesheets in, under one of the
/// system's data folders. The user's own folder is not looked in: the
/// Shell does not load its theme from there.
const SHELL_BUNDLE: &str = "gnome-shell/gnome-shell-theme.gresource";
/// A bundle larger than this is not the Shell's.
const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

/// The installed Shell's own stylesheet for `appearance`, as it ships it.
/// `None` where it cannot be read, and in Ubuntu's own session, whose
/// Shell draws with another stylesheet (Yaru).
pub fn shell_stylesheet(host: &Host<'_>, env: Env<'_>, appearance: Appearance) -> Option<String> {
    let desktop = env("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktop = desktop.to_string_lossy().to_lowercase();
    let ubuntu_mode = data_dirs(host, env)
        .iter()
        .skip(1)
        .any(|d| exists(host, &d.join("gnome-shell/modes/ubuntu.json")));
    if desktop.contains("ubuntu") || (ubuntu_mode && desktop != "gnome") {
        return None;
    }
    let bundle = data_dirs(host, env)
        .into_iter()
        .skip(1)
        .map(|d| d.join(SHELL_BUNDLE))
        .find(|p| exists(host, p))?;
    let bytes = read_opt(host, &bundle).ok().flatten()?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return None;
    }
    let name = match appearance {
        Appearance::Dark => "dark",
        Appearance::Light => "light",
    };
    let css = crate::gresource::file(
        &bytes,
        &format!("/org/gnome/shell/theme/gnome-shell-{name}.css"),
    )?;
    String::from_utf8(css.to_vec()).ok()
}

/// What the Shell target writes: the Shell's own stylesheet restated in
/// the theme's colours, or, where that cannot be read, a short list of
/// its main surfaces, said in a note.
fn shell_css(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    heavy: bool,
    plan: &mut TargetPlan,
) -> Vec<u8> {
    let restated = shell_stylesheet(host, env, theme.colors.appearance)
        .and_then(|stock| shell::restate(theme, &stock, heavy));
    match restated {
        Some(css) => css.into_bytes(),
        None => {
            plan.notes.push(Note::ShellShortList);
            if heavy {
                shell::render_for_extension(theme).into_bytes()
            } else {
                shell::render(theme).into_bytes()
            }
        }
    }
}

/// The top bar, overview, menus and dialogs, through the User Themes
/// extension. The generated theme is a folder holding one stylesheet; its
/// name changes with the colours, because the Shell reloads a theme only
/// when the name it is given changes.
pub fn plan_shell(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    clear_old: bool,
    untested: bool,
) -> TargetPlan {
    if high_contrast(host) {
        return TargetPlan::skip(SHELL, Skip::HighContrast);
    }
    // Selectors change between versions. On one that was not checked the
    // overrides may miss, or hit something else, so it is left alone
    // unless asked for outright.
    if !untested && let Some(version) = untested_shell(host) {
        return TargetPlan::skip(SHELL, Skip::UntestedShell(version));
    }
    // Velatura's own extension, when it is on, is the better way in: its
    // stylesheet sits with those of other extensions, not under them.
    if crate::companion::status(host, env).active() {
        return plan_shell_companion(host, env, theme, state, clear_old);
    }
    let Some(extension) = data_dirs(host, env)
        .into_iter()
        .map(|d| d.join("gnome-shell/extensions").join(USER_THEMES))
        .find(|d| exists(host, d))
    else {
        return TargetPlan::skip(SHELL, Skip::NeedsUserThemes);
    };
    // Installed for this user only, the extension brings its own schema.
    let own = extension.join("schemas");
    let schema = if exists(host, &own.join("gschemas.compiled")) {
        format!("{USER_THEMES_SCHEMA}@{}", path_str(&own))
    } else {
        USER_THEMES_SCHEMA.to_owned()
    };
    let now = match host.settings.read(&schema, "name") {
        Ok(Some(now)) => now,
        Ok(None) => return TargetPlan::skip(SHELL, Skip::SettingsUnavailable(schema)),
        Err(e) => return TargetPlan::skip(SHELL, Skip::SettingsUnavailable(e.to_string())),
    };

    let mut plan = TargetPlan::new(SHELL);
    let themes = host.dirs.data.join("themes");
    let name = shell::folder(theme);
    let folder = themes.join(&name);
    let inner = folder.join("gnome-shell");
    let css = inner.join("gnome-shell.css");
    let wanted = shell_css(host, env, theme, false, &mut plan);

    let current = read_opt(host, &css).ok().flatten();
    let pre = current.as_deref().map(sha256_hex);
    if pre.is_some() && !state.owned.contains_key(&path_str(&css)) {
        return TargetPlan::skip(SHELL, Skip::ForeignFile(css));
    }
    if !inside_home(host, &host.dirs.data) {
        return TargetPlan::skip(SHELL, Skip::OutsideHome(themes));
    }
    for dir in [&themes, &folder, &inner] {
        if !exists(host, dir) {
            plan.changes.push(Change::CreateDir { path: dir.clone() });
        }
    }
    if current.as_deref() != Some(wanted.as_slice()) {
        plan.changes.push(Change::Owned {
            path: css.clone(),
            content: Some(wanted),
            pre,
        });
    }
    if clear_old {
        clear_generated(host, state, Some(&css), &mut plan);
    }
    if now.effective != name {
        if now.writable {
            plan.changes.push(Change::Setting {
                schema,
                key: "name".to_owned(),
                value: Some(name),
                pre: now.user,
            });
        } else {
            plan.notes.push(Note::SettingLocked {
                key: "name".to_owned(),
            });
        }
    }

    // What decides whether the theme is seen, read from the Shell's own
    // settings. Absent keys mean an unknown state, and no note.
    if let Ok(Some(enabled)) = host.settings.read("org.gnome.shell", "enabled-extensions") {
        let enabled = listed(&enabled.effective);
        let user_off = matches!(
            host.settings.read("org.gnome.shell", "disable-user-extensions"),
            Ok(Some(s)) if s.effective == "true"
        );
        if user_off || !enabled.contains(&USER_THEMES) {
            plan.notes.push(Note::UserThemesOff);
        }
        let styled: Vec<String> = enabled
            .iter()
            .filter(|uuid| **uuid != USER_THEMES)
            .filter(|uuid| {
                data_dirs(host, env).iter().any(|d| {
                    let sheet = d
                        .join("gnome-shell/extensions")
                        .join(uuid)
                        .join("stylesheet.css");
                    crate::icon_set::read_capped(host, &sheet, 1024 * 1024)
                        .is_some_and(|css| colors_shell(&String::from_utf8_lossy(&css)))
                })
            })
            .map(|uuid| (*uuid).to_owned())
            .collect();
        if !styled.is_empty() {
            plan.notes
                .push(Note::ShellExtensionStyles { names: styled });
        }
    }
    plan
}

/// True if an extension's stylesheet colours the Shell's own surfaces: a
/// rule that starts with the top bar, a menu, quick settings, the
/// calendar, a notification, the overview's parts or a pop-up, and sets a
/// colour. Rules for the extension's own widgets, or that apply only under
/// a class of its own (an option switched on), do not count: they leave
/// the theme alone.
pub fn colors_shell(css: &str) -> bool {
    const SURFACES: [&str; 15] = [
        "#panel",
        ".panel-button",
        ".popup-menu",
        ".quick-settings",
        ".quick-toggle",
        ".calendar",
        ".message",
        "#overviewGroup",
        "#dash",
        ".dash-background",
        ".osd-window",
        ".switcher-list",
        ".modal-dialog",
        ".search-entry",
        ".candidate-popup",
    ];
    let mut text = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("/*") {
        text.push_str(&rest[..at]);
        rest = rest[at + 2..]
            .find("*/")
            .map_or("", |end| &rest[at + 2 + end + 2..]);
    }
    text.push_str(rest);
    let colours = |body: &str| {
        body.split(';').any(|d| {
            d.split_once(':').is_some_and(|(p, _)| {
                matches!(
                    p.trim(),
                    "color" | "background" | "background-color" | "border" | "border-color"
                )
            })
        })
    };
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else {
            break;
        };
        let (selectors, body) = (&rest[..open], &rest[open + 1..close]);
        let starts_on_surface = selectors.split(',').any(|s| {
            let s = s.trim();
            SURFACES.iter().any(|m| {
                s.strip_prefix(m).is_some_and(|after| {
                    !after.starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')
                })
            })
        });
        if starts_on_surface && colours(body) {
            return true;
        }
        rest = &rest[close + 1..];
    }
    false
}

/// Shell stylesheets Velatura wrote for an earlier theme, or for the other
/// of its two ways into the Shell, are its own and are cleared away. They
/// stay restorable: undo puts them back from the backup. `keep` is the one
/// being written now.
fn clear_generated(host: &Host<'_>, state: &State, keep: Option<&Path>, plan: &mut TargetPlan) {
    let themes = host.dirs.data.join("themes");
    let companion = crate::companion::stylesheet(host);
    for (old, written) in &state.owned {
        let old = PathBuf::from(old);
        let generated = old == companion
            || old.strip_prefix(&themes).is_ok_and(|rest| {
                rest.to_string_lossy().starts_with("Velatura-")
                    && rest.ends_with("gnome-shell/gnome-shell.css")
            });
        if !generated || Some(old.as_path()) == keep {
            continue;
        }
        // The theme's folder and the one inside it; for the extension's
        // stylesheet, the one folder made for it.
        let levels = if old == companion { 1 } else { 2 };
        match read_opt(host, &old)
            .ok()
            .flatten()
            .as_deref()
            .map(sha256_hex)
        {
            Some(hash) if hash == *written => {
                plan.changes.push(Change::Owned {
                    path: old.clone(),
                    content: None,
                    pre: Some(hash),
                });
                for dir in old.ancestors().skip(1).take(levels) {
                    plan.changes.push(Change::RemoveDir {
                        path: dir.to_path_buf(),
                    });
                }
            }
            Some(_) => plan.notes.push(Note::KeptFile { path: old }),
            None => {}
        }
    }
}

/// The Shell through Velatura's own extension: one stylesheet at a fixed
/// place, replaced in place. The extension sees the change and loads it,
/// so there is no folder per theme and no setting to flip.
/// The outline around the focused window: a stylesheet of one rule that
/// Velatura's own extension reads, giving the outline it draws the
/// theme's accent. Only that extension draws it.
pub fn plan_outline(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
) -> TargetPlan {
    let css = crate::companion::outline_stylesheet(host);
    plan_extension_sheet(OUTLINE, host, env, state, css, shell::outline(theme))
}

/// The top bar as a full bar or as pills, as opaque as chosen: a
/// stylesheet Velatura's own extension loads after the Shell part's.
pub fn plan_top_bar(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
) -> TargetPlan {
    let bar = crate::store::Prefs::load(host)
        .map(|p| p.top_bar)
        .unwrap_or_default();
    let css = crate::companion::top_bar_stylesheet(host);
    plan_extension_sheet(TOP_BAR, host, env, state, css, shell::top_bar(theme, &bar))
}

/// The UPS indicator in the top bar: Velatura's extension shows it while
/// its stylesheet is there, and reads the UPS from Network UPS Tools.
pub fn plan_ups(host: &Host<'_>, env: Env<'_>, theme: &Resolved<'_>, state: &State) -> TargetPlan {
    let css = crate::companion::ups_stylesheet(host);
    plan_extension_sheet(UPS, host, env, state, css, shell::ups(theme))
}

/// The checklist in the top bar: Velatura's extension shows it while its
/// stylesheet is there, and keeps the tasks itself. Only the look is
/// written here; the list is the user's and no part of any apply.
pub fn plan_tasks(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
) -> TargetPlan {
    let css = crate::companion::tasks_stylesheet(host);
    plan_extension_sheet(TASKS, host, env, state, css, shell::tasks(theme))
}

/// The media player in the top bar: Velatura's extension shows it while
/// its stylesheet is there, and reads the players through MPRIS.
pub fn plan_media(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
) -> TargetPlan {
    let css = crate::companion::media_stylesheet(host);
    plan_extension_sheet(MEDIA, host, env, state, css, shell::media(theme))
}

/// A stylesheet of a part only Velatura's extension can show, written in
/// place at `css`.
fn plan_extension_sheet(
    instance: &str,
    host: &Host<'_>,
    env: Env<'_>,
    state: &State,
    css: PathBuf,
    wanted: String,
) -> TargetPlan {
    let status = crate::companion::status(host, env);
    if !status.active() {
        return TargetPlan::skip(instance, Skip::NeedsVelaturaExtension);
    }
    if status.outdated {
        return TargetPlan::skip(instance, Skip::VelaturaExtensionOutdated);
    }
    let waiting = status.waiting;
    let wanted = wanted.into_bytes();
    let current = read_opt(host, &css).ok().flatten();
    let pre = current.as_deref().map(sha256_hex);
    if pre.is_some() && !state.owned.contains_key(&path_str(&css)) {
        return TargetPlan::skip(instance, Skip::ForeignFile(css));
    }
    if !inside_home(host, &host.dirs.data) {
        return TargetPlan::skip(instance, Skip::OutsideHome(css));
    }
    let mut plan = TargetPlan::new(instance);
    if let Some(dir) = css.parent().filter(|dir| !exists(host, dir)) {
        plan.changes.push(Change::CreateDir {
            path: dir.to_path_buf(),
        });
    }
    if current.as_deref() != Some(wanted.as_slice()) {
        plan.changes.push(Change::Owned {
            path: css,
            content: Some(wanted),
            pre,
        });
    }
    // Written all the same, so it shows as soon as the new copy runs.
    if waiting {
        plan.notes.push(Note::ExtensionWaiting);
    }
    plan
}

/// A part of [`SHEETS`] taken away, once it is turned off:
/// its stylesheet goes, and with it what the extension shows. Nothing
/// else of the Shell's.
pub fn plan_sheet_off(instance: &str, host: &Host<'_>, state: &State) -> TargetPlan {
    let css = match instance {
        TOP_BAR => crate::companion::top_bar_stylesheet(host),
        UPS => crate::companion::ups_stylesheet(host),
        TASKS => crate::companion::tasks_stylesheet(host),
        MEDIA => crate::companion::media_stylesheet(host),
        _ => crate::companion::outline_stylesheet(host),
    };
    let mut plan = TargetPlan::new(instance);
    let Some(written) = state.owned.get(&path_str(&css)) else {
        return TargetPlan::skip(instance, Skip::NotSelected);
    };
    match read_opt(host, &css)
        .ok()
        .flatten()
        .as_deref()
        .map(sha256_hex)
    {
        Some(hash) if hash == *written => plan.changes.push(Change::Owned {
            path: css,
            content: None,
            pre: Some(hash),
        }),
        Some(_) => plan.notes.push(Note::KeptFile { path: css }),
        None => {}
    }
    if plan.changes.is_empty() && plan.notes.is_empty() {
        return TargetPlan::skip(instance, Skip::NotSelected);
    }
    plan
}

fn plan_shell_companion(
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    clear_old: bool,
) -> TargetPlan {
    let css = crate::companion::stylesheet(host);
    let mut notes = TargetPlan::new(SHELL);
    let wanted = shell_css(host, env, theme, true, &mut notes);
    let current = read_opt(host, &css).ok().flatten();
    let pre = current.as_deref().map(sha256_hex);
    if pre.is_some() && !state.owned.contains_key(&path_str(&css)) {
        return TargetPlan::skip(SHELL, Skip::ForeignFile(css));
    }
    if !inside_home(host, &host.dirs.data) {
        return TargetPlan::skip(SHELL, Skip::OutsideHome(css));
    }
    let mut plan = notes;
    // Velatura's own data folder is there by the time this runs; only the
    // folder for the stylesheet may have to be made.
    if let Some(dir) = css.parent().filter(|dir| !exists(host, dir)) {
        plan.changes.push(Change::CreateDir {
            path: dir.to_path_buf(),
        });
    }
    if current.as_deref() != Some(wanted.as_slice()) {
        plan.changes.push(Change::Owned {
            path: css.clone(),
            content: Some(wanted),
            pre,
        });
    }
    if clear_old {
        clear_generated(host, state, Some(&css), &mut plan);
        // The name Velatura gave User Themes points at a folder that is
        // going away; it goes back to what it was by default.
        for (id, written) in &state.settings {
            let Some((schema, "name")) = id.rsplit_once(' ') else {
                continue;
            };
            if !schema.starts_with(USER_THEMES_SCHEMA) {
                continue;
            }
            if let Ok(Some(now)) = host.settings.read(schema, "name")
                && now.user.is_some()
                && now.user == *written
                && now.writable
            {
                plan.changes.push(Change::Setting {
                    schema: schema.to_owned(),
                    key: "name".to_owned(),
                    value: None,
                    pre: now.user,
                });
            }
        }
    }
    plan
}

pub fn plan_appearance(host: &Host<'_>, theme: &Resolved<'_>, keep_style: bool) -> TargetPlan {
    let mut plan = TargetPlan::new(APPEARANCE);
    // Light is asked for outright: with `default` the Shell of a normal
    // session stays dark, under a stylesheet made from its light one.
    let scheme = match theme.colors.appearance {
        Appearance::Dark => "prefer-dark",
        Appearance::Light => "prefer-light",
    };
    let accent = nearest_accent(theme.colors.ui.accent);
    let wanted = [
        ("color-scheme", scheme),
        ("accent-color", accent.setting_value()),
    ];
    for (key, value) in wanted {
        if keep_style && key == "color-scheme" {
            continue;
        }
        match host.settings.read(INTERFACE_SCHEMA, key) {
            Err(SettingsError::Unavailable(why)) => {
                return TargetPlan::skip(APPEARANCE, Skip::SettingsUnavailable(why));
            }
            Err(e) => {
                return TargetPlan::skip(APPEARANCE, Skip::SettingsUnavailable(e.to_string()));
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

/// Plan one instance by its id.
pub fn plan(
    instance: &str,
    host: &Host<'_>,
    env: Env<'_>,
    theme: &Resolved<'_>,
    state: &State,
    opts: &crate::engine::ApplyOpts,
) -> TargetPlan {
    let replace_changed = opts.replace_changed;
    match instance {
        KITTY => plan_kitty(host, env, theme, state, replace_changed),
        GHOSTTY => plan_ghostty(host, env, theme, state, replace_changed),
        GTK4 => plan_gtk4(host, env, theme, state, replace_changed),
        GTK3 => plan_gtk3(host, env, theme, state, replace_changed),
        TMUX => plan_tmux(host, env, theme, state, replace_changed),
        NVIM => plan_nvim(host, env, theme, state, replace_changed),
        STARSHIP => plan_starship(host, env, theme, state),
        PTYXIS => plan_ptyxis(host, env, theme, state, replace_changed),
        TEXT_EDITOR => plan_text_editor(host, theme, state, replace_changed),
        FIREFOX => crate::firefox::plan_firefox(host, env, theme, state, replace_changed),
        ICONS => crate::icon_set::plan_icons(host, env, theme, state, !opts.unattended, opts.probe),
        CURSORS => {
            crate::cursor_set::plan_cursors(host, env, theme, state, !opts.unattended, opts.probe)
        }
        FLATPAK => plan_flatpak(host, env, state),
        OUTLINE => plan_outline(host, env, theme, state),
        TOP_BAR => plan_top_bar(host, env, theme, state),
        UPS => plan_ups(host, env, theme, state),
        TASKS => plan_tasks(host, env, theme, state),
        MEDIA => plan_media(host, env, theme, state),
        SHELL => plan_shell(
            host,
            env,
            theme,
            state,
            !opts.unattended,
            opts.untested_shell,
        ),
        APPEARANCE => plan_appearance(host, theme, opts.keep_style),
        // A part listed in INSTANCES with no arm here: planned as nothing,
        // and caught by the test that plans every part.
        other => TargetPlan::skip(other, Skip::NotSelected),
    }
}

/// Something else on this machine that changes the same colours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtherTool {
    /// A known theming tool has left its files here.
    Known { name: &'static str, path: PathBuf },
    /// A user timer with "theme" in its name is enabled.
    Timer { unit: String },
    /// A dotfile manager that may write over Velatura's changes.
    Dotfiles { name: &'static str, path: PathBuf },
    /// An enabled Shell extension keeps a colour in its own settings, set
    /// there rather than left at the extension's default, so it stays the
    /// same whatever theme is applied. `keys` are the settings that hold one.
    PinnedColors { name: String, keys: Vec<String> },
}

/// Look for other tools that theme the desktop or manage the same files.
/// Nothing is run. Files are looked for by name; of Shell extensions, the
/// ones turned on, their description and their settings are read.
pub fn other_tools(host: &Host<'_>) -> Vec<OtherTool> {
    let d = host.dirs;
    let known: [(&'static str, PathBuf); 7] = [
        (
            "Gradience",
            d.home.join(".var/app/com.github.GradienceTeam.Gradience"),
        ),
        ("Gradience", d.config.join("presets")),
        ("pywal", d.cache.join("wal")),
        ("matugen", d.config.join("matugen")),
        ("darkman", d.config.join("darkman")),
        ("darkman", d.data.join("dark-mode.d")),
        (
            "Night Theme Switcher",
            d.data
                .join("gnome-shell/extensions/nightthemeswitcher@romainvigier.fr"),
        ),
    ];
    let dotfiles: [(&'static str, PathBuf); 3] = [
        ("chezmoi", d.data.join("chezmoi")),
        ("yadm", d.data.join("yadm/repo.git")),
        ("Home Manager", d.config.join("home-manager")),
    ];
    let mut found = Vec::new();
    for (name, path) in known {
        if exists(host, &path) {
            found.push(OtherTool::Known { name, path });
        }
    }
    let timers = d.config.join("systemd/user/timers.target.wants");
    for unit in host.fs.list(&timers).unwrap_or_default() {
        let unit = unit.to_string_lossy().into_owned();
        if unit.to_lowercase().contains("theme") && !unit.starts_with("velatura") {
            found.push(OtherTool::Timer { unit });
        }
    }
    for (name, path) in dotfiles {
        if exists(host, &path) {
            found.push(OtherTool::Dotfiles { name, path });
        }
    }
    found.extend(pinned_colors(host));
    found
}

/// Enabled Shell extensions with a colour set in their own settings, as a
/// border or a highlight of their own. Only values that were set count: a
/// colour left at the extension's default may well follow the accent.
fn pinned_colors(host: &Host<'_>) -> Vec<OtherTool> {
    let Ok(Some(enabled)) = host.settings.read("org.gnome.shell", "enabled-extensions") else {
        return Vec::new();
    };
    let mut dirs = vec![host.dirs.data.clone()];
    dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from));
    let mut found = Vec::new();
    for uuid in listed(&enabled.effective) {
        if uuid == crate::companion::UUID || uuid.contains('/') {
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
        let Some(id) = meta
            .get("settings-schema")
            .and_then(serde_json::Value::as_str)
            .filter(|id| {
                id.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        else {
            continue;
        };
        let local = dir.join("schemas");
        let schema = if exists(host, &local.join("gschemas.compiled")) {
            format!("{id}@{}", local.display())
        } else {
            id.to_owned()
        };
        let mut keys: Vec<String> = host
            .settings
            .keys(&schema)
            .into_iter()
            .filter(|key| {
                matches!(host.settings.read(&schema, key),
                    Ok(Some(s)) if s.user.as_deref().is_some_and(is_color))
            })
            .collect();
        keys.sort();
        keys.truncate(8);
        if !keys.is_empty() {
            let name = meta
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(uuid)
                .to_owned();
            found.push(OtherTool::PinnedColors { name, keys });
        }
    }
    found
}

/// A setting's value that is a colour as extensions write them: `#rgb`,
/// `#rrggbb` (with alpha too), `rgb(…)` or `rgba(…)`.
fn is_color(value: &str) -> bool {
    let v = value.trim().trim_matches(['\'', '"']).trim();
    if let Some(hex) = v.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    let inner = v
        .strip_prefix("rgba(")
        .or_else(|| v.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'));
    inner.is_some_and(|args| {
        let parts: Vec<&str> = args.split(',').map(str::trim).collect();
        matches!(parts.len(), 3 | 4)
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'%')
            })
    })
}

#[cfg(test)]
mod tests {
    use super::{colors_shell, is_color};

    #[test]
    fn colours_are_told_from_other_values() {
        for yes in [
            "#8493d8",
            "#FFF",
            "#11223344",
            "rgba(132, 147, 216, 1)",
            "rgb(145,65,172)",
            "'#abcdef'",
        ] {
            assert!(is_color(yes), "{yes}");
        }
        for no in [
            "auto",
            "auto-solid",
            "",
            "#12345",
            "rgba(1, 2)",
            "rgb(a, b, c)",
            "4",
            "true",
        ] {
            assert!(!is_color(no), "{no}");
        }
    }

    #[test]
    fn only_rules_that_start_on_a_shell_surface_and_set_a_colour_count() {
        assert!(colors_shell(
            "#panel.kanagawa-panel { background-color: #0d101a; }"
        ));
        assert!(colors_shell(
            ".quick-settings .icon-button:hover, .x { color: red }"
        ));
        assert!(colors_shell("a{}\n.calendar { border: 1px solid #fff; }"));
        assert!(!colors_shell("#panel { height: 2em; }"), "no colour");
        assert!(!colors_shell(
            ".my-option #panel { background-color: red; }"
        ));
        assert!(!colors_shell("#panelish, .popup-menu-x { color: red; }"));
        assert!(!colors_shell(
            "/* #panel { color: red; } */ .widget { color: red; }"
        ));
        assert!(!colors_shell("#panel { color: red; "), "not closed");
    }

    /// A light theme asks for light outright: with `default` the Shell of a
    /// normal session stays dark.
    #[test]
    fn the_style_is_asked_for_outright_both_ways() {
        use crate::host::{Dirs, Host, MemSettings, StdFs};
        use crate::plan::Change;
        use velatura_theme::{Appearance, Resolved, bundled};
        fn env(k: &str) -> Option<std::ffi::OsString> {
            (k == "HOME").then(|| "/nonexistent".into())
        }
        let dirs = Dirs::from_env(env).unwrap();
        let settings = MemSettings::gnome();
        let host = Host {
            dirs: &dirs,
            fs: &StdFs,
            settings: &settings,
            now: 0,
        };
        let mut theme = bundled::themes().remove(0);
        for (appearance, wanted) in [
            (Appearance::Dark, "prefer-dark"),
            (Appearance::Light, "prefer-light"),
        ] {
            for v in theme.variants.values_mut() {
                v.appearance = appearance;
            }
            let plan = super::plan_appearance(&host, &Resolved::new(&theme, None).unwrap(), false);
            assert!(
                plan.changes.iter().any(|c| matches!(c,
                    Change::Setting { key, value: Some(v), .. }
                        if key == "color-scheme" && v == wanted)),
                "{wanted}: {:?}",
                plan.changes
            );
        }
    }
}
