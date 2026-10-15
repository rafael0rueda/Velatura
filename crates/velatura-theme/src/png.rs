//! Reading and writing PNG pictures, for recolouring icons that are
//! pictures, not drawings.
//!
//! Only what icon themes use is read: 8 bits per channel, grey, grey with
//! alpha, RGB, RGBA or a palette, not interlaced. Everything comes out as
//! RGBA. Pictures are written as RGBA with each row's filter chosen for
//! size, compressed by `miniz_oxide`; chunks other than the picture itself
//! (text, colour profiles) are not kept. Every length and checksum is
//! checked, and a picture larger than [`MAX_SIDE`] on a side is refused.

use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;

/// The largest side a picture may have: icons are at most 512 pixels.
pub const MAX_SIDE: u32 = 4096;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// A picture, four bytes a pixel: red, green, blue, alpha, not
/// premultiplied, row after row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The CRC-32 of PNG chunks (ISO 3309), one byte at a time.
fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, slot) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut crc = 0xffff_ffffu32;
    for part in parts {
        for b in *part {
            crc = table[((crc ^ u32::from(*b)) & 0xff) as usize] ^ (crc >> 8);
        }
    }
    crc ^ 0xffff_ffff
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i16::from(a), i16::from(b), i16::from(c));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Read a PNG. `None` for anything this does not read, or that is
/// damaged.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    if bytes.get(..8)? != SIGNATURE {
        return None;
    }
    let mut at = 8;
    let mut header = None;
    let mut palette: &[u8] = &[];
    let mut transparency: &[u8] = &[];
    let mut data = Vec::new();
    loop {
        let len = be32(bytes, at)? as usize;
        let kind = bytes.get(at + 4..at + 8)?;
        let body = bytes.get(at + 8..at.checked_add(8)?.checked_add(len)?)?;
        if be32(bytes, at + 8 + len)? != crc32(&[kind, body]) {
            return None;
        }
        match kind {
            b"IHDR" if body.len() == 13 => header = Some(body),
            b"PLTE" => palette = body,
            b"tRNS" => transparency = body,
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            // Another chunk the picture cannot be drawn without.
            k if k[0].is_ascii_uppercase() && !matches!(k, b"IHDR" | b"PLTE" | b"IDAT") => {
                return None;
            }
            _ => {}
        }
        at += 12 + len;
    }
    let h = header?;
    let (width, height) = (be32(h, 0)?, be32(h, 4)?);
    let (depth, color, interlace) = (h[8], h[9], h[12]);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    if depth != 8 || h[10] != 0 || h[11] != 0 || interlace != 0 {
        return None;
    }
    let channels = match color {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        6 => 4,
        _ => return None,
    };
    let (w, rows) = (width as usize, height as usize);
    let stride = w * channels;
    let raw = decompress_to_vec_zlib_with_limit(&data, rows * (stride + 1)).ok()?;
    if raw.len() != rows * (stride + 1) {
        return None;
    }
    // Undo each row's filter.
    let mut pixels = vec![0u8; rows * stride];
    for y in 0..rows {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels {
                pixels[y * stride + x - channels]
            } else {
                0
            };
            let b = if y > 0 {
                pixels[(y - 1) * stride + x]
            } else {
                0
            };
            let c = if x >= channels && y > 0 {
                pixels[(y - 1) * stride + x - channels]
            } else {
                0
            };
            let add = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return None,
            };
            pixels[y * stride + x] = line[x].wrapping_add(add);
        }
    }
    let mut rgba = Vec::with_capacity(w * rows * 4);
    for px in pixels.chunks_exact(channels) {
        match color {
            0 => {
                let alpha = match transparency {
                    [_, v] if *v == px[0] => 0,
                    _ => 255,
                };
                rgba.extend_from_slice(&[px[0], px[0], px[0], alpha]);
            }
            4 => rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]),
            2 => {
                let alpha = match transparency {
                    [_, r, _, g, _, b] if [*r, *g, *b] == px[..3] => 0,
                    _ => 255,
                };
                rgba.extend_from_slice(&[px[0], px[1], px[2], alpha]);
            }
            3 => {
                let i = px[0] as usize;
                let rgb = palette.get(i * 3..i * 3 + 3)?;
                let alpha = transparency.get(i).copied().unwrap_or(255);
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
            }
            _ => rgba.extend_from_slice(px),
        }
    }
    Some(Image {
        width,
        height,
        rgba,
    })
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc32(&[kind, body]).to_be_bytes());
}

/// Write `image` as an RGBA PNG.
pub fn encode(image: &Image) -> Vec<u8> {
    let (w, rows) = (image.width as usize, image.height as usize);
    let stride = w * 4;
    let px = &image.rgba;
    let mut raw = Vec::with_capacity(rows * (stride + 1));
    let mut line = vec![0u8; stride];
    for y in 0..rows {
        let row = &px[y * stride..(y + 1) * stride];
        let up = |x: usize| if y > 0 { px[(y - 1) * stride + x] } else { 0 };
        let left = |x: usize| if x >= 4 { row[x - 4] } else { 0 };
        let corner = |x: usize| {
            if x >= 4 && y > 0 {
                px[(y - 1) * stride + x - 4]
            } else {
                0
            }
        };
        // The filter whose bytes add up smallest, counted as signed: the
        // usual way to choose, and what compresses well.
        let mut best = (u64::MAX, 0u8);
        for filter in 0..5u8 {
            let cost: u64 = (0..stride)
                .map(|x| {
                    let v = row[x].wrapping_sub(match filter {
                        0 => 0,
                        1 => left(x),
                        2 => up(x),
                        3 => ((u16::from(left(x)) + u16::from(up(x))) / 2) as u8,
                        _ => paeth(left(x), up(x), corner(x)),
                    });
                    u64::from((v as i8).unsigned_abs())
                })
                .sum();
            if cost < best.0 {
                best = (cost, filter);
            }
        }
        for x in 0..stride {
            line[x] = row[x].wrapping_sub(match best.1 {
                0 => 0,
                1 => left(x),
                2 => up(x),
                3 => ((u16::from(left(x)) + u16::from(up(x))) / 2) as u8,
                _ => paeth(left(x), up(x), corner(x)),
            });
        }
        raw.push(best.1);
        raw.extend_from_slice(&line);
    }
    let mut out = SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&image.width.to_be_bytes());
    header.extend_from_slice(&image.height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compress_to_vec_zlib(&raw, 9));
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Image {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[
                    (x * 7) as u8,
                    (y * 5) as u8,
                    ((x + y) * 3) as u8,
                    (x * y) as u8,
                ]);
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        for (w, h) in [(1, 1), (16, 16), (37, 5), (128, 128)] {
            let image = gradient(w, h);
            let bytes = encode(&image);
            assert_eq!(decode(&bytes), Some(image), "{w}x{h}");
        }
    }

    #[test]
    fn the_checksum_is_pngs_own() {
        // The CRC of an empty IEND chunk, as every PNG ends with it.
        assert_eq!(crc32(&[b"IEND", b""]), 0xae42_6082);
    }

    #[test]
    fn damaged_and_unread_pictures_give_nothing() {
        let good = encode(&gradient(8, 8));
        assert!(decode(&good).is_some());
        for cut in [0, 8, 20, good.len() - 13, good.len() - 1] {
            assert_eq!(decode(&good[..cut]), None, "cut at {cut}");
        }
        let mut flipped = good.clone();
        let mid = flipped.len() / 2;
        flipped[mid] ^= 0x40;
        assert_eq!(decode(&flipped), None, "a checksum catches it");
        // Sixteen bits a channel, and interlaced, are not read.
        for (at, value) in [(24, 16u8), (28, 1u8)] {
            let mut other = good.clone();
            other[at] = value;
            let crc = crc32(&[&other[12..16], &other[16..29]]);
            other[29..33].copy_from_slice(&crc.to_be_bytes());
            assert_eq!(decode(&other), None);
        }
    }
}
