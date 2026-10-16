//! Transaction tests for the targets added in phase 3a: tmux, Neovim and
//! Starship. Every one runs in a throw-away home folder.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::engine::{self, ApplyOpts, Report};
use velatura_engine::host::{Dirs, Faults, Fs, Host, MemSettings, Settings, StdFs};
use velatura_engine::plan::{Change, Note, Plan, Skip, TargetPlan};
use velatura_engine::store;
use velatura_engine::targets::{NVIM, STARSHIP, TMUX};
use velatura_theme::{Resolved, Theme, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

const TMUX_CONF: &str = "set -g mouse on\nset -g status-style \"fg=red\"\n";
const INIT_LUA: &str = "vim.o.number = true\n";
const NVIM_LINE: &str = "vim.api.nvim_create_autocmd(\"VimEnter\", { once = true, nested = true, command = \"colorscheme velatura\" })";
const STARSHIP_TOML: &str = "# The prompt.\nformat = \"[$directory](accent)$character\"\n\n[character]\nsuccess_symbol = \"[>](bold green)\"";

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    /// A home with nothing in it but the folders every home has.
    fn empty() -> Self {
        Self::with(&[])
    }

    fn with(env: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-cli-apps-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let dirs = Dirs::from_env(|k| {
            if k == "HOME" {
                return Some(OsString::from(&home));
            }
            env.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, rel)| OsString::from(home.join(rel)))
        })
        .unwrap();
        for dir in [".local/state", ".local/share", ".config"] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        Self { root, dirs }
    }

    /// A home where the three apps have settings of the user's.
    fn full() -> Self {
        let h = Self::empty();
        h.write(".config/tmux/tmux.conf", TMUX_CONF);
        h.write(".config/nvim/init.lua", INIT_LUA);
        h.write(".config/starship.toml", STARSHIP_TOML);
        h
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dirs.home.join(rel)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn host<'a>(&'a self, fs: &'a dyn Fs, settings: &'a dyn Settings) -> Host<'a> {
        Host {
            dirs: &self.dirs,
            fs,
            settings,
            now: 1_800_000_000,
        }
    }

    /// Every file and folder outside Velatura's own store.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if rel.starts_with(".local/") {
                    continue;
                }
                if path.is_dir() {
                    out.insert(format!("{rel}/"), Vec::new());
                    walk(&path, base, out);
                } else {
                    out.insert(rel, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.dirs.home, &self.dirs.home, &mut out);
        out
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

const APPS: [&str; 3] = [TMUX, NVIM, STARSHIP];

fn opts() -> ApplyOpts {
    ApplyOpts {
        only: APPS.iter().map(|s| (*s).to_owned()).collect(),
        ..ApplyOpts::default()
    }
}

fn plan_with(host: &Host<'_>, theme: &Theme, env: &dyn Fn(&str) -> Option<OsString>) -> Plan {
    let resolved = Resolved::new(theme, None).unwrap();
    engine::plan_apply(host, env, &resolved, &opts()).unwrap()
}

fn plan(host: &Host<'_>, theme: &Theme) -> Plan {
    plan_with(host, theme, &no_env)
}

fn apply(host: &Host<'_>, theme: &Theme) -> Report {
    engine::execute(host, &plan(host, theme), &opts().only).unwrap()
}

fn undo(host: &Host<'_>) -> Report {
    engine::execute(host, &engine::plan_undo(host).unwrap(), &[]).unwrap()
}

fn target<'a>(p: &'a Plan, instance: &str) -> &'a TargetPlan {
    p.targets.iter().find(|t| t.instance == instance).unwrap()
}

#[test]
fn all_three_apply_twice_and_undo_to_the_exact_start() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let start = home.snapshot();

    let p = plan(&host, &t[1]);
    // Two add to a file the user keeps; Neovim gets files of Velatura's
    // own, in a folder the user keeps. With no one there to agree, as in
    // a scheduled switch, none of the three is touched.
    assert_eq!(p.needing_consent(), [TMUX, NVIM, STARSHIP]);
    let alone = engine::execute(&host, &p, &[]).unwrap();
    assert!(alone.changes.is_empty(), "{:?}", alone.changes);
    assert_eq!(home.snapshot(), start);
    let report = engine::execute(&host, &p, &opts().only).unwrap();
    assert!(
        report
            .skipped
            .iter()
            .all(|(_, why)| *why == Skip::NotSelected),
        "{:?}",
        report.skipped
    );

    assert!(
        home.read(".config/tmux/velatura.conf")
            .contains("set -g pane-active-border-style \"fg=#e0914f\"")
    );
    assert_eq!(
        home.read(".config/tmux/tmux.conf"),
        format!(
            "{TMUX_CONF}# >>> velatura tmux 1 >>>\nsource-file -q ~/.config/tmux/velatura.conf\n# <<< velatura tmux 1 <<<\n"
        )
    );
    assert!(
        home.read(".config/nvim/colors/velatura.lua")
            .contains("hl(0, \"Normal\", { fg = \"#dbc7b0\", bg = \"#191316\" })")
    );
    assert!(
        home.read(".config/nvim/lua/velatura/palette.lua")
            .contains("  accent = \"#e0914f\",")
    );
    // The settings choose no colourscheme, so one line chooses Velatura's.
    assert_eq!(
        home.read(".config/nvim/init.lua"),
        format!("{INIT_LUA}-- >>> velatura nvim 1 >>>\n{NVIM_LINE}\n-- <<< velatura nvim 1 <<<\n")
    );
    let starship = home.read(".config/starship.toml");
    assert!(starship.starts_with(
        "# >>> velatura starship 1 >>>\npalette = \"velatura\"\npalettes.velatura.bg = \"#191316\"\n"
    ));
    assert!(starship.ends_with(&format!(
        "palettes.velatura.language = \"#9fa872\"\n# <<< velatura starship 1 <<<\n{STARSHIP_TOML}"
    )));
    // What Starship reads is the user's settings plus the palette.
    let read: toml::Table = starship.parse().unwrap();
    assert_eq!(read["palette"].as_str(), Some("velatura"));
    assert_eq!(
        read["format"].as_str(),
        Some("[$directory](accent)$character")
    );
    assert_eq!(
        read["palettes"]["velatura"]["accent"].as_str(),
        Some("#e0914f")
    );
    assert!(read["character"].as_table().unwrap().len() == 1);

    for (instance, note) in [
        (
            TMUX,
            Note::TmuxReload {
                conf: home.path(".config/tmux/tmux.conf"),
            },
        ),
        (
            NVIM,
            Note::NeovimOtherScheme {
                name: None,
                path: home.path(".config/nvim/init.lua"),
            },
        ),
        (NVIM, Note::NeovimReload),
        (STARSHIP, Note::StarshipNames),
    ] {
        assert!(
            report.notes.contains(&(instance.to_owned(), note.clone())),
            "{note:?}"
        );
    }

    // The same theme again changes nothing.
    assert!(plan(&host, &t[1]).is_empty());

    // Another theme rewrites the three colour files and the palette where
    // it stands; nothing new is asked.
    let p = plan(&host, &t[0]);
    assert!(p.needing_consent().is_empty());
    let second = engine::execute(&host, &p, &opts().only).unwrap();
    assert_eq!(second.changes.len(), 4, "{:?}", second.changes);
    assert!(
        home.read(".config/starship.toml")
            .starts_with("# >>> velatura starship 1 >>>\npalette = \"velatura\"\npalettes.velatura.bg = \"#141826\"\n")
    );

    undo(&host);
    undo(&host);
    assert_eq!(home.snapshot(), start);
}

#[test]
fn exhaustive_crash_test_over_the_three() {
    let t = bundled::themes();
    for start_applied in [false, true] {
        let mut crash_points = 0;
        for fail_at in 0.. {
            let home = Home::full();
            let settings = MemSettings::gnome();
            let clean = home.host(&StdFs, &settings);
            if start_applied {
                apply(&clean, &t[0]);
            } else {
                store::ensure_store(&clean).unwrap();
            }
            let before = home.snapshot();
            let p = plan(&clean, &t[1]);

            let faults = Faults::new(&StdFs, &settings, fail_at);
            let dying = home.host(&faults, &faults);
            if engine::execute(&dying, &p, &opts().only).is_ok() {
                break;
            }
            crash_points += 1;
            if let Some(r) = engine::recover(&clean).unwrap() {
                assert!(r.is_clean() && r.kept.is_empty(), "step {fail_at}: {r:?}");
            }
            assert_eq!(
                home.snapshot(),
                before,
                "state differs after a crash at step {fail_at} (already applied: {start_applied})"
            );
        }
        assert!(
            crash_points >= 4,
            "only {crash_points} crash points exercised"
        );
    }
}

#[test]
fn what_the_user_wrote_since_is_kept_by_undo() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);

    let tmux = home.read(".config/tmux/tmux.conf");
    home.write(
        ".config/tmux/tmux.conf",
        &format!("# mine\n{tmux}bind r source-file x\n"),
    );
    let starship = home.read(".config/starship.toml");
    home.write(
        ".config/starship.toml",
        &format!("{starship}\n\n[git_branch]\nstyle = \"git\"\n"),
    );
    undo(&host);
    assert_eq!(
        home.read(".config/tmux/tmux.conf"),
        format!("# mine\n{TMUX_CONF}bind r source-file x\n")
    );
    assert_eq!(
        home.read(".config/starship.toml"),
        format!("{STARSHIP_TOML}\n\n[git_branch]\nstyle = \"git\"\n")
    );
    assert!(!home.path(".config/nvim/colors").exists());
    assert!(!home.path(".config/nvim/lua").exists());
}

#[test]
fn apps_that_are_not_there_are_left_out_and_found_by_their_program() {
    let t = bundled::themes();
    let home = Home::empty();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let p = plan(&host, &t[0]);
    for app in APPS {
        assert_eq!(target(&p, app).skipped, Some(Skip::NotInstalled), "{app}");
    }

    // On the path, with no settings yet: the files are made from nothing.
    for program in ["tmux", "nvim", "starship"] {
        home.write(&format!("bin/{program}"), "");
    }
    let bin = home.path("bin");
    let env = move |k: &str| (k == "PATH").then(|| OsString::from(&bin));
    let start = home.snapshot();
    let p = plan_with(&host, &t[0], &env);
    assert!(
        p.needing_consent().is_empty(),
        "nothing of the user's is there"
    );
    engine::execute(&host, &p, &opts().only).unwrap();
    assert_eq!(
        home.read(".config/tmux/tmux.conf"),
        "# >>> velatura tmux 1 >>>\nsource-file -q ~/.config/tmux/velatura.conf\n# <<< velatura tmux 1 <<<\n"
    );
    assert!(home.path(".config/nvim/colors/velatura.lua").exists());
    assert!(
        home.read(".config/starship.toml")
            .ends_with("# <<< velatura starship 1 <<<\n")
    );
    undo(&host);
    assert_eq!(home.snapshot(), start);
}

#[test]
fn tmux_takes_the_file_that_has_the_last_word_and_reports_what_wins_over_it() {
    let t = bundled::themes();
    let home = Home::empty();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    // Only the old place: the block goes there, the colours into the
    // settings folder.
    home.write(".tmux.conf", "set -g mouse on\n");
    apply(&host, &t[0]);
    assert!(
        home.read(".tmux.conf")
            .contains("source-file -q ~/.config/tmux/velatura.conf")
    );
    assert!(home.path(".config/tmux/velatura.conf").exists());
    assert!(!home.path(".config/tmux/tmux.conf").exists());
    undo(&host);
    assert_eq!(home.read(".tmux.conf"), "set -g mouse on\n");
    assert!(!home.path(".config/tmux").exists());

    // Both: tmux reads the one in the settings folder last.
    home.write(".config/tmux/tmux.conf", "");
    apply(&host, &t[0]);
    assert!(!home.read(".tmux.conf").contains("velatura"));
    let mut conf = home.read(".config/tmux/tmux.conf");
    conf.push_str("set -g mouse off\nset -g status-style \"fg=red\"\nbind x kill-pane\nsetw -g mode-style bg=blue\n");
    home.write(".config/tmux/tmux.conf", &conf);
    let p = plan(&host, &t[0]);
    let lines = target(&p, TMUX).notes.iter().find_map(|n| match n {
        Note::Override { lines, .. } => Some(lines.clone()),
        _ => None,
    });
    assert_eq!(lines, Some(vec![5, 7]));
}

#[test]
fn a_settings_folder_tmux_could_misread_is_quoted_or_left_out() {
    let t = bundled::themes();
    let settings = MemSettings::gnome();
    let home = Home::with(&[("XDG_CONFIG_HOME", "my settings $HOME")]);
    home.write("my settings $HOME/tmux/tmux.conf", "");
    let host = home.host(&StdFs, &settings);
    apply(&host, &t[0]);
    let owned = home.path("my settings $HOME/tmux/velatura.conf");
    assert!(
        home.read("my settings $HOME/tmux/tmux.conf")
            .contains(&format!("source-file -q '{}'\n", owned.display()))
    );

    let home = Home::with(&[("XDG_CONFIG_HOME", "it's")]);
    home.write("it's/tmux/tmux.conf", "");
    let p = plan(&home.host(&StdFs, &settings), &t[0]);
    assert_eq!(
        target(&p, TMUX).skipped,
        Some(Skip::UnusualPath(home.path("it's/tmux/velatura.conf")))
    );
}

#[test]
fn neovim_files_that_are_not_velaturas_are_not_replaced() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    home.write(".config/nvim/colors/velatura.lua", "-- mine\n");
    let p = plan(&host, &t[0]);
    assert_eq!(
        target(&p, NVIM).skipped,
        Some(Skip::ForeignFile(
            home.path(".config/nvim/colors/velatura.lua")
        ))
    );
    fs::remove_file(home.path(".config/nvim/colors/velatura.lua")).unwrap();

    // The user's own folders stay when Velatura's files leave them.
    home.write(".config/nvim/lua/mine/init.lua", "return {}\n");
    apply(&host, &t[0]);
    // A file of Velatura's that was edited is not written over unasked.
    home.write(".config/nvim/lua/velatura/palette.lua", "return {}\n");
    let p = plan(&host, &t[1]);
    assert_eq!(
        target(&p, NVIM).skipped,
        Some(Skip::ChangedOutside(
            home.path(".config/nvim/lua/velatura/palette.lua")
        ))
    );
    let report = undo(&host);
    assert!(
        home.path(".config/nvim/colors").exists(),
        "it was the user's"
    );
    assert!(!home.path(".config/nvim/colors/velatura.lua").exists());
    assert!(home.path(".config/nvim/lua/mine/init.lua").exists());
    assert_eq!(
        home.read(".config/nvim/lua/velatura/palette.lua"),
        "return {}\n"
    );
    assert!(
        report
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::KeptFile { .. }))
    );
}

#[test]
fn starship_settings_that_choose_a_palette_or_do_not_read_are_left_alone() {
    let t = bundled::themes();
    let home = Home::empty();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let file = home.path(".config/starship.toml");
    let cases = [
        (
            "format = \"$all\"\npalette = \"mine\"\n[palettes.mine]\nx = \"#000000\"\n",
            Skip::OwnPalette(file.clone()),
        ),
        (
            "[palettes.velatura]\naccent = \"#000000\"\n",
            Skip::OwnPalette(file.clone()),
        ),
        (
            "[palettes]\nmine = { x = \"#000000\" }\n",
            Skip::OwnPalette(file.clone()),
        ),
        ("format = \"unfinished\n", Skip::Invalid(file.clone())),
        ("\u{ff}\u{fe}", Skip::Invalid(file.clone())),
    ];
    for (content, why) in cases {
        fs::write(&file, content.as_bytes()).unwrap();
        let p = plan(&host, &t[0]);
        assert_eq!(target(&p, STARSHIP).skipped, Some(why), "{content:?}");
        assert!(p.is_empty());
    }
    fs::write(&file, b"\xff\xfe not text").unwrap();
    assert_eq!(
        target(&plan(&host, &t[0]), STARSHIP).skipped,
        Some(Skip::Invalid(file.clone()))
    );

    // Other palettes that are only defined, not chosen, are no obstacle.
    home.write(
        ".config/starship.toml",
        "[palettes.mine]\nx = \"#000000\"\n",
    );
    let p = plan(&host, &t[0]);
    assert!(target(&p, STARSHIP).skipped.is_none());
    assert!(matches!(
        target(&p, STARSHIP).changes.as_slice(),
        [Change::Block { target, .. }] if target == "starship"
    ));

    // Once the block is in, a palette chosen below it collides with it.
    apply(&host, &t[0]);
    let mut now = home.read(".config/starship.toml");
    now = now.replace("[palettes.mine]", "palette = \"mine\"\n[palettes.mine]");
    home.write(".config/starship.toml", &now);
    assert_eq!(
        target(&plan(&host, &t[1]), STARSHIP).skipped,
        Some(Skip::Invalid(file))
    );
}

#[test]
fn starship_settings_are_found_where_the_environment_says() {
    let t = bundled::themes();
    let home = Home::empty();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    home.write("dotfiles/prompt.toml", "add_newline = false\n");
    home.write(".config/starship.toml", "add_newline = true\n");
    let named = home.path("dotfiles/prompt.toml");
    let env = move |k: &str| (k == "STARSHIP_CONFIG").then(|| OsString::from(&named));
    let p = plan_with(&host, &t[0], &env);
    engine::execute(&host, &p, &opts().only).unwrap();
    assert!(
        home.read("dotfiles/prompt.toml")
            .contains("palettes.velatura.accent")
    );
    assert_eq!(home.read(".config/starship.toml"), "add_newline = true\n");

    let outside = home.root.join("elsewhere/starship.toml");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::write(&outside, "").unwrap();
    let there = outside.clone();
    let env = move |k: &str| (k == "STARSHIP_CONFIG").then(|| OsString::from(&there));
    let p = plan_with(&host, &t[0], &env);
    assert_eq!(
        target(&p, STARSHIP).skipped,
        Some(Skip::OutsideHome(outside))
    );
}

#[test]
fn neovim_settings_that_choose_another_colorscheme_get_a_line_asked_for() {
    let t = bundled::themes();
    let home = Home::full();
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    // Settings that choose Velatura's colourscheme themselves are left as
    // they are; Neovim is agreed to with that first apply.
    home.write(
        ".config/nvim/lua/me/scheme.lua",
        "-- a colorscheme is set here\nvim.cmd.colorscheme(\"velatura\")\n",
    );
    let start = home.snapshot();
    let p = plan(&host, &t[1]);
    assert!(
        !target(&p, NVIM)
            .notes
            .iter()
            .any(|n| matches!(n, Note::NeovimOtherScheme { .. }))
    );
    engine::execute(&host, &p, &opts().only).unwrap();
    assert_eq!(home.read(".config/nvim/init.lua"), INIT_LUA);

    // Now they choose another, as a plugin's settings do. The ColorScheme
    // event, comments and a list of schemes to install are not choices.
    home.write(
        ".config/nvim/lua/me/scheme.lua",
        "require(\"lazy\").setup({ install = { colorscheme = { \"habamax\" } } })\n\
         vim.api.nvim_create_autocmd(\"ColorScheme\", { callback = f }) -- the colorscheme is set below\n\
         vim.cmd(\"colorscheme kanagawa-wave\")\n",
    );
    let p = plan(&host, &t[0]);
    let nvim = target(&p, NVIM);
    assert!(
        nvim.notes.contains(&Note::NeovimOtherScheme {
            name: Some("kanagawa-wave".to_owned()),
            path: home.path(".config/nvim/init.lua"),
        }),
        "{:?}",
        nvim.notes
    );
    // Agreed to before, but a line in the user's file is asked for anew;
    // with no one there to agree, Neovim is left alone and the rest goes on.
    assert_eq!(p.needing_consent(), [NVIM]);
    let colors = home.read(".config/nvim/colors/velatura.lua");
    let alone = engine::execute(&host, &p, &[]).unwrap();
    assert!(!alone.changes.is_empty());
    assert_eq!(home.read(".config/nvim/init.lua"), INIT_LUA);
    assert_eq!(home.read(".config/nvim/colors/velatura.lua"), colors);
    let p = plan(&host, &t[0]);
    assert_eq!(p.needing_consent(), [NVIM]);
    engine::execute(&host, &p, &opts().only).unwrap();
    assert!(
        home.read(".config/nvim/init.lua")
            .ends_with(&format!("{NVIM_LINE}\n-- <<< velatura nvim 1 <<<\n"))
    );
    // The line it added is not read as the user's own choice: the next
    // theme changes the colours and asks nothing.
    let p = plan(&host, &t[1]);
    assert!(p.needing_consent().is_empty());
    assert!(
        target(&p, NVIM)
            .changes
            .iter()
            .all(|c| !matches!(c, Change::Block { .. }))
    );

    undo(&host);
    undo(&host);
    undo(&host);
    home.write(
        ".config/nvim/lua/me/scheme.lua",
        "-- a colorscheme is set here\nvim.cmd.colorscheme(\"velatura\")\n",
    );
    assert_eq!(home.snapshot(), start);
}

#[test]
fn neovim_settings_in_vim_script_get_a_vim_line() {
    let t = bundled::themes();
    let home = Home::full();
    fs::remove_file(home.path(".config/nvim/init.lua")).unwrap();
    home.write(".config/nvim/init.vim", "colorscheme desert\n");
    let settings = MemSettings::gnome();
    let host = home.host(&StdFs, &settings);
    let p = plan(&host, &t[0]);
    assert!(target(&p, NVIM).notes.contains(&Note::NeovimOtherScheme {
        name: Some("desert".to_owned()),
        path: home.path(".config/nvim/init.vim"),
    }));
    engine::execute(&host, &p, &opts().only).unwrap();
    assert_eq!(
        home.read(".config/nvim/init.vim"),
        "colorscheme desert\n\" >>> velatura nvim-vim 1 >>>\nautocmd VimEnter * ++once ++nested colorscheme velatura\n\" <<< velatura nvim-vim 1 <<<\n"
    );
    assert!(!home.path(".config/nvim/init.lua").exists());
}
