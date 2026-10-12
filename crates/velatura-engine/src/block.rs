//! Managed blocks: the span Velatura adds to a file the user maintains.
//!
//! A block is a begin marker line, the body, and an end marker line. The file
//! is edited as bytes: its encoding, line endings and everything outside the
//! block are left exactly as they were.

/// Bumped if the marker text ever changes.
pub const FORMAT: u32 = 1;

fn begin(target: &str) -> String {
    format!("# >>> velatura {target} {FORMAT} >>>")
}

fn end(target: &str) -> String {
    format!("# <<< velatura {target} {FORMAT} <<<")
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
    for (i, (_, _, text)) in all.iter().enumerate() {
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
                start: all[*b].0,
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

/// Add the block at the end of the file, or replace the body of the existing
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
}
