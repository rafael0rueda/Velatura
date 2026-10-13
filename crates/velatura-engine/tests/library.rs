//! The user's theme library: saving, importing, replacing and removing.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use velatura_engine::host::{Dirs, Host, NoSettings, StdFs};
use velatura_engine::library::{self, Refused};
use velatura_theme::editor::Draft;
use velatura_theme::import::Format;
use velatura_theme::model::to_toml;
use velatura_theme::{Appearance, Color, bundled};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Home {
    root: PathBuf,
    dirs: Dirs,
}

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "velatura-library-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        fs::create_dir_all(home.join(".local/share")).unwrap();
        let dirs = Dirs::from_env(|k| (k == "HOME").then(|| OsString::from(&home))).unwrap();
        Self { root, dirs }
    }

    fn host(&self) -> Host<'_> {
        Host {
            dirs: &self.dirs,
            fs: &StdFs,
            settings: &NoSettings,
            now: 1_800_000_000,
        }
    }

    fn themes(&self) -> PathBuf {
        self.dirs.data.join("velatura/themes")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn draft(id: &str) -> Draft {
    let c = |hex| Color::parse(hex).unwrap();
    Draft::new(
        id,
        "Mine",
        Appearance::Dark,
        [c("#101418"), c("#e8e8e8"), c("#80a0ff")],
    )
}

fn ids(host: &Host<'_>) -> Vec<String> {
    library::load(host)
        .0
        .into_iter()
        .map(|e| e.theme.id)
        .collect()
}

#[test]
fn a_theme_made_in_the_editor_is_saved_listed_edited_and_removed() {
    let home = Home::new();
    let host = home.host();
    let mut d = draft("mine");

    let path = library::save(&host, &d.theme, None, false).unwrap();
    assert_eq!(path, home.themes().join("mine.toml"));
    assert_eq!(ids(&host), ["aetherius", "ember", "mine"]);
    assert!(library::taken(&host, "mine") && library::taken(&host, "ember"));
    assert!(!library::taken(&host, "other"));

    // A second save needs to say that it replaces.
    assert!(matches!(
        library::save(&host, &d.theme, None, false),
        Err(Refused::Exists(_))
    ));
    d.set_role("accent", Color::parse("#ff8800").unwrap());
    library::save(&host, &d.theme, None, true).unwrap();
    let (entries, rejected) = library::load(&host);
    assert!(rejected.is_empty());
    assert_eq!(
        entries[2].theme.variants["default"].ui.accent.hex(),
        "#ff8800"
    );
    assert_eq!(entries[2].file.as_deref(), Some(path.as_path()));
    // Written by the app both times: nothing of anyone's to keep aside.
    assert!(!home.themes().join("mine.toml.orig").exists());

    let text = library::remove(&host, "mine").unwrap();
    assert_eq!(ids(&host), ["aetherius", "ember"]);
    assert!(!path.exists());
    // And what removing gives back is enough to undo it.
    let back = velatura_theme::parse_theme(&text).unwrap().theme;
    library::save(&host, &back, Some(&text), false).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn what_comes_with_velatura_cannot_be_replaced_or_removed() {
    let home = Home::new();
    let host = home.host();
    let mut ember = bundled::themes().remove(1);
    for replace in [false, true] {
        assert!(matches!(
            library::save(&host, &ember, None, replace),
            Err(Refused::Reserved(_))
        ));
    }
    assert!(matches!(
        library::remove(&host, "ember"),
        Err(Refused::Reserved(_))
    ));
    assert!(matches!(
        library::remove(&host, "nothing"),
        Err(Refused::NotFound(_))
    ));
    for bad in ["", "Bad Id", "../x", "a/b", "-a"] {
        ember.id = bad.to_owned();
        assert!(
            matches!(
                library::save(&host, &ember, None, true),
                Err(Refused::BadId(_))
            ),
            "{bad:?}"
        );
    }
    assert!(!home.themes().exists(), "nothing was written");
    // A copy under another id is the way.
    ember.id = "ember-copy".to_owned();
    library::save(&host, &ember, None, false).unwrap();
    assert_eq!(ids(&host), ["aetherius", "ember", "ember-copy"]);
    // The bundled file itself can be had as text, to pass on.
    assert!(
        library::source(&host, "ember")
            .unwrap()
            .contains("id          = \"ember\"")
    );
}

#[test]
fn an_imported_file_is_checked_and_kept_as_it_was_written() {
    let home = Home::new();
    let host = home.host();
    let file = home.root.join("downloaded.toml");
    let written = format!(
        "# A theme by someone else.\n{}",
        to_toml(&draft("theirs").theme)
    );
    fs::write(&file, &written).unwrap();

    let library::Inspected { loaded, source, .. } = library::inspect(&host, &file).unwrap();
    assert_eq!(loaded.theme.id, "theirs");
    assert!(!home.themes().exists(), "looking is not importing");
    library::save(&host, &loaded.theme, Some(&source), false).unwrap();
    // Comments and all.
    assert_eq!(
        fs::read_to_string(home.themes().join("theirs.toml")).unwrap(),
        written
    );

    // Under another id the file has to be written anew; the id is the
    // only thing that differs.
    let mut renamed = loaded.theme.clone();
    renamed.id = "theirs-2".to_owned();
    library::save(&host, &renamed, Some(&source), false).unwrap();
    let text = fs::read_to_string(home.themes().join("theirs-2.toml")).unwrap();
    assert!(text.contains("id = \"theirs-2\"") && !text.contains("# A theme"));
    assert_eq!(ids(&host), ["aetherius", "ember", "theirs-2", "theirs"]);

    // Editing the hand-written one in the app keeps the original beside it.
    let mut edited = draft("theirs");
    edited.set_role("bg", Color::parse("#000000").unwrap());
    library::save(&host, &edited.theme, None, true).unwrap();
    assert_eq!(
        fs::read_to_string(home.themes().join("theirs.toml.orig")).unwrap(),
        written
    );
    assert_eq!(ids(&host).len(), 4, "the kept copy is not a second theme");
    // Only the first original is kept; later saves do not overwrite it.
    edited.set_role("bg", Color::parse("#111111").unwrap());
    library::save(&host, &edited.theme, None, true).unwrap();
    assert_eq!(
        fs::read_to_string(home.themes().join("theirs.toml.orig")).unwrap(),
        written
    );

    // What is not a theme is refused before anything is stored.
    let cases: [(&str, Vec<u8>); 4] = [
        ("not toml", b"this is { not toml".to_vec()),
        ("not text", vec![0xff, 0xfe, 0x00]),
        ("too large", vec![b'#'; 300 * 1024]),
        ("script", b"#!/bin/sh\nrm -rf ~\n".to_vec()),
    ];
    for (what, bytes) in cases {
        fs::write(&file, bytes).unwrap();
        assert!(
            matches!(
                library::inspect(&host, &file),
                Err(Refused::Invalid(_) | Refused::NotAThemeFile)
            ),
            "{what}"
        );
    }
    assert!(matches!(
        library::inspect(&host, &home.root.join("missing.toml")),
        Err(Refused::NotAThemeFile)
    ));
    assert!(matches!(
        library::inspect(&host, &home.root),
        Err(Refused::NotAThemeFile)
    ));
}

#[test]
fn a_file_in_the_way_is_not_overwritten_and_export_asks_before_replacing() {
    let home = Home::new();
    let host = home.host();
    // A file the library rejects sits where a new theme would go.
    fs::create_dir_all(home.themes()).unwrap();
    fs::write(home.themes().join("mine.toml"), "notes to self").unwrap();
    assert!(matches!(
        library::save(&host, &draft("mine").theme, None, true),
        Err(Refused::FileInTheWay(_))
    ));
    assert_eq!(
        fs::read_to_string(home.themes().join("mine.toml")).unwrap(),
        "notes to self"
    );

    library::save(&host, &draft("other").theme, None, false).unwrap();
    let out = home.root.join("shared.toml");
    library::export(&host, "other", &out, false).unwrap();
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        fs::read_to_string(home.themes().join("other.toml")).unwrap()
    );
    assert!(matches!(
        library::export(&host, "other", &out, false),
        Err(Refused::FileInTheWay(_))
    ));
    library::export(&host, "ember", &out, true).unwrap();
    assert!(fs::read_to_string(&out).unwrap().contains("Ember"));
    assert!(matches!(
        library::export(&host, "nothing", &out, true),
        Err(Refused::NotFound(_))
    ));
    // What was exported imports again as the same theme.
    let loaded = library::inspect(&host, &out).unwrap().loaded;
    assert_eq!(loaded.theme, bundled::themes()[1]);
}

#[test]
fn themes_in_other_formats_are_imported_as_themes_written_out() {
    let home = Home::new();
    let host = home.host();
    let fixtures =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../velatura-theme/tests/fixtures");
    for (file, format, id) in [
        ("base16-ocean.yaml", Format::Base16, "ocean"),
        ("base24-one-light.yaml", Format::Base24, "one-light"),
        ("kitty-dracula.conf", Format::Kitty, "dracula"),
    ] {
        let found = library::inspect(&host, &fixtures.join(file)).unwrap();
        assert_eq!(found.from, Some(format));
        assert_eq!(found.loaded.theme.id, id);
        let saved = library::save(&host, &found.loaded.theme, Some(&found.source), false).unwrap();
        // Stored as a theme file like any other: nothing of the file it
        // came from is kept but its colours and names.
        let stored = fs::read_to_string(saved).unwrap();
        assert_eq!(stored, to_toml(&found.loaded.theme));
        let (entries, rejected) = library::load(&host);
        assert!(rejected.is_empty());
        assert!(entries.iter().any(|e| e.theme == found.loaded.theme));
    }
    // Another of the file's colours as the accent, chosen before storing.
    let mut sea = library::inspect(&host, &fixtures.join("base16-ocean.yaml")).unwrap();
    assert_eq!(sea.accents[sea.accent].0, "base0D");
    let (name, purple) = sea.accents[6].clone();
    assert!(sea.choose_accent(purple));
    assert_eq!((name.as_str(), sea.accent), ("base0E", 6));
    sea.loaded.theme.id = "sea".to_owned();
    let saved = library::save(&host, &sea.loaded.theme, Some(&sea.source), false).unwrap();
    let stored = fs::read_to_string(saved).unwrap();
    assert!(stored.contains("accent = \"#b48ead\""), "{stored}");
    assert!(
        stored.contains("link = \"#b48ead\""),
        "what follows the accent follows it"
    );

    // The same file again is the same id, refused like any other clash.
    let again = library::inspect(&host, &fixtures.join("kitty-dracula.conf")).unwrap();
    assert!(matches!(
        library::save(&host, &again.loaded.theme, Some(&again.source), false),
        Err(Refused::Exists(_))
    ));

    // One that lacks colours says which, and nothing is stored.
    let file = home.root.join("short.yaml");
    fs::write(
        &file,
        "scheme: \"Short\"\nbase00: \"000000\"\nbase05: \"ffffff\"\n",
    )
    .unwrap();
    match library::inspect(&host, &file) {
        Err(Refused::Incomplete(lacking)) => {
            assert_eq!(lacking.format, Format::Base16);
            assert_eq!(lacking.missing.len(), 14);
        }
        other => panic!("{:?}", other.map(|i| i.from)),
    }
    // A file of Velatura's own with a mistake is still reported as that.
    let file = home.root.join("mine.toml");
    fs::write(&file, "schema = 1\n[meta]\nid = \"Bad Id\"\n").unwrap();
    assert!(matches!(
        library::inspect(&host, &file),
        Err(Refused::Invalid(_))
    ));
}
