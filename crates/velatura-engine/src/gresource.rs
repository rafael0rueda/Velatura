//! Reading one file out of a GLib resource bundle (`.gresource`), which is
//! how GNOME Shell ships its own stylesheets.
//!
//! A bundle is a GVDB table: a header, then a hash table whose items name
//! each path piece by piece, each pointing at a value that holds the
//! file's size, its flags and its bytes. Only what finding one file needs
//! is read here, and every offset is checked against the bundle's length.
//! Files stored compressed are not read: the Shell's stylesheets are not.

/// The bytes of the file at `path` (for example
/// `/org/gnome/shell/theme/gnome-shell-dark.css`) in `bundle`, or `None`
/// if it is not there, is compressed, or the bundle is not well formed.
pub fn file<'a>(bundle: &'a [u8], path: &str) -> Option<&'a [u8]> {
    if bundle.get(..8)? != b"GVariant" || u32_at(bundle, 8)? != 0 {
        return None;
    }
    let (start, end) = (u32_at(bundle, 16)?, u32_at(bundle, 20)?);
    let table = bundle.get(start as usize..end as usize)?;
    let bloom = u32_at(table, 0)? & ((1 << 27) - 1);
    let buckets = u32_at(table, 4)?;
    let items_at = 8usize
        .checked_add(4 * bloom as usize)?
        .checked_add(4 * buckets as usize)?;
    let items = table.get(items_at..)?;
    let count = items.len() / ITEM;
    let item = |i: usize| items.get(i * ITEM..(i + 1) * ITEM);
    // An item's name is its own piece after its parent's whole name.
    let name = |mut i: usize| -> Option<Vec<u8>> {
        let mut pieces = Vec::new();
        for _ in 0..=count {
            let it = item(i)?;
            let key_start = u32_at(it, 8)? as usize;
            let key_size = u16::from_le_bytes([it[12], it[13]]) as usize;
            pieces.push(bundle.get(key_start..key_start.checked_add(key_size)?)?);
            match u32_at(it, 4)? {
                u32::MAX => {
                    return Some(pieces.into_iter().rev().flatten().copied().collect());
                }
                parent => i = parent as usize,
            }
            if i >= count {
                return None;
            }
        }
        None
    };
    let found = (0..count).find(|&i| {
        item(i).is_some_and(|it| it[14] == b'v') && name(i).as_deref() == Some(path.as_bytes())
    })?;
    let it = item(found)?;
    let value = bundle.get(u32_at(it, 16)? as usize..u32_at(it, 20)? as usize)?;
    // A variant: the value, a zero byte, then its type, which here is a
    // size, flags, and the bytes.
    let split = value.iter().rposition(|b| *b == 0)?;
    if &value[split + 1..] != b"(uuay)" {
        return None;
    }
    let inner = &value[..split];
    let size = u32_at(inner, 0)? as usize;
    if u32_at(inner, 4)? != 0 {
        return None;
    }
    inner.get(8..8usize.checked_add(size)?)
}

/// The size of one hash item.
const ITEM: usize = 24;

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle with `/a/` holding `b.css`, built by hand the way GVDB
    /// lays one out, without a bloom filter.
    fn bundle(content: &[u8], flags: u32) -> Vec<u8> {
        let mut out = vec![0u8; 24];
        out[..8].copy_from_slice(b"GVariant");
        let keys_at = out.len();
        out.extend_from_slice(b"/a/b.css");
        let mut value = Vec::new();
        value.extend_from_slice(&(content.len() as u32).to_le_bytes());
        value.extend_from_slice(&flags.to_le_bytes());
        value.extend_from_slice(content);
        value.push(0);
        value.push(0);
        value.extend_from_slice(b"(uuay)");
        let value_at = out.len();
        out.extend_from_slice(&value);
        let table_at = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        let item = |parent: u32, key: (usize, usize), kind: u8, ptr: (usize, usize)| {
            let mut it = Vec::new();
            it.extend_from_slice(&0u32.to_le_bytes());
            it.extend_from_slice(&parent.to_le_bytes());
            it.extend_from_slice(&(key.0 as u32).to_le_bytes());
            it.extend_from_slice(&(key.1 as u16).to_le_bytes());
            it.push(kind);
            it.push(0);
            it.extend_from_slice(&(ptr.0 as u32).to_le_bytes());
            it.extend_from_slice(&(ptr.1 as u32).to_le_bytes());
            it
        };
        out.extend(item(u32::MAX, (keys_at, 3), b'L', (0, 0)));
        out.extend(item(
            0,
            (keys_at + 3, 5),
            b'v',
            (value_at, value_at + value.len()),
        ));
        let end = out.len();
        out[16..20].copy_from_slice(&(table_at as u32).to_le_bytes());
        out[20..24].copy_from_slice(&(end as u32).to_le_bytes());
        out
    }

    #[test]
    fn a_file_is_found_by_its_whole_path() {
        let b = bundle(b"stage { color: #fff; }", 0);
        assert_eq!(file(&b, "/a/b.css"), Some(&b"stage { color: #fff; }"[..]));
        assert_eq!(file(&b, "/a/"), None);
        assert_eq!(file(&b, "b.css"), None);
        assert_eq!(file(&b, "/a/c.css"), None);
    }

    #[test]
    fn compressed_damaged_and_foreign_bundles_give_nothing() {
        assert_eq!(file(&bundle(b"x", 1), "/a/b.css"), None);
        let good = bundle(b"x", 0);
        for cut in [0, 7, 23, good.len() / 2, good.len() - 1] {
            assert_eq!(file(&good[..cut], "/a/b.css"), None, "cut at {cut}");
        }
        let mut wrong = good.clone();
        wrong[0] = b'X';
        assert_eq!(file(&wrong, "/a/b.css"), None);
        // An item that names itself as its own parent ends, not loops.
        let mut looped = good;
        let items = looped.len() - 2 * ITEM;
        looped[items + 4..items + 8].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(file(&looped, "/a/b.css"), None);
    }

    /// The real bundle of the Shell installed here, when there is one.
    #[test]
    fn reads_the_shells_own_stylesheet() {
        let Ok(bytes) = std::fs::read("/usr/share/gnome-shell/gnome-shell-theme.gresource") else {
            return;
        };
        let css = file(&bytes, "/org/gnome/shell/theme/gnome-shell-dark.css").unwrap();
        let css = std::str::from_utf8(css).unwrap();
        assert!(css.contains("stage {") && css.contains("#panel {"));
    }
}
