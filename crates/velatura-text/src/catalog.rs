//! Translations, compiled into the program.
//!
//! The catalogs in `po/` are read as they are, so the same text is found
//! from a build folder, an install and a package, with nothing to install
//! beside the program and no library to load it.

use std::collections::HashMap;
use std::sync::OnceLock;

/// `(language, the catalog's text)`, one per line of `po/LINGUAS`.
const SOURCES: [(&str, &str); 1] = [("es", include_str!("../../../po/es.po"))];

#[derive(Debug, Default)]
pub struct Catalog {
    one: HashMap<String, String>,
    /// Singular source text to every form the language has.
    many: HashMap<String, Vec<String>>,
}

/// The text between the quotes of one line of a catalog, unescaped.
fn unquote(line: &str) -> Option<String> {
    let inner = line.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            other => out.push(other),
        }
    }
    Some(out)
}

impl Catalog {
    /// Read a catalog. Entries marked fuzzy, and ones with no translation,
    /// are left out, so the source text is shown for them.
    pub fn parse(text: &str) -> Self {
        #[derive(Default)]
        struct Entry {
            id: String,
            forms: Vec<String>,
            plural: bool,
            fuzzy: bool,
        }
        let mut catalog = Self::default();
        let mut entry = Entry::default();
        // Which string a continuation line belongs to: 0 the source text,
        // 1 its plural, 2 and up the translations.
        let mut slot = 0;
        let mut finish = |entry: &mut Entry| {
            let e = std::mem::take(entry);
            if e.id.is_empty() || e.fuzzy || e.forms.iter().any(String::is_empty) {
                return;
            }
            if e.plural {
                catalog.many.insert(e.id, e.forms);
            } else if let Some(first) = e.forms.into_iter().next() {
                catalog.one.insert(e.id, first);
            }
        };
        for line in text.lines().chain(std::iter::once("")) {
            let line = line.trim();
            if line.is_empty() {
                finish(&mut entry);
                slot = 0;
            } else if let Some(flags) = line.strip_prefix("#,") {
                entry.fuzzy |= flags.contains("fuzzy");
            } else if line.starts_with('#') {
            } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
                entry.plural = true;
                slot = 1;
                let _ = rest;
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                entry.id = unquote(rest).unwrap_or_default();
                slot = 0;
            } else if let Some(rest) = line.strip_prefix("msgstr") {
                let rest =
                    rest.trim_start_matches(|c: char| c == '[' || c == ']' || c.is_ascii_digit());
                entry.forms.push(unquote(rest).unwrap_or_default());
                slot = 2;
            } else if let Some(more) = unquote(line) {
                match slot {
                    0 => entry.id.push_str(&more),
                    1 => {}
                    _ => {
                        if let Some(last) = entry.forms.last_mut() {
                            last.push_str(&more);
                        }
                    }
                }
            }
        }
        catalog
    }

    pub fn find<'a>(&'a self, text: &'a str) -> &'a str {
        self.one.get(text).map_or(text, String::as_str)
    }

    /// The form for `n`. Every language here has two forms, chosen as in
    /// English; one that needs more gets its own rule when it is added.
    pub fn find_plural<'a>(&'a self, one: &'a str, many: &'a str, n: usize) -> &'a str {
        let index = usize::from(n != 1);
        match self.many.get(one).and_then(|forms| forms.get(index)) {
            Some(form) => form,
            None if n == 1 => one,
            None => many,
        }
    }

    pub fn len(&self) -> usize {
        self.one.len() + self.many.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The catalog for a language, if there is one: `es` for `es`, `es_MX`,
/// `es_ES.UTF-8` and so on.
pub fn for_language(tag: &str) -> Option<&'static Catalog> {
    static ALL: OnceLock<Vec<(&'static str, Catalog)>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        SOURCES
            .iter()
            .map(|(lang, text)| (*lang, Catalog::parse(text)))
            .collect()
    });
    let tag = tag.split(['.', '@']).next().unwrap_or(tag);
    let short = tag.split('_').next().unwrap_or(tag);
    all.iter()
        .find(|(lang, _)| *lang == tag)
        .or_else(|| all.iter().find(|(lang, _)| *lang == short))
        .map(|(_, catalog)| catalog)
}

/// The language the session asks for, the way gettext reads it: nothing
/// under the C locale; otherwise the first of `LANGUAGE` there is a
/// catalog for, or the locale's own language.
pub fn from_environment(var: &dyn Fn(&str) -> Option<String>) -> Option<&'static Catalog> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    let locale = set("LC_ALL")
        .or_else(|| set("LC_MESSAGES"))
        .or_else(|| set("LANG"))?;
    if locale == "C" || locale == "POSIX" || locale.starts_with("C.") {
        return None;
    }
    match set("LANGUAGE") {
        Some(list) => list.split(':').find_map(for_language),
        None => for_language(&locale),
    }
}
