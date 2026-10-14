//! Managed blocks: the span Velatura adds to a file the user maintains.
//!
//! A block is a begin marker line, the body, and an end marker line. The file
//! is edited as bytes: its encoding, line endings and everything outside the
//! block are left exactly as they were.

/// Bumped if the marker text ever changes.
pub const FORMAT: u32 = 1;

/// How a comment is written in the file a target's block lives in: CSS for
/// the GTK targets and Firefox's stylesheet, JavaScript for Firefox's
/// preferences, a leading `#` everywhere else.
fn comment(target: &str) -> (&'static str, &'static str) {
    match target {
        FIREFOX_PREFS => ("// ", ""),
        FIREFOX => ("/* ", " */"),
        t if t.starts_with("gtk") => ("/* ", " */"),
        _ => ("# ", ""),
    }
}

/// The blocks of the Firefox target: one in `userChrome.css`, one in
/// `user.js`.
pub const FIREFOX: &str = "firefox";
pub const FIREFOX_PREFS: &str = "firefox-prefs";

const BOM: &[u8] = b"\xef\xbb\xbf";

/// True for a block that goes at the top of its file, not the end. In a
/// TOML file, keys of the file's root have to come before its first table;
/// in a stylesheet, an `@import` has to come before every rule.
pub fn at_top(target: &str) -> bool {
    matches!(target, "starship" | FIREFOX)
}

fn begin(target: &str) -> String {
    let (open, close) = comment(target);
    format!("{open}>>> velatura {target} {FORMAT} >>>{close}")
}

fn end(target: &str) -> String {
    let (open, close) = comment(target);
    format!("{open}<<< velatura {target} {FORMAT} <<<{close}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Absent,
    /// Byte range of the whole block including both marker lines and the end
    /// marker's line ending, and the body without markers.
    Present {
        start: usize,
        end: usize,
        body: String,
    },
    /// One marker without the other, markers out of order, or more than one
    /// block. The file is not touched in this state.
    Malformed,
}

/// The file's line ending: that of its first line, `\n` if it has none.
fn eol(bytes: &[u8]) -> &'static str {
    match bytes.iter().position(|b| *b == b'\n') {
        Some(i) if i > 0 && bytes[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// `(start, end_with_eol, text_without_eol)` for each line.
fn lines(bytes: &[u8]) -> Vec<(usize, usize, &[u8])> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let stop = bytes[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |i| start + i + 1);
        let mut text = &bytes[start..stop];
        if let Some(t) = text.strip_suffix(b"\n") {
            text = t.strip_suffix(b"\r").unwrap_or(t);
        }
        out.push((start, stop, text));
        start = stop;
    }
    out
}

pub fn find(bytes: &[u8], target: &str) -> Found {
    let (begin, end) = (begin(target), end(target));
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let all = lines(bytes);
    // A byte order mark is not part of the first line's text.
    let mark = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
    for (i, (start, _, text)) in all.iter().enumerate() {
        let text = if *start == 0 { &text[mark..] } else { text };
        let trimmed = text.trim_ascii();
        if trimmed == begin.as_bytes() {
            begins.push(i);
        } else if trimmed == end.as_bytes() {
            ends.push(i);
        }
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Found::Absent,
        ([b], [e]) if b < e => {
            let body: Vec<&str> = all[b + 1..*e]
                .iter()
                .map(|(_, _, text)| std::str::from_utf8(text).unwrap_or(""))
                .collect();
            Found::Present {
                start: all[*b].0.max(mark),
                end: all[*e].1,
                body: body.join("\n"),
            }
        }
        _ => Found::Malformed,
    }
}

/// The result of putting a block into a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upsert {
    pub content: Vec<u8>,
    /// A line ending had to be added before the block because the file did
    /// not end with one. Removal takes it out again.
    pub added_eol: bool,
    /// The body the block had before, if there was a block.
    pub before: Option<String>,
}

/// Add the block at the end of the file (at the top for a target that needs
/// it there, after a byte order mark if there is one), or replace the body of the existing
/// one where it stands. `None` when the file's markers are malformed.
pub fn upsert(bytes: &[u8], target: &str, body: &str) -> Option<Upsert> {
    let eol = eol(bytes);
    let mut block = String::new();
    for line in std::iter::once(begin(target).as_str())
        .chain(body.lines())
        .chain(std::iter::once(end(target).as_str()))
    {
        block.push_str(line);
        block.push_str(eol);
    }
    match find(bytes, target) {
        Found::Malformed => None,
        Found::Present { start, end, body } => {
            let mut content = bytes[..start].to_vec();
            // Keep the file's final-newline state if the block is its last line.
            let block = if end == bytes.len() && !bytes.ends_with(b"\n") {
                block.trim_end_matches(['\r', '\n'])
            } else {
                block.as_str()
            };
            content.extend_from_slice(block.as_bytes());
            content.extend_from_slice(&bytes[end..]);
            Some(Upsert {
                content,
                added_eol: false,
                before: Some(body),
            })
        }
        Found::Absent if at_top(target) => {
            let mark = if bytes.starts_with(b"\xef\xbb\xbf") {
                3
            } else {
                0
            };
            let mut content = bytes[..mark].to_vec();
            content.extend_from_slice(block.as_bytes());
            content.extend_from_slice(&bytes[mark..]);
            Some(Upsert {
                content,
                added_eol: false,
                before: None,
            })
        }
        Found::Absent => {
            let added_eol = !bytes.is_empty() && !bytes.ends_with(b"\n");
            let mut content = bytes.to_vec();
            if added_eol {
                content.extend_from_slice(eol.as_bytes());
            }
            content.extend_from_slice(block.as_bytes());
            Some(Upsert {
                content,
                added_eol,
                before: None,
            })
        }
    }
}

/// Take the block out of the file as it is now. `added_eol` is what
/// [`upsert`] reported when the block was first added. `None` when there is
/// no well-formed block to remove.
pub fn remove(bytes: &[u8], target: &str, added_eol: bool) -> Option<Vec<u8>> {
    let Found::Present { start, end, .. } = find(bytes, target) else {
        return None;
    };
    let mut content = bytes[..start].to_vec();
    if added_eol && end == bytes.len() {
        if content.ends_with(b"\r\n") {
            content.truncate(content.len() - 2);
        } else if content.ends_with(b"\n") {
            content.truncate(content.len() - 1);
        }
    }
    content.extend_from_slice(&bytes[end..]);
    Some(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "include velatura.conf";

    #[test]
    fn a_block_at_the_top_is_added_and_removed_byte_exact() {
        let body = "palette = \"velatura\"\npalettes.velatura.bg = \"#000000\"";
        let files: [&[u8]; 5] = [
            b"",
            b"format = \"$all\"\n[character]\nx = 1\n",
            b"[character]\nx = 1",
            b"\xef\xbb\xbfa = 1\r\n[b]\r\n",
            b"\n",
        ];
        for original in files {
            let up = upsert(original, "starship", body).unwrap();
            assert!(up.before.is_none() && !up.added_eol);
            let Found::Present { start, .. } = find(&up.content, "starship") else {
                panic!("no block in {:?}", String::from_utf8_lossy(&up.content));
            };
            assert_eq!(start, if original.starts_with(b"\xef") { 3 } else { 0 });
            assert!(up.content.ends_with(&original[start..]));
            // Updating it leaves it where it is.
            let again = upsert(&up.content, "starship", "palette = \"velatura\"").unwrap();
            assert_eq!(again.before.as_deref(), Some(body));
            assert!(
                matches!(find(&again.content, "starship"), Found::Present { start: s, .. } if s == start)
            );
            assert_eq!(remove(&up.content, "starship", false).unwrap(), original);
            assert_eq!(remove(&again.content, "starship", false).unwrap(), original);
        }
    }

    #[test]
    fn add_then_remove_is_byte_exact() {
        let files: [&[u8]; 9] = [
            b"",
            b"\n",
            b"font_size 12\n",
            b"font_size 12",
            b"a\r\nb\r\n",
            b"a\r\nb",
            b"\xff\xfe latin-1 \xe9\n",
            b"\n\n\n",
            b"# comment only",
        ];
        for original in files {
            let up = upsert(original, "kitty", BODY).unwrap();
            assert!(up.before.is_none());
            assert_eq!(
                up.added_eol,
                !original.is_empty() && !original.ends_with(b"\n")
            );
            assert!(up.content.starts_with(original));
            assert!(matches!(find(&up.content, "kitty"), Found::Present { .. }));
            assert_eq!(
                remove(&up.content, "kitty", up.added_eol).unwrap(),
                original,
                "{:?}",
                String::from_utf8_lossy(original)
            );
        }
    }

    #[test]
    fn uses_the_files_line_ending() {
        let up = upsert(b"a\r\nb\r\n", "kitty", BODY).unwrap();
        assert_eq!(
            up.content,
            b"a\r\nb\r\n# >>> velatura kitty 1 >>>\r\ninclude velatura.conf\r\n# <<< velatura kitty 1 <<<\r\n"
        );
    }

    #[test]
    fn replaces_in_place_and_reports_the_old_body() {
        let first = upsert(b"top\n", "kitty", "old line").unwrap().content;
        let mut edited = first.clone();
        edited.extend_from_slice(b"user line after\n");
        let up = upsert(&edited, "kitty", BODY).unwrap();
        assert_eq!(up.before.as_deref(), Some("old line"));
        assert!(!up.added_eol);
        assert_eq!(
            String::from_utf8(up.content.clone()).unwrap(),
            "top\n# >>> velatura kitty 1 >>>\ninclude velatura.conf\n# <<< velatura kitty 1 <<<\nuser line after\n"
        );
        // Same body again changes nothing.
        assert_eq!(
            upsert(&up.content, "kitty", BODY).unwrap().content,
            up.content
        );
    }

    #[test]
    fn removal_keeps_edits_made_around_the_block() {
        let up = upsert(b"font_size 12", "kitty", BODY).unwrap();
        let mut edited = b"# new first line\n".to_vec();
        edited.extend_from_slice(&up.content);
        edited.extend_from_slice(b"map f1 new_window\n");
        assert_eq!(
            remove(&edited, "kitty", up.added_eol).unwrap(),
            b"# new first line\nfont_size 12\nmap f1 new_window\n"
        );
    }

    #[test]
    fn malformed_markers_are_refused() {
        let b = "# >>> velatura kitty 1 >>>\n";
        let e = "# <<< velatura kitty 1 <<<\n";
        for bad in [
            b.to_owned(),
            e.to_owned(),
            format!("{e}x\n{b}"),
            format!("{b}x\n{e}{b}y\n{e}"),
            format!("{b}{b}x\n{e}"),
            format!("{b}x\n{e}{e}"),
        ] {
            assert_eq!(find(bad.as_bytes(), "kitty"), Found::Malformed, "{bad:?}");
            assert!(upsert(bad.as_bytes(), "kitty", BODY).is_none());
            assert!(remove(bad.as_bytes(), "kitty", false).is_none());
        }
    }

    #[test]
    fn other_targets_and_lookalikes_are_ignored() {
        let text = "# >>> velatura ghostty 1 >>>\nx\n# <<< velatura ghostty 1 <<<\n#>>> velatura kitty 1 >>>\n";
        assert_eq!(find(text.as_bytes(), "kitty"), Found::Absent);
        assert!(matches!(
            find(text.as_bytes(), "ghostty"),
            Found::Present { .. }
        ));
        // Indented markers still count, so a reformatted file is recognised.
        let indented = "  # >>> velatura kitty 1 >>>  \nx\n\t# <<< velatura kitty 1 <<<\n";
        assert!(matches!(
            find(indented.as_bytes(), "kitty"),
            Found::Present { start: 0, .. }
        ));
    }

    #[test]
    fn css_files_get_css_comments() {
        let original = b"@define-color window_bg_color #000;\n";
        let up = upsert(original, "gtk4", "@import url(\"velatura.css\");").unwrap();
        assert_eq!(
            String::from_utf8(up.content.clone()).unwrap(),
            "@define-color window_bg_color #000;\n/* >>> velatura gtk4 1 >>> */\n@import url(\"velatura.css\");\n/* <<< velatura gtk4 1 <<< */\n"
        );
        assert_eq!(remove(&up.content, "gtk4", up.added_eol).unwrap(), original);
        assert_eq!(find(&up.content, "gtk3"), Found::Absent);
    }

    #[test]
    fn firefox_gets_an_import_at_the_top_and_a_preference_at_the_end() {
        let sheet = b"#nav-bar { border: none; }\n";
        let up = upsert(sheet, FIREFOX, "@import url(\"velatura.css\");").unwrap();
        assert_eq!(
            String::from_utf8(up.content.clone()).unwrap(),
            "/* >>> velatura firefox 1 >>> */\n@import url(\"velatura.css\");\n/* <<< velatura firefox 1 <<< */\n#nav-bar { border: none; }\n"
        );
        assert_eq!(remove(&up.content, FIREFOX, up.added_eol).unwrap(), sheet);
        let prefs = b"user_pref(\"a\", 1);";
        let up = upsert(prefs, FIREFOX_PREFS, "user_pref(\"b\", true);").unwrap();
        assert_eq!(
            String::from_utf8(up.content.clone()).unwrap(),
            "user_pref(\"a\", 1);\n// >>> velatura firefox-prefs 1 >>>\nuser_pref(\"b\", true);\n// <<< velatura firefox-prefs 1 <<<\n"
        );
        assert_eq!(
            remove(&up.content, FIREFOX_PREFS, up.added_eol).unwrap(),
            prefs
        );
        // The two names do not find each other's block.
        assert_eq!(find(&up.content, FIREFOX), Found::Absent);
    }
}
