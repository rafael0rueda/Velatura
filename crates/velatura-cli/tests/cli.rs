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
        let root = std::env::temp_dir().join(format!("velatura-cli-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "home/.config/kitty",
            "home/.local/share",
            "home/.local/state",
            "run",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn run(&self, program: &Path, args: &[&str]) -> Output {
        Command::new("dbus-run-session")
            .arg("--")
            .arg(program)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("GSETTINGS_BACKEND", "dconf")
            .env("LC_ALL", "C.UTF-8")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
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
