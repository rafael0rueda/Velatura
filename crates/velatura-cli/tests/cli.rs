//! End-to-end test of the command against a real settings store.
//!
//! Runs the built binary in a throw-away home folder, inside a private D-Bus
//! session with its own dconf database, so nothing of the machine running
//! the test is read or changed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        Self::named("main")
    }

    fn named(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("velatura-cli-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "home/.config/kitty",
            "home/.local/share",
            "home/.local/state",
            "run",
            "bin",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The machine's programs, after the sandbox's own stand-ins.
    fn path_var(&self) -> std::ffi::OsString {
        let rest = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(
            std::iter::once(self.root.join("bin")).chain(std::env::split_paths(&rest)),
        )
        .unwrap()
    }

    fn run(&self, program: &Path, args: &[&str]) -> Output {
        Command::new("dbus-run-session")
            .arg("--")
            .arg(program)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", self.path_var())
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("GSETTINGS_BACKEND", "dconf")
            .env("LC_ALL", "C.UTF-8")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    /// The command as run from a text console: no session bus at all.
    fn velatura_without_session(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_velatura"))
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C.UTF-8")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn velatura(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.run(Path::new(env!("CARGO_BIN_EXE_velatura")), args);
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn gsetting(&self, key: &str) -> String {
        let out = self.run(
            Path::new("gsettings"),
            &["get", "org.gnome.desktop.interface", key],
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[test]
fn apply_and_undo_against_a_private_settings_store() {
    if !(have("dbus-run-session") && have("gsettings")) {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "dbus-run-session and gsettings are required in CI"
        );
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::new();
    let conf = sb.home().join(".config/kitty/kitty.conf");
    let owned = sb.home().join(".config/kitty/velatura.conf");
    let original = "font_size 12\nbackground_opacity 0.9";
    fs::write(&conf, original).unwrap();
    assert_eq!(
        sb.gsetting("accent-color"),
        "'blue'",
        "sandbox starts at defaults"
    );

    let (code, out, _) = sb.velatura(&["list"]);
    assert_eq!(code, 0);
    assert!(out.contains("aetherius") && out.contains("ember"), "{out}");

    // Every part the engine knows is listed, under the name --only takes.
    let (code, out, _) = sb.velatura(&["targets"]);
    assert_eq!(code, 0);
    for instance in velatura_engine::targets::INSTANCES {
        let id = instance.strip_suffix(":default").unwrap_or(instance);
        assert!(
            out.lines().any(|l| l.split_whitespace().next() == Some(id)),
            "{id} missing from:\n{out}"
        );
    }

    // Preview changes nothing.
    let (code, out, _) = sb.velatura(&["apply", "ember", "--preview"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Add a marked block with one line"), "{out}");
    assert!(out.contains("Set Accent Color to Orange"), "{out}");
    assert_eq!(fs::read_to_string(&conf).unwrap(), original);
    assert!(!owned.exists());

    // No terminal and no --yes: nothing at all is changed.
    let (code, _, err) = sb.velatura(&["apply", "ember"]);
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("Nothing was changed"), "{err}");
    assert_eq!(fs::read_to_string(&conf).unwrap(), original);
    assert_eq!(sb.gsetting("accent-color"), "'blue'");

    let (code, out, err) = sb.velatura(&["apply", "ember", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Applied ember/default."), "{out}");
    assert!(out.contains("To undo: velatura undo"), "{out}");
    assert!(
        fs::read_to_string(&owned)
            .unwrap()
            .contains("background #191316")
    );
    assert!(fs::read_to_string(&conf).unwrap().ends_with(
        "background_opacity 0.9\n# >>> velatura kitty 1 >>>\ninclude velatura.conf\n# <<< velatura kitty 1 <<<\n"
    ));
    assert_eq!(sb.gsetting("accent-color"), "'orange'");
    assert_eq!(sb.gsetting("color-scheme"), "'prefer-dark'");

    let (code, out, _) = sb.velatura(&["status"]);
    assert_eq!(code, 0);
    assert!(out.contains("Theme in use: ember/default"), "{out}");
    assert!(!out.contains("changed outside"), "{out}");

    let (code, out, _) = sb.velatura(&["apply", "ember", "--yes"]);
    assert_eq!(code, 0);
    assert!(out.contains("Nothing to change."), "{out}");

    let (code, out, _) = sb.velatura(&["apply", "aetherius", "--json"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("\"format\": 1") && out.contains("\"backup\": 2"),
        "{out}"
    );
    assert_eq!(sb.gsetting("accent-color"), "'blue'");

    let (code, out, _) = sb.velatura(&["history"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 2, "{out}");

    // Back to before the first backup: two undo steps.
    let (code, out, err) = sb.velatura(&["restore", "1"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(fs::read_to_string(&conf).unwrap(), original);
    assert!(!owned.exists());
    assert_eq!(sb.gsetting("accent-color"), "'blue'");
    assert_eq!(sb.gsetting("color-scheme"), "'default'");
    let (code, _, err) = sb.velatura(&["undo"]);
    assert_eq!(code, 1);
    assert!(err.contains("nothing to undo"), "{err}");

    // Apply again, then remove everything in one step.
    assert_eq!(sb.velatura(&["apply", "ember", "--yes"]).0, 0);
    let (code, out, _) = sb.velatura(&["restore", "--original"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(fs::read_to_string(&conf).unwrap(), original);
    assert_eq!(sb.gsetting("color-scheme"), "'default'");

    // From a text console with no desktop session: files are put back, and
    // the settings that could not be are listed with the command to run.
    assert_eq!(sb.velatura(&["apply", "ember", "--yes"]).0, 0);
    let (code, out, err) = sb.velatura_without_session(&["undo"]);
    assert_eq!(code, 5, "{out}{err}");
    assert_eq!(fs::read_to_string(&conf).unwrap(), original);
    assert!(!owned.exists());
    assert!(
        out.contains("run: gsettings reset org.gnome.desktop.interface accent-color"),
        "{out}"
    );
    assert!(
        !out.contains("Set Accent Color"),
        "nothing is claimed that did not happen: {out}"
    );
    assert_eq!(sb.gsetting("accent-color"), "'orange'", "truly unchanged");
    // The same from the desktop, and a theme can be applied again after.
    let (code, out, _) = sb.velatura_without_session(&["apply", "ember", "--yes", "--preview"]);
    assert_eq!(code, 0);
    assert!(out.contains("desktop settings are not available"), "{out}");

    // Usage errors and an invalid theme file.
    assert_eq!(sb.velatura(&["apply", "no-such-theme"]).0, 2);
    assert_eq!(sb.velatura(&["frobnicate"]).0, 2);
    let bad = sb.root.join("bad.toml");
    fs::write(&bad, "schema = 1\n[meta]\nid = \"Bad Id\"\n").unwrap();
    let (code, _, err) = sb.velatura(&["validate", bad.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("meta.id"), "{err}");
    let (code, out, _) = sb.velatura(&["--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("From a text console") || out.contains("from a text console"),
        "{out}"
    );
}

/// The schedule from the command: set up, run as the timer runs it, and
/// taken away again. systemctl is a stand-in that writes down its orders.
#[test]
fn schedule_and_the_switch_with_no_one_present() {
    if !(have("dbus-run-session") && have("gsettings")) {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "dbus-run-session and gsettings are required in CI"
        );
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::named("schedule");
    let conf = sb.home().join(".config/kitty/kitty.conf");
    fs::write(&conf, "font_size 12\n").unwrap();
    let log = sb.root.join("systemctl.log");
    let fake = sb.root.join("bin/systemctl");
    fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let units = sb.home().join(".config/systemd/user");

    assert_eq!(sb.velatura(&["switch", "set", "day", "ember"]).0, 0);
    assert_eq!(sb.velatura(&["switch", "set", "night", "aetherius"]).0, 0);
    let (code, out, _) = sb.velatura(&["schedule"]);
    assert_eq!(code, 0);
    assert!(out.contains("Scheduled switching is off."), "{out}");

    // A copy in a build folder is no place for a timer to look.
    let (code, _, err) = sb.velatura(&["schedule", "set"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("installed where it stays"), "{err}");
    assert!(!units.exists() && !log.exists());

    // Installed, the way scripts/install-local.sh does it.
    let installed = sb.home().join(".local/bin/velatura");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_velatura"), &installed).unwrap();
    let run = |args: &[&str]| {
        let out = sb.run(&installed, args);
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, _, err) = run(&["schedule", "set", "--day", "25:00"]);
    assert_eq!(code, 2);
    assert!(err.contains("is not a time"), "{err}");
    let (code, out, _) = run(&["schedule", "set", "--preview"]);
    assert_eq!(code, 0);
    assert!(out.contains("Would write") && !units.exists(), "{out}");

    let (code, out, err) = run(&["schedule", "set", "--day", "06:30", "--night", "21:30"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("the daytime theme from 06:30, the nighttime theme from 21:30"),
        "{out}"
    );
    let service = fs::read_to_string(units.join("velatura-switch.service")).unwrap();
    assert!(
        service.contains(&format!(
            "ExecStart=\"{}\" switch auto",
            installed.display()
        )),
        "{service}"
    );
    assert!(
        fs::read_to_string(units.join("velatura-switch.timer"))
            .unwrap()
            .contains("OnCalendar=*-*-* 21:30:00")
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "--user daemon-reload\n--user enable velatura-switch.timer\n--user restart velatura-switch.timer\n"
    );
    let (_, out, _) = run(&["status"]);
    assert!(out.contains("Scheduled switching is on"), "{out}");
    let (_, out, _) = run(&["schedule", "--json"]);
    assert!(
        out.contains("\"on\": true") && out.contains("\"night_at\": \"21:30\""),
        "{out}"
    );

    // As the timer runs it. Which theme depends on the time of the test;
    // either way Kitty's file is not touched, because nobody agreed to it.
    let (code, out, err) = run(&["switch", "auto"]);
    assert_eq!(code, 5, "{out}{err}");
    assert!(out.contains("Applied ember/default.") || out.contains("Applied aetherius/default."));
    assert!(
        // Kitty first; after it, other apps this machine has that ask
        // before their first change (Ptyxis, Text Editor).
        out.contains("The last scheduled switch left out: Kitty"),
        "{out}"
    );
    assert_eq!(fs::read_to_string(&conf).unwrap(), "font_size 12\n");
    assert_eq!(sb.gsetting("color-scheme"), "'prefer-dark'");
    let (_, out, _) = run(&["status"]);
    assert!(
        // Kitty first; after it, other apps this machine has that ask
        // before their first change (Ptyxis, Text Editor).
        out.contains("The last scheduled switch left out: Kitty"),
        "{out}"
    );
    let (_, out, _) = run(&["history"]);
    assert!(out.contains("scheduled"), "{out}");

    // Once per part of the day.
    let (code, out, _) = run(&["switch", "auto"]);
    assert_eq!(code, 0);
    assert!(out.contains("Nothing to do"), "{out}");
    let (code, out, _) = run(&["switch", "auto", "--again"]);
    assert_eq!(code, 5, "{out}");
    assert!(out.contains("is already applied"), "{out}");

    // Undo reverses the schedule's switch too: nothing is left applied.
    let (code, out, err) = run(&["undo"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(sb.gsetting("color-scheme"), "'default'");
    assert!(run(&["status"]).1.contains("No theme applied."));

    // Removing everything takes the timer with it.
    assert_eq!(run(&["apply", "ember", "--yes"]).0, 0);
    let (code, out, err) = run(&["restore", "--original"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Scheduled switching is off."), "{out}");
    assert!(
        !units.exists(),
        "the folders made for the timer are gone too"
    );
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("--user disable --now velatura-switch.timer\n")
    );
    assert_eq!(fs::read_to_string(&conf).unwrap(), "font_size 12\n");
    let (code, out, _) = run(&["schedule", "off"]);
    assert_eq!(code, 0);
    assert!(out.contains("was not on"), "{out}");
}

/// Icons and fonts chosen from the command, and the Flatpak overrides.
#[test]
fn look_and_flatpak_from_the_command() {
    if !(have("dbus-run-session") && have("gsettings")) {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "dbus-run-session and gsettings are required in CI"
        );
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::named("look");
    // A theme of the user's own, so the test does not depend on what the
    // machine has installed.
    let theme = sb.home().join(".local/share/icons/Mine");
    fs::create_dir_all(&theme).unwrap();
    fs::write(
        theme.join("index.theme"),
        "[Icon Theme]\nName=My Icons\nDirectories=16x16/apps\n",
    )
    .unwrap();

    let (code, out, _) = sb.velatura(&["look"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Icons:") && out.contains("Monospace Font:"),
        "{out}"
    );
    let (code, out, _) = sb.velatura(&["look", "list", "icons"]);
    assert_eq!(code, 0);
    assert!(
        out.lines()
            .any(|l| l.starts_with("Mine") && l.contains("My Icons")),
        "{out}"
    );

    let (code, _, err) = sb.velatura(&["look", "set", "icons", "../Mine"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("No installed theme has that name"), "{err}");
    assert_eq!(sb.velatura(&["look", "set", "colors", "x"]).0, 2);
    let (code, out, _) = sb.velatura(&["look", "set", "icons", "Mine", "--preview"]);
    assert_eq!(code, 0);
    assert!(out.contains("Set Icons to Mine"), "{out}");
    assert_ne!(sb.gsetting("icon-theme"), "'Mine'");

    let (code, out, err) = sb.velatura(&["look", "set", "icons", "My Icons"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("Changed.") && out.contains("To undo: velatura undo"),
        "{out}"
    );
    assert_eq!(sb.gsetting("icon-theme"), "'Mine'");
    let (code, _, err) = sb.velatura(&["look", "set", "monospace-font", "Fira Code"]);
    assert_eq!(code, 2);
    assert!(err.contains("its name and a size"), "{err}");
    assert_eq!(
        sb.velatura(&["look", "set", "monospace-font", "Fira Code 12"])
            .0,
        0
    );
    assert_eq!(sb.gsetting("monospace-font-name"), "'Fira Code 12'");
    let (_, out, _) = sb.velatura(&["history"]);
    assert_eq!(out.matches("select").count(), 2, "{out}");

    assert_eq!(sb.velatura(&["undo"]).0, 0);
    assert_ne!(sb.gsetting("monospace-font-name"), "'Fira Code 12'");
    assert_eq!(sb.velatura(&["look", "reset", "icons"]).0, 0);
    assert_ne!(sb.gsetting("icon-theme"), "'Mine'");

    // Flatpak: named outright, with agreement, and undone to the byte.
    let overrides = sb.home().join(".local/share/flatpak/overrides/global");
    fs::create_dir_all(overrides.parent().unwrap()).unwrap();
    let original = "[Context]\nfilesystems=home;\n";
    fs::write(&overrides, original).unwrap();
    let (code, out, _) = sb.velatura(&["apply", "ember", "--only", "flatpak", "--preview"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Let Flatpak apps read the two GTK settings folders"),
        "{out}"
    );
    assert_eq!(sb.velatura(&["apply", "ember", "--only", "flatpak"]).0, 3);
    assert_eq!(fs::read_to_string(&overrides).unwrap(), original);
    assert_eq!(
        sb.velatura(&["apply", "ember", "--only", "flatpak", "--yes"])
            .0,
        0
    );
    assert_eq!(
        fs::read_to_string(&overrides).unwrap(),
        "[Context]\nfilesystems=home;xdg-config/gtk-3.0:ro;xdg-config/gtk-4.0:ro;\n"
    );
    // Left out of an ordinary apply until turned on.
    let (_, out, _) = sb.velatura(&["status"]);
    assert!(
        out.contains("Flatpak Apps: themed, and left out of the next apply"),
        "{out}"
    );
    assert_eq!(sb.velatura(&["undo"]).0, 0);
    assert_eq!(fs::read_to_string(&overrides).unwrap(), original);
}

/// A theme file imported, applied, exported and removed from the command.
#[test]
fn themes_in_and_out_of_the_library() {
    if !(have("dbus-run-session") && have("gsettings")) {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "dbus-run-session and gsettings are required in CI"
        );
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::named("themes");
    let (code, resolved, _) = sb.velatura(&["theme", "resolve", "ember"]);
    assert_eq!(code, 0);
    assert!(resolved.contains("[variants.default.ui]") && !resolved.contains('@'));
    let file = sb.root.join("shared.toml");
    fs::write(&file, resolved.replace("id = \"ember\"", "id = \"embers\"")).unwrap();
    let file = file.to_str().unwrap();

    let (code, out, _) = sb.velatura(&["theme", "import", file, "--preview"]);
    assert_eq!(code, 0);
    assert!(out.contains("They cannot run anything"), "{out}");
    assert!(!sb.velatura(&["list"]).1.contains("embers"));
    let (code, out, err) = sb.velatura(&["theme", "import", file]);
    assert_eq!(code, 0, "{out}{err}");
    let (_, out, _) = sb.velatura(&["list"]);
    assert!(
        out.lines()
            .any(|l| l.starts_with("embers") && l.contains("(yours)")),
        "{out}"
    );

    // Twice needs a word; a bundled id can only be had under another one.
    let (code, _, err) = sb.velatura(&["theme", "import", file]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("already exists") && err.contains("--as <id>"),
        "{err}"
    );
    assert_eq!(sb.velatura(&["theme", "import", file, "--replace"]).0, 0);
    assert_eq!(
        sb.velatura(&["theme", "import", file, "--as", "ember"]).0,
        2
    );
    assert_eq!(
        sb.velatura(&["theme", "import", file, "--as", "Not An Id"])
            .0,
        2
    );
    assert_eq!(
        sb.velatura(&["theme", "import", file, "--as", "embers-two"])
            .0,
        0
    );

    // It applies like any other.
    let (code, out, err) = sb.velatura(&["apply", "embers-two", "--only", "gnome:appearance"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(sb.gsetting("accent-color"), "'orange'");

    let out_file = sb.root.join("out.toml");
    let out_path = out_file.to_str().unwrap();
    assert_eq!(sb.velatura(&["theme", "export", "embers", out_path]).0, 0);
    let (code, _, err) = sb.velatura(&["theme", "export", "ember", out_path]);
    assert_eq!(code, 2);
    assert!(err.contains("--replace"), "{err}");
    assert!(
        fs::read_to_string(&out_file)
            .unwrap()
            .contains("id = \"embers\"")
    );
    assert!(
        sb.velatura(&["theme", "export", "ember"])
            .1
            .contains("Bundled with Velatura")
    );

    // Not a theme: refused, with the reason.
    let bad = sb.root.join("bad.toml");
    fs::write(&bad, "#!/bin/sh\necho hello\n").unwrap();
    let (code, _, err) = sb.velatura(&["theme", "import", bad.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("Not a valid theme file"), "{err}");

    // A Kitty theme and a base16 scheme are read as themes, and apply like
    // any other.
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../velatura-theme/tests/fixtures");
    let dracula = fixtures.join("kitty-dracula.conf");
    let (code, out, err) = sb.velatura(&["theme", "import", dracula.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("A Kitty theme, read as a theme."), "{out}");
    assert!(out.contains("Imported Dracula as"), "{out}");
    let ocean = fixtures.join("base16-ocean.yaml");
    let (code, out, err) =
        sb.velatura(&["theme", "import", ocean.to_str().unwrap(), "--as", "sea"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("A base16 scheme, read as a theme."), "{out}");
    // No one to ask and none named: the usual accent, and how to name one.
    assert!(
        out.contains("Accent color: base0D #8fa1b3, the usual one. For another: --accent"),
        "{out}"
    );
    // Named by its name in the file, by its place in the list, or as a
    // color; anything else is refused with the names there are.
    for (word, id, hex) in [
        ("base0E", "sea-purple", "#b48ead"),
        ("BASE0e", "sea-purple-2", "#b48ead"),
        ("1", "sea-red", "#bf616a"),
        ("#336699", "sea-own", "#336699"),
    ] {
        let (code, out, err) = sb.velatura(&[
            "theme",
            "import",
            ocean.to_str().unwrap(),
            "--as",
            id,
            "--accent",
            word,
        ]);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains(&format!("{hex}.")), "{out}");
        let (_, out, _) = sb.velatura(&["show", id]);
        assert!(out.contains(&format!("accent         {hex}")), "{out}");
    }
    let (code, _, err) = sb.velatura(&[
        "theme",
        "import",
        ocean.to_str().unwrap(),
        "--as",
        "x",
        "--accent",
        "mauve",
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("base08, base09") && err.contains("#rrggbb"),
        "{err}"
    );
    assert!(!sb.velatura(&["list"]).1.contains("\nx "));
    // A theme file of Velatura's own names its accent itself.
    let (code, _, err) =
        sb.velatura(&["theme", "import", file, "--as", "y", "--accent", "#336699"]);
    assert_eq!(code, 2, "{err}");
    let (code, out, _) = sb.velatura(&["show", "sea"]);
    assert_eq!(code, 0);
    assert!(out.contains("#2b303b"), "{out}");
    let (code, out, err) = sb.velatura(&["apply", "dracula", "--only", "gnome:appearance"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(sb.gsetting("accent-color"), "'purple'");
    let short = sb.root.join("short.conf");
    fs::write(
        &short,
        "background #000000\nforeground #ffffff\ncolor0 #000000\n",
    )
    .unwrap();
    let (code, _, err) = sb.velatura(&["theme", "import", short.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        err.contains("A Kitty theme without all the colors a theme needs. Missing: color1, color2"),
        "{err}"
    );

    assert_eq!(sb.velatura(&["theme", "remove", "ember"]).0, 2);
    let (code, out, _) = sb.velatura(&["theme", "remove", "embers"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("To put it back: velatura theme import"),
        "{out}"
    );
    assert!(
        !sb.velatura(&["list"])
            .1
            .lines()
            .any(|l| l.starts_with("embers "))
    );
    let kept = sb.home().join(".local/share/velatura/removed/embers.toml");
    assert_eq!(
        sb.velatura(&["theme", "import", kept.to_str().unwrap()]).0,
        0
    );
}

/// Velatura's own Shell extension from the command, and the refusal to
/// follow the dark style with two dark themes.
#[test]
fn the_extension_from_the_command() {
    if !(have("dbus-run-session") && have("gsettings")) {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "dbus-run-session and gsettings are required in CI"
        );
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::named("extension");
    let dir = sb
        .home()
        .join(".local/share/gnome-shell/extensions/velatura@rafael0rueda.github.io");
    let (code, out, _) = sb.velatura(&["extension"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("is not installed") && out.contains("velatura extension install"),
        "{out}"
    );

    let (code, out, err) = sb.velatura(&["extension", "install"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("after signing out and in"), "{out}");
    assert!(out.contains("installed and turned off"), "{out}");
    assert!(dir.join("metadata.json").is_file() && dir.join("extension.js").is_file());
    // Installing does not turn it on: that is left to the user.
    let enabled = sb.run(
        Path::new("gsettings"),
        &["get", "org.gnome.shell", "enabled-extensions"],
    );
    assert!(!String::from_utf8_lossy(&enabled.stdout).contains("velatura"));
    let (_, out, _) = sb.velatura(&["extension", "--json"]);
    assert!(out.contains("\"by_velatura\": true"), "{out}");

    let (code, out, _) = sb.velatura(&["extension", "remove"]);
    assert_eq!(code, 0);
    assert!(out.contains("was removed"), "{out}");
    assert!(!sb.home().join(".local/share/gnome-shell").exists());
    assert!(
        sb.velatura(&["extension", "remove"])
            .1
            .contains("has not installed")
    );

    // Both bundled themes are dark: not a pair to follow the style with.
    assert_eq!(sb.velatura(&["switch", "set", "day", "ember"]).0, 0);
    assert_eq!(sb.velatura(&["switch", "set", "night", "aetherius"]).0, 0);
    let (code, _, err) = sb.velatura(&["schedule", "follow"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("needs a light theme for the day and a dark theme for the night"),
        "{err}"
    );
}

/// Ptyxis and Text Editor against the real settings store, in a private
/// session: a profile's settings live at a path of their own, which only
/// the real store can show to work.
#[test]
fn ptyxis_and_text_editor_settings_are_set_and_put_back() {
    if !(have("dbus-run-session") && have("gsettings")) {
        eprintln!("SKIPPED: dbus-run-session or gsettings is not installed");
        return;
    }
    let sb = Sandbox::named("gnome-apps");
    let get = |schema: &str, key: &str| {
        let out = sb.run(Path::new("gsettings"), &["get", schema, key]);
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    if get("org.gnome.Ptyxis", "default-profile-uuid").is_empty()
        || get("org.gnome.TextEditor", "style-scheme").is_empty()
    {
        assert!(
            std::env::var_os("VELATURA_CI").is_none(),
            "Ptyxis and Text Editor are required in CI"
        );
        eprintln!("SKIPPED: Ptyxis or Text Editor is not installed");
        return;
    }
    let profile = "org.gnome.Ptyxis.Profile:/org/gnome/Ptyxis/Profiles/0a1b2c3d/";
    let set = |schema: &str, key: &str, value: &str| {
        assert!(
            sb.run(Path::new("gsettings"), &["set", schema, key, value])
                .status
                .success()
        );
    };
    set("org.gnome.Ptyxis", "default-profile-uuid", "0a1b2c3d");
    set("org.gnome.Ptyxis", "profile-uuids", "['0a1b2c3d']");
    set(profile, "palette", "dracula");
    let only = ["--only", "ptyxis", "--only", "texteditor"];

    // Without a word of agreement nothing is touched.
    let (code, _, err) = sb.velatura(&[&["apply", "ember"], &only[..]].concat());
    assert_eq!(code, 3, "{err}");
    assert_eq!(get(profile, "palette"), "'dracula'");

    let (code, out, err) = sb.velatura(&[&["apply", "ember", "--yes"], &only[..]].concat());
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Set Ptyxis Palette to velatura"), "{out}");
    assert!(out.contains("Set Text Editor Style to velatura"), "{out}");
    assert_eq!(get(profile, "palette"), "'velatura'");
    assert_eq!(get("org.gnome.TextEditor", "style-scheme"), "'velatura'");
    let palette = sb
        .home()
        .join(".local/share/org.gnome.Ptyxis/palettes/velatura.palette");
    let scheme = sb
        .home()
        .join(".local/share/gtksourceview-5/styles/velatura.xml");
    assert!(
        fs::read_to_string(&palette)
            .unwrap()
            .contains("Background=#191316")
    );
    assert!(scheme.exists());

    // Another theme: the files change, the settings are right already.
    let (code, out, err) = sb.velatura(&[&["apply", "aetherius"], &only[..]].concat());
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        fs::read_to_string(&palette)
            .unwrap()
            .contains("Background=#141826")
    );

    assert_eq!(sb.velatura(&["undo", "--yes"]).0, 0);
    let (code, out, err) = sb.velatura(&["undo", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        get(profile, "palette"),
        "'dracula'",
        "the user's own choice is back"
    );
    let default = get("org.gnome.TextEditor", "style-scheme");
    assert_ne!(default, "'velatura'");
    assert!(!palette.exists() && !scheme.exists());
}
