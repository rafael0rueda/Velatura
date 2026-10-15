//! Plans: what an operation would do, as data.
//!
//! A preview and the operation it previews are the same [`Plan`]. Every
//! [`Change`] carries the state it was computed against, and executing it
//! refuses to run if that state has moved.

use std::path::PathBuf;

/// One thing an operation does to the machine. Nothing happens that is not
/// one of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    CreateDir {
        path: PathBuf,
    },
    /// Removed only if it is empty when its turn comes.
    RemoveDir {
        path: PathBuf,
    },
    /// A file Velatura creates and owns. `content` of `None` removes it.
    Owned {
        path: PathBuf,
        content: Option<Vec<u8>>,
        /// SHA-256 of the file as planned against; `None` if absent.
        pre: Option<String>,
    },
    /// A marked span inside a file the user maintains.
    Block {
        path: PathBuf,
        target: String,
        /// The whole file after the edit; `None` removes a file that
        /// Velatura created and that holds nothing else.
        content: Option<Vec<u8>>,
        pre: Option<String>,
        body_before: Option<String>,
        body_after: Option<String>,
        added_eol: bool,
        created_file: bool,
    },
    /// `value` of `None` returns the key to its default.
    Setting {
        schema: String,
        key: String,
        value: Option<String>,
        /// The user's value as planned against; `None` if at default.
        pre: Option<String>,
    },
}

/// Why an app or desktop part was left out of an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    NotInstalled,
    /// Excluded by `--only` or `--except`.
    NotSelected,
    /// Adding to settings the user maintains (a line in a file, or files in
    /// a folder) was not agreed to.
    NoConsent,
    /// The real location, after following links, is outside the home folder.
    OutsideHome(PathBuf),
    NotAFile(PathBuf),
    Unreadable(PathBuf),
    /// The file's markers are damaged: one without the other, or duplicates.
    MalformedBlock(PathBuf),
    /// A file with the name Velatura uses exists and Velatura did not make it.
    ForeignFile(PathBuf),
    /// A file Velatura made was changed outside Velatura since.
    ChangedOutside(PathBuf),
    SettingsUnavailable(String),
    /// The file has more than one name; replacing it would split them.
    HardLinked(PathBuf),
    /// High Contrast is on, and recolouring would lower contrast.
    HighContrast,
    /// Older GTK apps can only be recoloured through the adw-gtk3 theme,
    /// which is not installed.
    NeedsAdwGtk3,
    /// The Shell can only load a stylesheet through the User Themes
    /// extension, which is not installed.
    NeedsUserThemes,
    /// A GNOME Shell version Velatura has not been checked with.
    UntestedShell(String),
    /// The path would have to be written into a settings file and has
    /// characters that file's language gives a meaning to.
    UnusualPath(PathBuf),
    /// The file is not valid as it stands, so nothing is added to it.
    Invalid(PathBuf),
    /// The icon theme in use has no colour of its own to replace: it is
    /// all grey, or made of pictures, not drawings, or cannot be found.
    IconsNotRecolorable(String),
    /// The icon theme in use would need more recoloured icons than a set
    /// of Velatura's may hold.
    IconsTooMany(String),
    /// Starship's settings already choose a palette, or define the one
    /// Velatura would add. That choice is the user's.
    OwnPalette(PathBuf),
    /// Firefox is installed but has no profile yet: it has not been
    /// started, or its list of profiles cannot be read.
    FirefoxNoProfile,
    /// The cursor theme in use cannot be recoloured: it is not installed,
    /// or holds no cursor this reads.
    CursorsNotRecolorable(String),
    /// A template of the user's that cannot be used, and why.
    TemplateInvalid(crate::templates::Invalid),
}

/// Something the user should know that does not stop the operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Lines after Velatura's block that set the same colours and so win.
    Override {
        path: PathBuf,
        lines: Vec<usize>,
    },
    /// Kitty applies this file on top of every other colour setting.
    KittyAutoTheme {
        path: PathBuf,
    },
    /// Open Kitty windows pick the change up by themselves on recent
    /// versions; older ones need a manual reload.
    KittyReload,
    /// Ghostty reloads its settings on a key press.
    GhosttyReload,
    /// A running tmux keeps its colours until its settings are loaded
    /// again; `conf` is the file to load.
    TmuxReload {
        conf: PathBuf,
    },
    /// Neovim shows the colourscheme once it is selected, which is the
    /// user's to do in their own settings.
    NeovimSelect,
    /// The prompt shows the palette's colours only where its settings name
    /// them.
    StarshipNames,
    /// Ptyxis has no default profile yet, or has several: the palette is
    /// there to choose in its preferences for any profile it was not
    /// given to.
    PtyxisChoose,
    /// Ptyxis windows that are open may keep the colours they have.
    PtyxisReopen,
    /// GTK reads its stylesheet once, when an app starts.
    GtkRestart,
    /// Open apps show recoloured icons once reopened; signing out and in
    /// shows them everywhere.
    IconsReopen,
    /// The same for a recoloured cursor.
    CursorReopen,
    /// Firefox reads its stylesheet when it starts.
    FirefoxRestart,
    /// Firefox has other profiles, which are left as they are.
    FirefoxOtherProfiles,
    /// Firefox keeps the preference that loads `userChrome.css` in its own
    /// settings once it has read it, so taking the line out does not turn
    /// it off.
    FirefoxPrefStays,
    /// The settings file is a link; the change lands in `real`.
    LinkedFile {
        path: PathBuf,
        real: PathBuf,
    },
    /// A legacy GTK3 setting forces dark and would fight a light theme.
    /// A setting could not be put back because the settings store cannot
    /// be written from here (a text console with no desktop session).
    /// `value` is what it should be set to; `None` means its default.
    SettingNeedsSession {
        schema: String,
        key: String,
        value: Option<String>,
    },
    /// The Shell's own stylesheet could not be read, so only its main
    /// surfaces are recoloured, and it keeps the system accent.
    ShellShortList,
    /// User Themes is installed but turned off, so the Shell does not load
    /// the theme until it is turned on.
    UserThemesOff,
    /// Enabled extensions whose stylesheets colour the Shell's own
    /// surfaces, which always wins over a user theme.
    ShellExtensionStyles {
        names: Vec<String>,
    },
    Gtk3DarkForced {
        path: PathBuf,
    },
    /// The system accent is the nearest of GNOME's fixed set.
    AccentNearest {
        wanted: String,
        used: &'static str,
    },
    /// This GNOME version has no accent setting.
    AccentUnsupported,
    SettingLocked {
        key: String,
    },
    /// Left as it is because it was changed outside Velatura.
    KeptFile {
        path: PathBuf,
    },
    KeptSetting {
        key: String,
    },
    /// Text on background is below the contrast floor.
    LowContrast,
}

/// The part of a plan that concerns one app or desktop part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetPlan {
    /// `target:instance`, for example `kitty:default`.
    pub instance: String,
    pub changes: Vec<Change>,
    pub notes: Vec<Note>,
    pub skipped: Option<Skip>,
    /// True when this would add to settings the user maintains for the
    /// first time: a line in a file of theirs, or files in a folder of theirs.
    pub needs_consent: bool,
}

impl TargetPlan {
    pub fn new(instance: &str) -> Self {
        Self {
            instance: instance.to_owned(),
            changes: Vec::new(),
            notes: Vec::new(),
            skipped: None,
            needs_consent: false,
        }
    }

    pub fn skip(instance: &str, why: Skip) -> Self {
        Self {
            skipped: Some(why),
            ..Self::new(instance)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Apply,
    /// An apply made by the schedule, with no one there to answer.
    Scheduled,
    /// A choice of icons, cursor, wallpaper or font. The theme in use stays.
    Select,
    Undo,
    RestoreOriginal,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Scheduled => "scheduled",
            Self::Select => "select",
            Self::Undo => "undo",
            Self::RestoreOriginal => "restore-original",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub cause: Cause,
    /// `theme/variant` and its fingerprint, for an apply.
    pub theme: Option<(String, String)>,
    /// The backup an undo reverses.
    pub undo_of: Option<u64>,
    /// For an undo: the change being reversed was made by the schedule, so
    /// the change the user made is the one before it.
    pub scheduled: bool,
    pub targets: Vec<TargetPlan>,
}

impl Plan {
    /// True when executing would change nothing.
    pub fn is_empty(&self) -> bool {
        self.changes().next().is_none()
    }

    /// Every change of every target that is not skipped.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.targets
            .iter()
            .filter(|t| t.skipped.is_none())
            .flat_map(|t| &t.changes)
    }

    /// Instances that need the user's agreement before they are touched.
    pub fn needing_consent(&self) -> Vec<&str> {
        self.targets
            .iter()
            .filter(|t| t.skipped.is_none() && t.needs_consent)
            .map(|t| t.instance.as_str())
            .collect()
    }
}
