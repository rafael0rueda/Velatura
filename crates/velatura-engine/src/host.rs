//! The host-access layer: the only place that touches the file system.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The host's XDG base directories.
///
/// Resolved from an environment lookup that the caller supplies, so tests do
/// not depend on the process environment and a sandboxed build can map them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl Dirs {
    /// Follows the XDG Base Directory specification: a variable is used only
    /// if it is set to an absolute path; otherwise the default under `HOME`.
    pub fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Option<Self> {
        let home = PathBuf::from(var("HOME")?);
        if !home.is_absolute() {
            return None;
        }
        let xdg = |name: &str, default: &str| match var(name).map(PathBuf::from) {
            Some(p) if p.is_absolute() => p,
            _ => home.join(default),
        };
        Some(Self {
            config: xdg("XDG_CONFIG_HOME", ".config"),
            data: xdg("XDG_DATA_HOME", ".local/share"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            home,
        })
    }
}

/// File operations the engine is allowed to perform.
pub trait Fs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Replace the content of `path` so that a reader sees either the old
    /// content or the new content, never a mixture.
    ///
    /// If `path` is a symlink the file it points to is replaced and the link
    /// is kept. An existing file keeps its permission bits; a new file gets
    /// `mode`.
    fn write_atomic(&self, path: &Path, content: &[u8], mode: u32) -> io::Result<()>;

    /// What is at `path`, without following a final symlink. `None` if
    /// nothing is there.
    fn kind(&self, path: &Path) -> io::Result<Option<Kind>>;

    /// Resolve every symlink in `path`.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// Create one directory. The parent must exist.
    fn create_dir(&self, path: &Path, mode: u32) -> io::Result<()>;

    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Fails if the directory is not empty.
    fn remove_empty_dir(&self, path: &Path) -> io::Result<()>;

    /// Names of the entries in a directory, sorted.
    fn list(&self, path: &Path) -> io::Result<Vec<OsString>>;

    /// How many names the file at `path` has, following a final symlink.
    /// More than one means it is hard-linked, and replacing it by rename
    /// would silently split it from its other names.
    fn links(&self, path: &Path) -> io::Result<u64>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

/// The file name [`StdFs::write_atomic`] gives its temporary file starts
/// with a dot, then the target's name, then this.
pub const TMP_MARK: &str = ".velatura-tmp-";

/// [`Fs`] on the real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdFs;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Fs for StdFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write_atomic(&self, path: &Path, content: &[u8], mode: u32) -> io::Result<()> {
        // Write through a symlink to the real file, so a dotfile manager's
        // link is not replaced by a regular file.
        let (real, mode) = match fs::canonicalize(path) {
            Ok(real) => {
                let mode = fs::metadata(&real)?.permissions().mode() & 0o7777;
                (real, mode)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (path.to_path_buf(), mode),
            Err(e) => return Err(e),
        };
        let dir = real
            .parent()
            .ok_or_else(|| io::Error::other("path has no parent directory"))?;
        let name = real
            .file_name()
            .ok_or_else(|| io::Error::other("path has no file name"))?;

        // The temporary file lives in the same directory, so the rename
        // cannot cross a file system boundary.
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(format!(
            "{TMP_MARK}{}-{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let tmp = dir.join(tmp_name);

        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(content)?;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            copy_attributes(&real, &tmp);
            file.sync_all()?;
            fs::rename(&tmp, &real)?;
            File::open(dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    fn kind(&self, path: &Path) -> io::Result<Option<Kind>> {
        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => Ok(Some(Kind::Symlink)),
            Ok(m) if m.is_file() => Ok(Some(Kind::File)),
            Ok(m) if m.is_dir() => Ok(Some(Kind::Dir)),
            Ok(_) => Ok(Some(Kind::Other)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn create_dir(&self, path: &Path, mode: u32) -> io::Result<()> {
        fs::DirBuilder::new().mode(mode).create(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(path)? {
            names.push(entry?.file_name());
        }
        names.sort();
        Ok(names)
    }

    fn links(&self, path: &Path) -> io::Result<u64> {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path)?.nlink())
    }
}

/// Carry the extended attributes of the file being replaced over to its
/// replacement: the security label, access lists and anything a tool has
/// tagged it with. Best effort: an attribute the system refuses to set is
/// skipped, because a new file already gets the folder's defaults.
fn copy_attributes(from: &Path, to: &Path) {
    let Ok(names) = xattr::list(from) else {
        return;
    };
    for name in names {
        if let Ok(Some(value)) = xattr::get(from, &name) {
            let _ = xattr::set(to, &name, &value);
        }
    }
}

/// The state of one settings key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingState {
    /// The value the user set, or `None` when the key is at its default.
    pub user: Option<String>,
    /// The value in effect: the user's, or the default.
    pub effective: String,
    /// The value in effect while the user has set none.
    pub default: String,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsError {
    /// The settings store cannot be used at all (no session, wrong backend).
    Unavailable(String),
    NotWritable,
    Failed(String),
}

/// The reason alone, as it reads after "desktop settings are not available".
impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) | Self::Failed(why) => f.write_str(why),
            Self::NotWritable => f.write_str("the setting is locked"),
        }
    }
}

/// Desktop settings as text. Text keys read and write as their value;
/// other types read in their printed form (`true`, `42`) and are not written.
/// A schema given as `id@/folder` is looked up in that folder's compiled
/// schemas, where an extension installed for one user keeps its own.
/// A schema given as `id:/path/` has no place of its own in the store and
/// is read and written at that path, as `gsettings` takes it.
/// A schema given as `dbus:name` is not a setting at all: it reads the
/// property `key` of the program of that name on the session bus, which is
/// how the running Shell says which version it is. One given as
/// `extension:uuid` reads the field `key` of the metadata the running
/// Shell has loaded for that extension, which is how it says which copy
/// it runs. Neither is ever written.
pub trait Settings {
    /// `Ok(None)` when the schema or the key does not exist on this system.
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError>;

    /// Set the user's value, or with `None` return the key to its default.
    fn write(&self, schema: &str, key: &str, value: Option<&str>) -> Result<(), SettingsError>;

    /// Block until earlier writes have reached the store.
    fn sync(&self);

    /// The keys `schema` has, in the same forms of schema as `read` takes.
    /// Empty when the schema does not exist or the store cannot list them.
    fn keys(&self, _schema: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Settings held in memory, for tests and for systems with no settings store.
#[derive(Debug, Default)]
pub struct MemSettings {
    defaults: BTreeMap<(String, String), String>,
    user: RefCell<BTreeMap<(String, String), String>>,
    locked: RefCell<Vec<(String, String)>>,
    /// Number of successful writes, so a test can assert "nothing written".
    pub writes: Cell<u32>,
}

impl MemSettings {
    /// A store that knows `keys`, each given as `(schema, key, default)`.
    pub fn new(keys: &[(&str, &str, &str)]) -> Self {
        Self {
            defaults: keys
                .iter()
                .map(|(s, k, d)| (((*s).to_owned(), (*k).to_owned()), (*d).to_owned()))
                .collect(),
            ..Self::default()
        }
    }

    /// The GNOME 47+ keys the appearance target uses, at their defaults.
    pub fn gnome() -> Self {
        Self::new(&[
            ("org.gnome.desktop.interface", "color-scheme", "default"),
            ("org.gnome.desktop.interface", "accent-color", "blue"),
            ("org.gnome.desktop.interface", "gtk-theme", "Adwaita"),
            ("org.gnome.desktop.a11y.interface", "high-contrast", "false"),
            ("org.gnome.shell.extensions.user-theme", "name", ""),
        ])
    }

    pub fn lock(&self, schema: &str, key: &str) {
        self.locked
            .borrow_mut()
            .push((schema.to_owned(), key.to_owned()));
    }

    /// Every user-set value, for comparing two states.
    pub fn snapshot(&self) -> BTreeMap<(String, String), String> {
        self.user.borrow().clone()
    }
}

impl Settings for MemSettings {
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError> {
        let id = (schema.to_owned(), key.to_owned());
        let Some(default) = self.defaults.get(&id) else {
            return Ok(None);
        };
        let user = self.user.borrow().get(&id).cloned();
        Ok(Some(SettingState {
            effective: user.clone().unwrap_or_else(|| default.clone()),
            default: default.clone(),
            user,
            writable: !self.locked.borrow().contains(&id),
        }))
    }

    fn write(&self, schema: &str, key: &str, value: Option<&str>) -> Result<(), SettingsError> {
        let id = (schema.to_owned(), key.to_owned());
        if !self.defaults.contains_key(&id) {
            return Err(SettingsError::Failed("no such key".to_owned()));
        }
        if self.locked.borrow().contains(&id) {
            return Err(SettingsError::NotWritable);
        }
        match value {
            Some(v) => self.user.borrow_mut().insert(id, v.to_owned()),
            None => self.user.borrow_mut().remove(&id),
        };
        self.writes.set(self.writes.get() + 1);
        Ok(())
    }

    fn sync(&self) {}

    fn keys(&self, schema: &str) -> Vec<String> {
        self.defaults
            .keys()
            .filter(|(s, _)| s == schema)
            .map(|(_, k)| k.clone())
            .collect()
    }
}

/// A settings store that is absent: every key reads as missing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSettings;

impl Settings for NoSettings {
    fn read(&self, _: &str, _: &str) -> Result<Option<SettingState>, SettingsError> {
        Err(SettingsError::Unavailable(
            "built without desktop settings support".to_owned(),
        ))
    }
    fn write(&self, _: &str, _: &str, _: Option<&str>) -> Result<(), SettingsError> {
        Err(SettingsError::NotWritable)
    }
    fn sync(&self) {}
}

/// Counts every operation that changes the machine and makes the one numbered
/// `fail_at`, and all after it, fail. With `fail_at` past the end it only
/// counts. This is how tests stop an apply at every possible step.
pub struct Faults<'a> {
    fs: &'a dyn Fs,
    settings: &'a dyn Settings,
    fail_at: u32,
    pub count: Cell<u32>,
}

impl<'a> Faults<'a> {
    pub fn new(fs: &'a dyn Fs, settings: &'a dyn Settings, fail_at: u32) -> Self {
        Self {
            fs,
            settings,
            fail_at,
            count: Cell::new(0),
        }
    }

    fn step(&self) -> io::Result<()> {
        let n = self.count.get();
        self.count.set(n + 1);
        if n >= self.fail_at {
            Err(io::Error::other("injected failure"))
        } else {
            Ok(())
        }
    }
}

impl Fs for Faults<'_> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.fs.read(path)
    }
    fn write_atomic(&self, path: &Path, content: &[u8], mode: u32) -> io::Result<()> {
        self.step()?;
        self.fs.write_atomic(path, content, mode)
    }
    fn kind(&self, path: &Path) -> io::Result<Option<Kind>> {
        self.fs.kind(path)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.fs.canonicalize(path)
    }
    fn create_dir(&self, path: &Path, mode: u32) -> io::Result<()> {
        self.step()?;
        self.fs.create_dir(path, mode)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.step()?;
        self.fs.remove_file(path)
    }
    fn remove_empty_dir(&self, path: &Path) -> io::Result<()> {
        self.step()?;
        self.fs.remove_empty_dir(path)
    }
    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        self.fs.list(path)
    }
    fn links(&self, path: &Path) -> io::Result<u64> {
        self.fs.links(path)
    }
}

impl Settings for Faults<'_> {
    fn read(&self, schema: &str, key: &str) -> Result<Option<SettingState>, SettingsError> {
        self.settings.read(schema, key)
    }
    fn write(&self, schema: &str, key: &str, value: Option<&str>) -> Result<(), SettingsError> {
        self.step()
            .map_err(|e| SettingsError::Failed(e.to_string()))?;
        self.settings.write(schema, key, value)
    }
    fn sync(&self) {
        self.settings.sync();
    }
    fn keys(&self, schema: &str) -> Vec<String> {
        self.settings.keys(schema)
    }
}

/// The one program Velatura runs: the user's own systemd, to load and turn
/// on the timer it writes. Never the system's, never with elevated rights.
pub trait Systemd {
    /// Run `systemctl --user` with `args`. The error is what it printed.
    fn user(&self, args: &[&str]) -> Result<(), String>;
}

/// [`Systemd`] through the `systemctl` on the path.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdSystemd;

impl Systemd for StdSystemd {
    fn user(&self, args: &[&str]) -> Result<(), String> {
        let out = std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let said = String::from_utf8_lossy(&out.stderr);
            Err(said.lines().next().unwrap_or("systemctl failed").to_owned())
        }
    }
}

/// A systemd that only remembers what it was asked, for tests. With `fail`
/// set, a call whose first word is that fails.
#[derive(Debug, Default)]
pub struct MemSystemd {
    pub calls: RefCell<Vec<String>>,
    pub fail: Cell<Option<&'static str>>,
}

impl Systemd for MemSystemd {
    fn user(&self, args: &[&str]) -> Result<(), String> {
        self.calls.borrow_mut().push(args.join(" "));
        if self.fail.get().is_some_and(|f| args.first() == Some(&f)) {
            Err("injected failure".to_owned())
        } else {
            Ok(())
        }
    }
}

/// Where the running program is, for a timer to start it again later.
/// With `sibling`, the program of that name in the same folder instead.
pub fn self_command(sibling: Option<&str>) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    match sibling {
        Some(name) => Some(exe.parent()?.join(name)),
        None => Some(exe),
    }
}

/// Take the lock that serialises every change Velatura makes. Waits up to
/// `wait` for another holder; `Ok(None)` means it is still busy. The lock is
/// released when the returned file is dropped, or when the process dies.
pub fn acquire_lock(path: &Path, wait: std::time::Duration) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// Everything the engine may touch, handed to it by the caller.
#[derive(Clone, Copy)]
pub struct Host<'a> {
    pub dirs: &'a Dirs,
    pub fs: &'a dyn Fs,
    pub settings: &'a dyn Settings,
    /// Seconds since the Unix epoch.
    pub now: u64,
}

impl Host<'_> {
    pub fn state_dir(&self) -> PathBuf {
        self.dirs.state.join("velatura")
    }
    pub fn data_dir(&self) -> PathBuf {
        self.dirs.data.join("velatura")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir().join("backups")
    }
    pub fn themes_dir(&self) -> PathBuf {
        self.data_dir().join("themes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "velatura-test-{tag}-{}-{}",
                std::process::id(),
                TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn dirs_use_defaults_and_absolute_overrides() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let d = Dirs::from_env(env(&[("HOME", "/home/u")])).unwrap();
        assert_eq!(d.config, Path::new("/home/u/.config"));
        assert_eq!(d.data, Path::new("/home/u/.local/share"));
        assert_eq!(d.state, Path::new("/home/u/.local/state"));
        assert_eq!(d.cache, Path::new("/home/u/.cache"));

        let d = Dirs::from_env(env(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_DATA_HOME", "relative/ignored"),
        ]))
        .unwrap();
        assert_eq!(d.config, Path::new("/cfg"));
        assert_eq!(d.data, Path::new("/home/u/.local/share"));

        assert_eq!(Dirs::from_env(env(&[])), None);
        assert_eq!(Dirs::from_env(env(&[("HOME", "not/absolute")])), None);
    }

    #[test]
    fn creates_new_file_with_requested_mode() {
        let t = TempDir::new("new");
        let path = t.0.join("velatura.conf");
        StdFs.write_atomic(&path, b"one\n", 0o644).unwrap();
        assert_eq!(StdFs.read(&path).unwrap(), b"one\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o644
        );
        assert_eq!(
            names(&t.0),
            ["velatura.conf"],
            "no temporary file left behind"
        );
    }

    #[test]
    fn replaces_content_and_keeps_existing_mode() {
        let t = TempDir::new("replace");
        let path = t.0.join("kitty.conf");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        StdFs.write_atomic(&path, b"new", 0o644).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert_eq!(names(&t.0), ["kitty.conf"]);
    }

    #[test]
    fn writes_through_a_symlink_and_keeps_the_link() {
        let t = TempDir::new("symlink");
        let dotfiles = t.0.join("dotfiles");
        let config = t.0.join("config");
        fs::create_dir_all(&dotfiles).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(dotfiles.join("gtk.css"), b"old").unwrap();
        let link = config.join("gtk.css");
        symlink(dotfiles.join("gtk.css"), &link).unwrap();

        StdFs.write_atomic(&link, b"new", 0o644).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(dotfiles.join("gtk.css")).unwrap(), b"new");
        assert_eq!(names(&config), ["gtk.css"]);
        assert_eq!(names(&dotfiles), ["gtk.css"]);
    }

    #[test]
    fn failure_leaves_the_original_untouched() {
        let t = TempDir::new("fail");
        let missing_dir = t.0.join("no-such-dir").join("file");
        assert!(StdFs.write_atomic(&missing_dir, b"x", 0o644).is_err());
        assert_eq!(names(&t.0), Vec::<String>::new());
    }

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let t = TempDir::new("lock");
        let path = t.0.join("lock");
        let none = std::time::Duration::ZERO;
        let held = acquire_lock(&path, none)
            .unwrap()
            .expect("first taker gets it");
        assert!(
            acquire_lock(&path, none).unwrap().is_none(),
            "second is told busy"
        );
        let waited = std::time::Instant::now();
        assert!(
            acquire_lock(&path, std::time::Duration::from_millis(120))
                .unwrap()
                .is_none()
        );
        assert!(waited.elapsed() >= std::time::Duration::from_millis(100));
        drop(held);
        assert!(acquire_lock(&path, none).unwrap().is_some());
    }

    #[test]
    fn memory_settings_tell_set_from_default() {
        let s = MemSettings::gnome();
        let read = |s: &MemSettings| s.read("org.gnome.desktop.interface", "accent-color");
        assert_eq!(read(&s).unwrap().unwrap().user, None);
        s.write("org.gnome.desktop.interface", "accent-color", Some("blue"))
            .unwrap();
        let now = read(&s).unwrap().unwrap();
        assert_eq!(
            (now.user.as_deref(), now.effective.as_str()),
            (Some("blue"), "blue")
        );
        s.write("org.gnome.desktop.interface", "accent-color", None)
            .unwrap();
        assert_eq!(read(&s).unwrap().unwrap().user, None);
        assert_eq!(s.read("org.gnome.desktop.interface", "nope").unwrap(), None);
        assert!(s.write("no.such.schema", "k", Some("v")).is_err());
    }

    #[test]
    fn replacing_a_file_keeps_its_extended_attributes() {
        let t = TempDir::new("xattr");
        let path = t.0.join("gtk.css");
        fs::write(&path, b"old").unwrap();
        if xattr::set(&path, "user.velatura.test", b"kept").is_err() {
            eprintln!("SKIPPED: this file system has no user attributes");
            return;
        }
        StdFs.write_atomic(&path, b"new", 0o644).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            xattr::get(&path, "user.velatura.test").unwrap().as_deref(),
            Some(&b"kept"[..])
        );
    }

    #[test]
    fn counts_hard_links() {
        let t = TempDir::new("links");
        let a = t.0.join("a");
        fs::write(&a, b"x").unwrap();
        assert_eq!(StdFs.links(&a).unwrap(), 1);
        fs::hard_link(&a, t.0.join("b")).unwrap();
        assert_eq!(StdFs.links(&a).unwrap(), 2);
    }
}
