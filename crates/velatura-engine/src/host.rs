//! The host-access layer: the only place that touches the file system.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
}

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
            ".velatura-tmp-{}-{}",
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
            file.sync_all()?;
            fs::rename(&tmp, &real)?;
            File::open(dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
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
}
