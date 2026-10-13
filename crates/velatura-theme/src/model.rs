//! The theme file format: parsing, validation and derivation.
//!
//! A theme file is untrusted input. It is size-capped before parsing, every
//! colour is parsed into a [`Color`], and no string from the file other than
//! the validated slugs can reach a generated config.

use std::collections::BTreeMap;
use std::fmt;

use toml::{Table, Value};

use crate::Color;

/// The newest file format this build reads.
pub const SCHEMA: i64 = 1;
/// The newest derivation rule this build knows.
pub const DERIVE: i64 = 1;
/// Theme files larger than this are refused before parsing.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

const MAX_VARIANTS: usize = 16;
const MAX_PALETTE: usize = 512;
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 300;

/// The 16 terminal colours, in the order terminals number them.
pub const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

pub const SYNTAX_ROLES: [&str; 16] = [
    "comment",
    "keyword",
    "string",
    "number",
    "constant",
    "function",
    "type",
    "variable",
    "parameter",
    "operator",
    "punctuation",
    "property",
    "tag",
    "attribute",
    "builtin",
    "escape",
];
pub const DIFF_ROLES: [&str; 4] = ["add", "change", "delete", "text"];
pub const DIAGNOSTIC_ROLES: [&str; 5] = ["error", "warning", "info", "hint", "ok"];

/// Per-app slots that targets declare. A theme may only fill these.
pub const SLOTS: [(&str, &[&str]); 1] = [("starship", &crate::starship::SLOTS)];

const SUGGESTS: [&str; 2] = ["icon_theme", "cursor_theme"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// The interface roles. Every one is filled: by the file, or by derivation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ui {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub bg_dim: Color,
    pub surface: Color,
    pub border: Color,
    pub fg_muted: Color,
    pub fg_subtle: Color,
    pub accent_fg: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub cursor: Color,
    pub cursor_fg: Color,
    pub link: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
}

pub const UI_REQUIRED: [&str; 3] = ["bg", "fg", "accent"];
pub const UI_OPTIONAL: [&str; 14] = [
    "bg_dim",
    "surface",
    "border",
    "fg_muted",
    "fg_subtle",
    "accent_fg",
    "selection_bg",
    "selection_fg",
    "cursor",
    "cursor_fg",
    "link",
    "success",
    "warning",
    "error",
];

impl Ui {
    /// The role called `name`, if there is one.
    pub fn get(&self, name: &str) -> Option<Color> {
        self.roles()
            .into_iter()
            .find(|(role, _)| *role == name)
            .map(|(_, c)| c)
    }

    /// Give the role called `name` a colour. False if there is no such role.
    pub fn set(&mut self, name: &str, color: Color) -> bool {
        let slot = match name {
            "bg" => &mut self.bg,
            "fg" => &mut self.fg,
            "accent" => &mut self.accent,
            "bg_dim" => &mut self.bg_dim,
            "surface" => &mut self.surface,
            "border" => &mut self.border,
            "fg_muted" => &mut self.fg_muted,
            "fg_subtle" => &mut self.fg_subtle,
            "accent_fg" => &mut self.accent_fg,
            "selection_bg" => &mut self.selection_bg,
            "selection_fg" => &mut self.selection_fg,
            "cursor" => &mut self.cursor,
            "cursor_fg" => &mut self.cursor_fg,
            "link" => &mut self.link,
            "success" => &mut self.success,
            "warning" => &mut self.warning,
            "error" => &mut self.error,
            _ => return false,
        };
        *slot = color;
        true
    }

    /// Every role with its name, in a fixed order.
    pub fn roles(&self) -> [(&'static str, Color); 17] {
        [
            ("bg", self.bg),
            ("fg", self.fg),
            ("accent", self.accent),
            ("bg_dim", self.bg_dim),
            ("surface", self.surface),
            ("border", self.border),
            ("fg_muted", self.fg_muted),
            ("fg_subtle", self.fg_subtle),
            ("accent_fg", self.accent_fg),
            ("selection_bg", self.selection_bg),
            ("selection_fg", self.selection_fg),
            ("cursor", self.cursor),
            ("cursor_fg", self.cursor_fg),
            ("link", self.link),
            ("success", self.success),
            ("warning", self.warning),
            ("error", self.error),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// Display name. Sanitised; never written to a generated file.
    pub name: String,
    pub appearance: Appearance,
    pub palette: BTreeMap<String, Color>,
    pub ui: Ui,
    /// Names of the `ui` roles that were filled by derivation.
    pub derived: Vec<&'static str>,
    pub ansi: [Color; 16],
    pub syntax: BTreeMap<&'static str, Color>,
    pub diff: BTreeMap<&'static str, Color>,
    pub diagnostic: BTreeMap<&'static str, Color>,
    pub slots: BTreeMap<&'static str, BTreeMap<&'static str, Color>>,
    /// Advisory names. Matched by equality against installed items only.
    pub suggests: BTreeMap<&'static str, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Slug: `[a-z0-9-]`, starts with a letter or digit, at most 64 bytes.
    pub id: String,
    pub name: String,
    pub author: String,
    pub license: String,
    pub description: String,
    pub derive: i64,
    /// Keyed by variant slug (same alphabet as `id`).
    pub variants: BTreeMap<String, Variant>,
}

impl Theme {
    /// `default` if the theme has one, otherwise the first by name.
    pub fn default_variant(&self) -> &str {
        if self.variants.contains_key("default") {
            "default"
        } else {
            self.variants
                .keys()
                .next()
                .map_or("default", String::as_str)
        }
    }
}

impl Variant {
    /// The colour of a code role: the theme's own, or one taken from the
    /// terminal and interface colours when the theme does not give it.
    /// `role` is one of [`SYNTAX_ROLES`]; anything else is the text colour.
    pub fn syntax_color(&self, role: &str) -> Color {
        if let Some(color) = self.syntax.get(role) {
            return *color;
        }
        let (ui, ansi) = (&self.ui, &self.ansi);
        match role {
            "comment" => ui.fg_subtle,
            "keyword" => ansi[5],
            "string" => ansi[2],
            "number" => ansi[13],
            "constant" => ansi[3],
            "function" => ansi[4],
            "type" => ansi[6],
            "parameter" | "operator" | "punctuation" => ui.fg_muted,
            "property" => ansi[12],
            "tag" => ansi[1],
            "attribute" => ansi[11],
            "builtin" => ansi[14],
            "escape" => ansi[9],
            _ => ui.fg,
        }
    }

    /// The colour of a diagnostic role, one of [`DIAGNOSTIC_ROLES`].
    pub fn diagnostic_color(&self, role: &str) -> Color {
        if let Some(color) = self.diagnostic.get(role) {
            return *color;
        }
        match role {
            "error" => self.ui.error,
            "warning" => self.ui.warning,
            "info" => self.ansi[4],
            "hint" => self.ansi[6],
            _ => self.ui.success,
        }
    }

    /// The background of a changed line, for one of [`DIFF_ROLES`]. Without
    /// one of its own it is the background tinted towards a signal colour.
    pub fn diff_color(&self, role: &str) -> Color {
        if let Some(color) = self.diff.get(role) {
            return *color;
        }
        let (toward, by) = match role {
            "add" => (self.ui.success, 0.2),
            "change" => (self.ui.warning, 0.2),
            "delete" => (self.ui.error, 0.2),
            _ => (self.ansi[4], 0.35),
        };
        self.ui.bg.mix(toward, by)
    }

    /// The colour of one of an app's own slots, or `fallback`.
    pub fn slot(&self, app: &str, name: &str, fallback: Color) -> Color {
        self.slots
            .get(app)
            .and_then(|slots| slots.get(name))
            .copied()
            .unwrap_or(fallback)
    }

    /// What the derivation rule gives for every role, from this variant's
    /// background, text, accent and terminal colours.
    pub fn automatic(&self) -> Ui {
        derive_v1(
            self.appearance,
            self.ui.bg,
            self.ui.fg,
            self.ui.accent,
            &self.ansi,
        )
    }

    /// Fill the roles listed in `derived` again, after one of the colours
    /// they are worked out from has changed.
    pub fn rederive(&mut self) {
        let auto = self.automatic();
        for role in self.derived.clone() {
            if let Some(color) = auto.get(role) {
                self.ui.set(role, color);
            }
        }
    }
}

/// Text as a TOML basic string.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The theme as a theme file with every colour written out: no references,
/// nothing left to derivation. Reading it back gives the same colours.
pub fn to_toml(theme: &Theme) -> String {
    use std::fmt::Write;
    let mut out = format!("schema = {SCHEMA}\nderive = {}\n\n[meta]\n", theme.derive);
    let _ = writeln!(out, "id = {}", quoted(&theme.id));
    for (key, value) in [
        ("name", &theme.name),
        ("author", &theme.author),
        ("license", &theme.license),
        ("description", &theme.description),
    ] {
        if !value.is_empty() {
            let _ = writeln!(out, "{key} = {}", quoted(value));
        }
    }
    for (id, v) in &theme.variants {
        let at = format!("variants.{id}");
        let _ = writeln!(out, "\n[{at}]\nname = {}", quoted(&v.name));
        let _ = writeln!(out, "appearance = \"{}\"", v.appearance.as_str());
        let mut table = |name: &str, colors: &mut dyn Iterator<Item = (&str, Color)>| {
            let mut colors = colors.peekable();
            if colors.peek().is_some() {
                let _ = writeln!(out, "\n[{at}.{name}]");
                for (key, color) in colors {
                    let _ = writeln!(out, "{key} = \"{}\"", color.hex());
                }
            }
        };
        table(
            "palette",
            &mut v.palette.iter().map(|(k, c)| (k.as_str(), *c)),
        );
        table("ui", &mut v.ui.roles().into_iter());
        table("ansi", &mut ANSI_NAMES.iter().copied().zip(v.ansi));
        table("syntax", &mut v.syntax.iter().map(|(k, c)| (*k, *c)));
        table("diff", &mut v.diff.iter().map(|(k, c)| (*k, *c)));
        table(
            "diagnostic",
            &mut v.diagnostic.iter().map(|(k, c)| (*k, *c)),
        );
        for (target, slots) in &v.slots {
            table(
                &format!("slots.{target}"),
                &mut slots.iter().map(|(k, c)| (*k, *c)),
            );
        }
        if !v.suggests.is_empty() {
            let _ = writeln!(out, "\n[{at}.suggests]");
            for (key, value) in &v.suggests {
                let _ = writeln!(out, "{key} = {}", quoted(value));
            }
        }
    }
    out
}

/// Stable codes for everything the validator can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    TooLarge,
    Syntax,
    NewerSchema,
    BadSchema,
    NewerDerive,
    Missing,
    WrongType,
    BadSlug,
    BadColor,
    AlphaNotAllowed,
    UnknownReference,
    BadAppearance,
    NoVariants,
    TooMany,
    UnknownKey,
    UnknownSlot,
    LowContrast,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too-large",
            Self::Syntax => "syntax",
            Self::NewerSchema => "newer-schema",
            Self::BadSchema => "bad-schema",
            Self::NewerDerive => "newer-derive",
            Self::Missing => "missing",
            Self::WrongType => "wrong-type",
            Self::BadSlug => "bad-slug",
            Self::BadColor => "bad-color",
            Self::AlphaNotAllowed => "alpha-not-allowed",
            Self::UnknownReference => "unknown-reference",
            Self::BadAppearance => "bad-appearance",
            Self::NoVariants => "no-variants",
            Self::TooMany => "too-many",
            Self::UnknownKey => "unknown-key",
            Self::UnknownSlot => "unknown-slot",
            Self::LowContrast => "low-contrast",
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::TooLarge => "the file is larger than 256 KiB",
            Self::Syntax => "not valid TOML",
            Self::NewerSchema => "made for a newer version of Velatura",
            Self::BadSchema => "schema must be 1",
            Self::NewerDerive => "uses a derivation rule this version does not know",
            Self::Missing => "required value is missing",
            Self::WrongType => "value has the wrong type",
            Self::BadSlug => "must be lower-case letters, digits and hyphens, at most 64",
            Self::BadColor => "color must be #RRGGBB or @name",
            Self::AlphaNotAllowed => "this role does not allow transparency",
            Self::UnknownReference => "no color with this name in the variant's palette",
            Self::BadAppearance => "appearance must be \"dark\" or \"light\"",
            Self::NoVariants => "a theme needs at least one variant",
            Self::TooMany => "too many entries",
            Self::UnknownKey => "unknown key, ignored",
            Self::UnknownSlot => "unknown app or slot, ignored",
            Self::LowContrast => "text on background is below the 4.5 to 1 contrast floor",
        }
    }
}

/// One finding, as a stable code plus where it was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    /// Dotted key path, sanitised for display.
    pub at: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.at.is_empty() {
            f.write_str(self.code.text())
        } else {
            write!(f, "{}: {}", self.at, self.code.text())
        }
    }
}

/// A theme that loaded, with the warnings it produced.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<Diagnostic>,
}

/// Why a theme was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub Vec<Diagnostic>);

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Invalid {}

/// Remove control and direction-override characters and cap the length, so
/// the text is safe to show in a terminal or a label.
pub fn clean_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    *c,
                    '\u{061C}'
                        | '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2060}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        })
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// True for `[a-z0-9][a-z0-9-]*` of at most 64 bytes.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_color_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

struct Ctx {
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl Ctx {
    fn err(&mut self, code: Code, at: &str) {
        self.errors.push(Diagnostic {
            code,
            at: clean_text(at, 120),
        });
    }
    fn warn(&mut self, code: Code, at: &str) {
        self.warnings.push(Diagnostic {
            code,
            at: clean_text(at, 120),
        });
    }

    fn table<'a>(&mut self, parent: &'a Table, key: &str, at: &str) -> Option<&'a Table> {
        match parent.get(key) {
            None => None,
            Some(Value::Table(t)) => Some(t),
            Some(_) => {
                self.err(Code::WrongType, at);
                None
            }
        }
    }

    fn text(&mut self, parent: &Table, key: &str, at: &str, max: usize) -> Option<String> {
        match parent.get(key) {
            None => None,
            Some(Value::String(s)) => Some(clean_text(s, max)),
            Some(_) => {
                self.err(Code::WrongType, at);
                None
            }
        }
    }

    fn unknown_keys(&mut self, table: &Table, known: &[&str], at: &str) {
        for key in table.keys() {
            if !known.contains(&key.as_str()) {
                self.warn(Code::UnknownKey, &join(at, key));
            }
        }
    }

    /// A colour literal or a one-level `@name` reference.
    fn color(
        &mut self,
        value: &Value,
        palette: &BTreeMap<String, Color>,
        at: &str,
    ) -> Option<Color> {
        let Value::String(s) = value else {
            self.err(Code::WrongType, at);
            return None;
        };
        let color = if let Some(name) = s.strip_prefix('@') {
            match palette.get(name) {
                Some(c) => *c,
                None => {
                    self.err(Code::UnknownReference, at);
                    return None;
                }
            }
        } else {
            match Color::parse(s) {
                Ok(c) => c,
                Err(_) => {
                    self.err(Code::BadColor, at);
                    return None;
                }
            }
        };
        if !color.is_opaque() {
            self.err(Code::AlphaNotAllowed, at);
            return None;
        }
        Some(color)
    }

    /// A table of colours restricted to `roles`. Unknown keys warn.
    fn roles(
        &mut self,
        parent: &Table,
        key: &str,
        roles: &[&'static str],
        palette: &BTreeMap<String, Color>,
        at: &str,
        unknown: Code,
    ) -> BTreeMap<&'static str, Color> {
        let mut out = BTreeMap::new();
        let at = join(at, key);
        let Some(table) = self.table(parent, key, &at) else {
            return out;
        };
        for (name, value) in table {
            let here = join(&at, name);
            match roles.iter().find(|r| **r == name.as_str()) {
                Some(role) => {
                    if let Some(c) = self.color(value, palette, &here) {
                        out.insert(*role, c);
                    }
                }
                None => self.warn(unknown, &here),
            }
        }
        out
    }
}

fn join(at: &str, key: &str) -> String {
    if at.is_empty() {
        key.to_owned()
    } else {
        format!("{at}.{key}")
    }
}

/// Parse and validate a theme file.
pub fn parse_theme(src: &str) -> Result<Loaded, Invalid> {
    let fail = |code| {
        Err(Invalid(vec![Diagnostic {
            code,
            at: String::new(),
        }]))
    };
    if src.len() > MAX_FILE_BYTES {
        return fail(Code::TooLarge);
    }
    let Ok(root) = src.parse::<Table>() else {
        return fail(Code::Syntax);
    };

    // Versions first: a newer file is refused before anything else is read.
    match root.get("schema") {
        Some(Value::Integer(n)) if *n == SCHEMA => {}
        Some(Value::Integer(n)) if *n > SCHEMA => return fail(Code::NewerSchema),
        _ => return fail(Code::BadSchema),
    }
    let derive = match root.get("derive") {
        None => DERIVE,
        Some(Value::Integer(n)) if (1..=DERIVE).contains(n) => *n,
        _ => return fail(Code::NewerDerive),
    };

    let mut cx = Ctx {
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    cx.unknown_keys(&root, &["schema", "derive", "meta", "variants"], "");

    let empty = Table::new();
    let meta = cx.table(&root, "meta", "meta").unwrap_or(&empty);
    cx.unknown_keys(
        meta,
        &["id", "name", "author", "license", "description"],
        "meta",
    );
    let id = match meta.get("id") {
        Some(Value::String(s)) if is_slug(s) => s.clone(),
        Some(Value::String(_)) => {
            cx.err(Code::BadSlug, "meta.id");
            String::new()
        }
        Some(_) => {
            cx.err(Code::WrongType, "meta.id");
            String::new()
        }
        None => {
            cx.err(Code::Missing, "meta.id");
            String::new()
        }
    };
    let name = cx
        .text(meta, "name", "meta.name", MAX_NAME)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.clone());
    let author = cx
        .text(meta, "author", "meta.author", MAX_NAME)
        .unwrap_or_default();
    let license = cx
        .text(meta, "license", "meta.license", MAX_NAME)
        .unwrap_or_default();
    let description = cx
        .text(meta, "description", "meta.description", MAX_DESCRIPTION)
        .unwrap_or_default();

    let mut variants = BTreeMap::new();
    match cx.table(&root, "variants", "variants") {
        Some(table) if table.len() > MAX_VARIANTS => cx.err(Code::TooMany, "variants"),
        Some(table) if !table.is_empty() => {
            for (key, value) in table {
                let at = join("variants", key);
                if !is_slug(key) {
                    cx.err(Code::BadSlug, &at);
                    continue;
                }
                let Value::Table(v) = value else {
                    cx.err(Code::WrongType, &at);
                    continue;
                };
                if let Some(variant) = variant(&mut cx, v, &at, &name) {
                    variants.insert(key.clone(), variant);
                }
            }
        }
        _ => cx.err(Code::NoVariants, "variants"),
    }

    if !cx.errors.is_empty() {
        return Err(Invalid(cx.errors));
    }
    Ok(Loaded {
        theme: Theme {
            id,
            name,
            author,
            license,
            description,
            derive,
            variants,
        },
        warnings: cx.warnings,
    })
}

fn variant(cx: &mut Ctx, v: &Table, at: &str, theme_name: &str) -> Option<Variant> {
    let before = cx.errors.len();
    cx.unknown_keys(
        v,
        &[
            "name",
            "appearance",
            "palette",
            "ui",
            "ansi",
            "syntax",
            "diff",
            "diagnostic",
            "slots",
            "suggests",
        ],
        at,
    );

    let name = cx
        .text(v, "name", &join(at, "name"), MAX_NAME)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| theme_name.to_owned());

    let appearance = match v.get("appearance") {
        Some(Value::String(s)) if s == "dark" => Appearance::Dark,
        Some(Value::String(s)) if s == "light" => Appearance::Light,
        Some(_) => {
            cx.err(Code::BadAppearance, &join(at, "appearance"));
            Appearance::Dark
        }
        None => {
            cx.err(Code::Missing, &join(at, "appearance"));
            Appearance::Dark
        }
    };

    // The palette holds literals only, so references are one level deep.
    let mut palette = BTreeMap::new();
    let palette_at = join(at, "palette");
    if let Some(table) = cx.table(v, "palette", &palette_at) {
        if table.len() > MAX_PALETTE {
            cx.err(Code::TooMany, &palette_at);
        } else {
            for (key, value) in table {
                let here = join(&palette_at, key);
                if !is_color_name(key) {
                    cx.err(Code::BadSlug, &here);
                    continue;
                }
                match value {
                    Value::String(s) => match Color::parse(s) {
                        Ok(c) => {
                            palette.insert(key.clone(), c);
                        }
                        Err(_) => cx.err(Code::BadColor, &here),
                    },
                    _ => cx.err(Code::WrongType, &here),
                }
            }
        }
    }

    let empty = Table::new();
    let ui_at = join(at, "ui");
    let ui_table = cx.table(v, "ui", &ui_at).unwrap_or(&empty);
    let known_ui: Vec<&str> = UI_REQUIRED.iter().chain(&UI_OPTIONAL).copied().collect();
    cx.unknown_keys(ui_table, &known_ui, &ui_at);
    let required = |cx: &mut Ctx, key: &str| match ui_table.get(key) {
        Some(value) => cx.color(value, &palette, &join(&ui_at, key)),
        None => {
            cx.err(Code::Missing, &join(&ui_at, key));
            None
        }
    };
    let bg = required(cx, "bg");
    let fg = required(cx, "fg");
    let accent = required(cx, "accent");

    let ansi_at = join(at, "ansi");
    let mut ansi = [Color::rgb(0, 0, 0); 16];
    match cx.table(v, "ansi", &ansi_at) {
        Some(table) => {
            cx.unknown_keys(table, &ANSI_NAMES, &ansi_at);
            for (i, key) in ANSI_NAMES.iter().enumerate() {
                let here = join(&ansi_at, key);
                match table.get(*key) {
                    Some(value) => {
                        if let Some(c) = cx.color(value, &palette, &here) {
                            ansi[i] = c;
                        }
                    }
                    None => cx.err(Code::Missing, &here),
                }
            }
        }
        None => {
            if !v.contains_key("ansi") {
                cx.err(Code::Missing, &ansi_at);
            }
        }
    }

    let syntax = cx.roles(v, "syntax", &SYNTAX_ROLES, &palette, at, Code::UnknownKey);
    let diff = cx.roles(v, "diff", &DIFF_ROLES, &palette, at, Code::UnknownKey);
    let diagnostic = cx.roles(
        v,
        "diagnostic",
        &DIAGNOSTIC_ROLES,
        &palette,
        at,
        Code::UnknownKey,
    );

    let mut slots = BTreeMap::new();
    let slots_at = join(at, "slots");
    if let Some(table) = cx.table(v, "slots", &slots_at) {
        for key in table.keys() {
            match SLOTS.iter().find(|(target, _)| *target == key.as_str()) {
                Some((target, names)) => {
                    let values =
                        cx.roles(table, key, names, &palette, &slots_at, Code::UnknownSlot);
                    slots.insert(*target, values);
                }
                None => cx.warn(Code::UnknownSlot, &join(&slots_at, key)),
            }
        }
    }

    let mut suggests = BTreeMap::new();
    let suggests_at = join(at, "suggests");
    if let Some(table) = cx.table(v, "suggests", &suggests_at) {
        cx.unknown_keys(table, &SUGGESTS, &suggests_at);
        for key in SUGGESTS {
            if let Some(text) = cx.text(table, key, &join(&suggests_at, key), MAX_NAME) {
                suggests.insert(key, text);
            }
        }
    }

    if cx.errors.len() > before {
        return None;
    }
    let (bg, fg, accent) = (bg?, fg?, accent?);

    let mut derived = Vec::new();
    let mut optional = |cx: &mut Ctx, key: &'static str, fallback: Color| {
        match ui_table.get(key) {
            Some(value) => cx.color(value, &palette, &join(&ui_at, key)),
            None => {
                derived.push(key);
                Some(fallback)
            }
        }
        .unwrap_or(fallback)
    };
    let d = derive_v1(appearance, bg, fg, accent, &ansi);
    let ui = Ui {
        bg,
        fg,
        accent,
        bg_dim: optional(cx, "bg_dim", d.bg_dim),
        surface: optional(cx, "surface", d.surface),
        border: optional(cx, "border", d.border),
        fg_muted: optional(cx, "fg_muted", d.fg_muted),
        fg_subtle: optional(cx, "fg_subtle", d.fg_subtle),
        accent_fg: optional(cx, "accent_fg", d.accent_fg),
        selection_bg: optional(cx, "selection_bg", d.selection_bg),
        selection_fg: optional(cx, "selection_fg", d.selection_fg),
        cursor: optional(cx, "cursor", d.cursor),
        cursor_fg: optional(cx, "cursor_fg", d.cursor_fg),
        link: optional(cx, "link", d.link),
        success: optional(cx, "success", d.success),
        warning: optional(cx, "warning", d.warning),
        error: optional(cx, "error", d.error),
    };
    if cx.errors.len() > before {
        return None;
    }
    if ui.fg.contrast(ui.bg) < CONTRAST_FLOOR {
        cx.warn(Code::LowContrast, &ui_at);
    }

    Some(Variant {
        name,
        appearance,
        palette,
        ui,
        derived,
        ansi,
        syntax,
        diff,
        diagnostic,
        slots,
        suggests,
    })
}

/// Below this ratio of text to background, applying needs explicit consent.
pub const CONTRAST_FLOOR: f64 = 4.5;

/// Derivation rule 1. Frozen: a better rule becomes rule 2.
fn derive_v1(
    appearance: Appearance,
    bg: Color,
    fg: Color,
    accent: Color,
    ansi: &[Color; 16],
) -> Ui {
    let black = Color::rgb(0, 0, 0);
    let accent_fg = if accent.contrast(bg) >= accent.contrast(fg) {
        bg
    } else {
        fg
    };
    Ui {
        bg,
        fg,
        accent,
        bg_dim: match appearance {
            Appearance::Dark => bg.mix(black, 0.25),
            Appearance::Light => bg.mix(black, 0.04),
        },
        surface: bg.mix(fg, 0.08),
        border: bg.mix(fg, 0.18),
        fg_muted: fg.mix(bg, 0.25),
        fg_subtle: fg.mix(bg, 0.5),
        accent_fg,
        selection_bg: bg.mix(accent, 0.35),
        selection_fg: fg,
        cursor: fg,
        cursor_fg: bg,
        link: accent,
        success: ansi[2],
        warning: ansi[3],
        error: ansi[1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled;

    const MINIMAL: &str = r##"
schema = 1
[meta]
id = "mini"
[variants.default]
appearance = "light"
[variants.default.palette]
ink = "#202020"
[variants.default.ui]
bg = "#fafafa"
fg = "@ink"
accent = "#3584e4"
[variants.default.ansi]
black = "#000000"
red = "#c01c28"
green = "#26a269"
yellow = "#a2734c"
blue = "#12488b"
magenta = "#a347ba"
cyan = "#2aa1b3"
white = "#d0cfcc"
bright_black = "#5e5c64"
bright_red = "#f66151"
bright_green = "#33d17a"
bright_yellow = "#e9ad0c"
bright_blue = "#2a7bde"
bright_magenta = "#c061cb"
bright_cyan = "#33c7de"
bright_white = "#ffffff"
"##;

    fn codes(src: &str) -> Vec<(Code, String)> {
        parse_theme(src)
            .unwrap_err()
            .0
            .into_iter()
            .map(|d| (d.code, d.at))
            .collect()
    }

    #[test]
    fn bundled_themes_load_without_warnings_and_derive_nothing() {
        for (id, src) in bundled::SOURCES {
            let loaded = parse_theme(src).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(loaded.theme.id, id);
            assert!(loaded.warnings.is_empty(), "{id}: {:?}", loaded.warnings);
            let v = &loaded.theme.variants["default"];
            assert!(v.derived.is_empty(), "{id} leaves roles to derivation");
            assert_eq!(v.palette.len(), 45);
            assert_eq!(v.syntax.len(), SYNTAX_ROLES.len());
        }
        assert_eq!(bundled::themes().len(), bundled::SOURCES.len());
    }

    #[test]
    fn minimal_theme_derives_the_optional_roles() {
        let loaded = parse_theme(MINIMAL).unwrap();
        let v = &loaded.theme.variants["default"];
        assert_eq!(loaded.theme.name, "mini");
        assert_eq!(v.appearance, Appearance::Light);
        assert_eq!(v.ui.fg, Color::rgb(0x20, 0x20, 0x20));
        assert_eq!(v.derived, UI_OPTIONAL);
        assert_eq!(v.ui.cursor, v.ui.fg);
        assert_eq!(v.ui.error, v.ansi[1]);
        // Frozen values of rule 1.
        assert_eq!(v.ui.surface.hex(), "#e9e9e9");
        assert_eq!(v.ui.selection_bg.hex(), "#b5d1f2");
    }

    #[test]
    fn versions_are_checked_before_anything_else() {
        assert_eq!(
            codes("schema = 2\nnonsense = 1"),
            [(Code::NewerSchema, String::new())]
        );
        assert_eq!(
            codes("[meta]\nid = \"x\""),
            [(Code::BadSchema, String::new())]
        );
        assert_eq!(codes("schema = \"1\""), [(Code::BadSchema, String::new())]);
        assert_eq!(
            codes("schema = 1\nderive = 2"),
            [(Code::NewerDerive, String::new())]
        );
        assert_eq!(codes("schema = 1\n[meta"), [(Code::Syntax, String::new())]);
        let big = format!("schema = 1\n#{}", "x".repeat(MAX_FILE_BYTES));
        assert_eq!(codes(&big), [(Code::TooLarge, String::new())]);
    }

    #[test]
    fn reports_every_problem_with_its_location() {
        let src = MINIMAL
            .replace("id = \"mini\"", "id = \"Mini Theme\"")
            .replace("fg = \"@ink\"", "fg = \"@nope\"")
            .replace("accent = \"#3584e4\"", "accent = \"#3584e480\"")
            .replace("red = \"#c01c28\"\n", "")
            .replace("blue = \"#12488b\"", "blue = \"blue\"");
        assert_eq!(
            codes(&src),
            [
                (Code::BadSlug, "meta.id".to_owned()),
                (Code::UnknownReference, "variants.default.ui.fg".to_owned()),
                (
                    Code::AlphaNotAllowed,
                    "variants.default.ui.accent".to_owned()
                ),
                (Code::Missing, "variants.default.ansi.red".to_owned()),
                (Code::BadColor, "variants.default.ansi.blue".to_owned()),
            ]
        );
    }

    #[test]
    fn references_are_one_level_and_palette_holds_literals() {
        let src = MINIMAL.replace("ink = \"#202020\"", "ink = \"@ink\"");
        assert_eq!(
            codes(&src)[0],
            (Code::BadColor, "variants.default.palette.ink".to_owned())
        );
    }

    #[test]
    fn unknown_keys_and_slots_warn_but_load() {
        let src = format!(
            "{MINIMAL}\n[variants.default.slots.starship]\ndirectory = \"#112233\"\nfuture = \"#112233\"\n\
             [variants.default.slots.someapp]\nx = \"#112233\"\n[variants.default.syntax]\nkeyword = \"@ink\"\nlambda = \"@ink\"\n\
             [future_section]\nx = 1\n"
        );
        let loaded = parse_theme(&src).unwrap();
        let mut warned: Vec<_> = loaded
            .warnings
            .iter()
            .map(|d| (d.code, d.at.as_str()))
            .collect();
        warned.sort_by_key(|(_, at)| *at);
        assert_eq!(
            warned,
            [
                (Code::UnknownKey, "future_section"),
                (Code::UnknownSlot, "variants.default.slots.someapp"),
                (Code::UnknownSlot, "variants.default.slots.starship.future"),
                (Code::UnknownKey, "variants.default.syntax.lambda"),
            ]
        );
        let v = &loaded.theme.variants["default"];
        assert_eq!(v.slots["starship"].len(), 1);
        assert_eq!(v.syntax.len(), 1);
    }

    #[test]
    fn low_contrast_is_a_warning() {
        let src = MINIMAL.replace("ink = \"#202020\"", "ink = \"#e0e0e0\"");
        let loaded = parse_theme(&src).unwrap();
        assert_eq!(loaded.warnings[0].code, Code::LowContrast);
    }

    #[test]
    fn display_text_is_cleaned_and_capped() {
        assert_eq!(clean_text("a\u{1b}[31mb\nc\u{202e}d", 99), "a[31mbcd");
        assert_eq!(clean_text(&"x".repeat(500), 64).len(), 64);
        let src = MINIMAL.replace(
            "id = \"mini\"",
            "id = \"mini\"\nname = \"Ni\\u202Ece\\nTheme\"",
        );
        assert_eq!(parse_theme(&src).unwrap().theme.name, "NiceTheme");
    }

    #[test]
    fn slugs() {
        for ok in ["a", "aetherius", "base16-ocean", "3024"] {
            assert!(is_slug(ok), "{ok}");
        }
        for bad in ["", "-a", "A", "a b", "a/b", "..", "a_b", &"a".repeat(65)] {
            assert!(!is_slug(bad), "{bad}");
        }
    }
}
