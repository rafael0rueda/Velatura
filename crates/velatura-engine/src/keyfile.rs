//! Editing one list in a key file (the `[group]` and `key=a;b;` format of
//! desktop files and of Flatpak's overrides) without touching anything
//! else in it.
//!
//! A key file has no place for a marker, so what Velatura added is known
//! from its own records: the members it put in the list. Taking them out
//! again leaves every other member, key, group, comment and blank line as
//! it was, byte for byte.

/// The result of adding members to a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub content: Vec<u8>,
    /// The members that were not there yet, in the order given.
    pub added: Vec<String>,
    /// What [`remove`] needs to give the file back exactly: what had to be
    /// made (the group, the key, or only members) and the members.
    pub receipt: String,
    /// True if a line ending had to be put after the file's last line.
    pub added_eol: bool,
}

fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Byte ranges of each line, without its line ending.
fn lines(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            let end = if i > start && text.as_bytes()[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            out.push((start, end));
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

/// The line holding `[group]`, and the line holding `key=` inside it.
fn locate(text: &str, group: &str, key: &str) -> (Option<usize>, Option<usize>) {
    let header = format!("[{group}]");
    let mut inside = None;
    for (n, (start, end)) in lines(text).into_iter().enumerate() {
        let line = text[start..end].trim();
        if line.starts_with('[') {
            if inside.is_some() {
                break;
            }
            if line == header {
                inside = Some(n);
            }
        } else if inside.is_some()
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            return (inside, Some(n));
        }
    }
    (inside, None)
}

fn members(value: &str) -> Vec<&str> {
    value.split(';').filter(|m| !m.is_empty()).collect()
}

/// Add each of `wanted` to the list `key` in `group` unless `same` says a
/// member already there stands for it. `None` if the file is not text.
pub fn add(
    content: &[u8],
    group: &str,
    key: &str,
    wanted: &[&str],
    same: &dyn Fn(&str, &str) -> bool,
) -> Option<Added> {
    let text = std::str::from_utf8(content).ok()?;
    let nl = eol(text);
    let all = lines(text);
    let (header, line) = locate(text, group, key);
    let mut out = String::with_capacity(text.len() + 96);
    let mut added_eol = false;
    let added: Vec<String>;
    let made: String = match (header, line) {
        (_, Some(n)) => {
            let (start, end) = all[n];
            let (name, value) = text[start..end].split_once('=')?;
            let present = members(value);
            added = wanted
                .iter()
                .filter(|w| !present.iter().any(|p| same(p, w)))
                .map(|w| (*w).to_owned())
                .collect();
            if added.is_empty() {
                return Some(Added {
                    content: content.to_vec(),
                    added,
                    receipt: String::new(),
                    added_eol,
                });
            }
            // The list keeps the way it ended: with a separator or without.
            let closed = value.trim_end().ends_with(';') || present.is_empty();
            let mut list: Vec<&str> = present;
            list.extend(added.iter().map(String::as_str));
            out.push_str(&text[..start]);
            out.push_str(name);
            out.push('=');
            out.push_str(&list.join(";"));
            if closed {
                out.push(';');
            }
            out.push_str(&text[end..]);
            // An empty list is put back the way it was written.
            if list.len() == added.len() {
                format!("members{value}")
            } else {
                "members".to_owned()
            }
        }
        (Some(n), None) => {
            added = wanted.iter().map(|w| (*w).to_owned()).collect();
            let (_, end) = all[n];
            let after = &text[end..];
            // Straight after the group's own line.
            let skip = if after.starts_with("\r\n") {
                2
            } else {
                usize::from(after.starts_with('\n'))
            };
            out.push_str(&text[..end]);
            out.push_str(nl);
            out.push_str(&format!("{key}={};", added.join(";")));
            // If the group's line was the last, with no line ending, so is
            // the new one.
            if skip > 0 {
                out.push_str(nl);
            }
            out.push_str(&after[skip..]);
            if skip > 0 { "key" } else { "key-noeol" }.to_owned()
        }
        (None, _) => {
            added = wanted.iter().map(|w| (*w).to_owned()).collect();
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(nl);
                added_eol = true;
            }
            out.push_str(&format!("[{group}]{nl}{key}={};{nl}", added.join(";")));
            "group".to_owned()
        }
    };
    Some(Added {
        content: out.into_bytes(),
        receipt: format!("{made}|{}", added.join(";")),
        added,
        added_eol,
    })
}

/// The members a receipt says were added.
fn ours(receipt: &str) -> (&str, Vec<&str>) {
    let (made, list) = receipt.split_once('|').unwrap_or(("members", receipt));
    (made, members(list))
}

/// True if everything the receipt says was added is still in the list.
pub fn holds(content: &[u8], group: &str, key: &str, receipt: &str) -> bool {
    let Ok(text) = std::str::from_utf8(content) else {
        return false;
    };
    let (_, Some(n)) = locate(text, group, key) else {
        return false;
    };
    let (start, end) = lines(text)[n];
    let present = members(text[start..end].split_once('=').map_or("", |(_, v)| v));
    ours(receipt).1.iter().all(|m| present.contains(m))
}

/// Take out what [`add`] put in, going by its receipt: the members, then
/// the key if `add` made it and it is empty again, then the group if `add`
/// made it and it holds nothing else. `added_eol` is what `add` reported.
/// `None` if the file is not text.
pub fn remove(
    content: &[u8],
    group: &str,
    key: &str,
    receipt: &str,
    added_eol: bool,
) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(content).ok()?;
    let all = lines(text);
    let (made, ours) = ours(receipt);
    let (Some(header), Some(n)) = locate(text, group, key) else {
        return Some(content.to_vec());
    };
    let (start, end) = all[n];
    let (name, value) = text[start..end].split_once('=')?;
    let left: Vec<&str> = members(value)
        .into_iter()
        .filter(|m| !ours.contains(m))
        .collect();
    let mut out = String::with_capacity(text.len());
    // A key that was there before stays, even with nothing left in it.
    if !left.is_empty() || made.starts_with("members") {
        out.push_str(&text[..start]);
        out.push_str(name);
        out.push('=');
        out.push_str(&left.join(";"));
        if left.is_empty() {
            out.push_str(made.trim_start_matches("members"));
        } else if value.trim_end().ends_with(';') {
            out.push(';');
        }
        out.push_str(&text[end..]);
        return Some(out.into_bytes());
    }
    // The key's whole line goes, with its line ending.
    let next = all.get(n + 1).map_or(text.len(), |(s, _)| *s);
    if made == "key-noeol" && end == text.len() && n > 0 {
        // It was put after a last line that had no ending; that is so again.
        out.push_str(&text[..all[n - 1].1]);
        return Some(out.into_bytes());
    }
    let group_now_empty = header + 1 == n
        && all
            .get(n + 1)
            .is_none_or(|(s, e)| text[*s..*e].trim_start().starts_with('['));
    if made == "group" && group_now_empty {
        out.push_str(&text[..all[header].0]);
        out.push_str(&text[next..]);
        if added_eol && next == text.len() && out.ends_with('\n') {
            out.pop();
            if out.ends_with('\r') {
                out.pop();
            }
        }
    } else {
        out.push_str(&text[..start]);
        out.push_str(&text[next..]);
    }
    Some(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WANT: [&str; 2] = ["xdg-config/gtk-3.0:ro", "xdg-config/gtk-4.0:ro"];
    const G: &str = "Context";
    const K: &str = "filesystems";

    fn exact(a: &str, b: &str) -> bool {
        a == b
    }

    /// Whatever the file looked like, adding and then removing gives it
    /// back byte for byte.
    #[test]
    fn add_then_remove_is_byte_exact() {
        let files: [&str; 13] = [
            "",
            "[Context]\n",
            "[Context]",
            "[Context]\nfilesystems=home;\n",
            "[Context]\nfilesystems=home\n",
            "[Context]\nfilesystems=\n",
            "[Context]\nfilesystems=;\n",
            "[Context]\nsockets=wayland;\nfilesystems=home;xdg-download:ro;\n\n[Environment]\nA=1\n",
            "[Environment]\nGTK_THEME=x\n",
            "[Environment]\nGTK_THEME=x",
            "# a comment\r\n[Context]\r\nsockets=x11;\r\n\r\n[Environment]\r\nA=1\r\n",
            "[Context]\n\n[Environment]\nA=1\n",
            "[Session Bus Policy]\norg.a.b=talk\n\n[Context]\ndevices=dri;\n",
        ];
        for file in files {
            let a = add(file.as_bytes(), G, K, &WANT, &exact).unwrap();
            assert_eq!(a.added, WANT, "{file:?}");
            let text = String::from_utf8(a.content.clone()).unwrap();
            assert!(holds(&a.content, G, K, &a.receipt), "{text:?}");
            // Adding again changes nothing and claims nothing.
            let again = add(&a.content, G, K, &WANT, &exact).unwrap();
            assert_eq!(again.content, a.content);
            assert!(again.added.is_empty());
            let back = remove(&a.content, G, K, &a.receipt, a.added_eol).unwrap();
            assert_eq!(String::from_utf8(back).unwrap(), file, "via {text:?}");
        }
    }

    #[test]
    fn writes_what_flatpak_itself_would() {
        let a = add(b"", G, K, &WANT, &exact).unwrap();
        assert_eq!(
            String::from_utf8(a.content).unwrap(),
            "[Context]\nfilesystems=xdg-config/gtk-3.0:ro;xdg-config/gtk-4.0:ro;\n"
        );
        let a = add(b"[Context]\nfilesystems=home;\n", G, K, &WANT, &exact).unwrap();
        assert_eq!(
            String::from_utf8(a.content).unwrap(),
            "[Context]\nfilesystems=home;xdg-config/gtk-3.0:ro;xdg-config/gtk-4.0:ro;\n"
        );
    }

    #[test]
    fn what_was_there_is_not_claimed_and_not_removed() {
        let file = b"[Context]\nfilesystems=xdg-config/gtk-4.0:ro;home;\n";
        let a = add(file, G, K, &WANT, &exact).unwrap();
        assert_eq!(a.added, ["xdg-config/gtk-3.0:ro"]);
        assert_eq!(a.receipt, "members|xdg-config/gtk-3.0:ro");
        let back = remove(&a.content, G, K, &a.receipt, a.added_eol).unwrap();
        assert_eq!(back, file);

        // Members and keys added by someone else afterwards stay.
        let a = add(b"", G, K, &WANT, &exact).unwrap();
        let edited = String::from_utf8(a.content.clone())
            .unwrap()
            .replace("gtk-4.0:ro;", "gtk-4.0:ro;xdg-music;")
            + "sockets=x11;\n";
        let back = remove(edited.as_bytes(), G, K, &a.receipt, false).unwrap();
        assert_eq!(
            String::from_utf8(back).unwrap(),
            "[Context]\nfilesystems=xdg-music;\nsockets=x11;\n"
        );
        // The list emptied, but the group now holds something else.
        let edited = String::from_utf8(a.content.clone()).unwrap() + "sockets=x11;\n";
        let back = remove(edited.as_bytes(), G, K, &a.receipt, false).unwrap();
        assert_eq!(
            String::from_utf8(back).unwrap(),
            "[Context]\nsockets=x11;\n"
        );
        // Already taken out by hand: nothing to do.
        let gone = b"[Context]\nsockets=x11;\n";
        assert_eq!(remove(gone, G, K, &a.receipt, false).unwrap(), gone);
        assert!(!holds(gone, G, K, &a.receipt));
    }

    #[test]
    fn a_member_that_stands_for_ours_is_respected() {
        let same = |have: &str, want: &str| {
            let path = |m: &str| {
                m.trim_start_matches('!')
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            };
            path(have) == path(want)
        };
        // Access the user denied, or gave in full, is the user's decision.
        let file = b"[Context]\nfilesystems=!xdg-config/gtk-3.0;xdg-config/gtk-4.0;\n";
        let a = add(file, G, K, &WANT, &same).unwrap();
        assert!(a.added.is_empty());
        assert_eq!(a.content, file);
    }

    #[test]
    fn a_file_that_is_not_text_is_refused() {
        assert!(add(b"\xff\xfe", G, K, &WANT, &exact).is_none());
        assert!(remove(b"\xff\xfe", G, K, "group|a", false).is_none());
    }
}
