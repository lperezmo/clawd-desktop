//! Writes assets/clawd.ico and assets/clawd-256.png from the walk sheet's
//! frame 0.
//!
//!     cargo run --example make-icon
//!
//! The walk art is drawn at 2x (every feature is a 2 by 2 block) with two
//! blank rows on top, so the icon works from the 12 by 8 grid underneath and
//! scales that by whole numbers. Sizes up to 48 get a one-pixel #141413
//! outline so Clawd reads on light and dark backgrounds. The 256 entry is
//! PNG, the rest are 32-bit BMP with an AND mask.

#[path = "../src/sprites.rs"]
#[allow(dead_code)]
mod sprites;
#[path = "../src/sprites_data.rs"]
#[rustfmt::skip]
#[allow(dead_code)]
mod sprites_data;

use sprites::{CLEAR, SHEETS, sheet_index};
use std::path::PathBuf;

const SIZES: [usize; 7] = [16, 24, 32, 48, 64, 128, 256];
const OUTLINE_UP_TO: usize = 48;
const OUTLINE: [u8; 4] = [0x14, 0x14, 0x13, 0xff];

/// The 12 by 8 grid under the walk sheet's frame 0, as RGBA (None = clear).
fn base() -> (usize, usize, Vec<Option<[u8; 4]>>) {
    let s = &SHEETS[sheet_index("walk")];
    let f = s.frames[0];
    let top = (0..s.h)
        .find(|&y| f[y * s.w..(y + 1) * s.w].iter().any(|&p| p != CLEAR))
        .unwrap_or(0);
    let (w, h) = (s.w / 2, (s.h - top) / 2);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = f[(top + y * 2) * s.w + x * 2];
            out.push((p != CLEAR).then(|| {
                let c = s.palette[p as usize];
                [(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xff]
            }));
        }
    }
    (w, h, out)
}

/// RGBA, `n` by `n`, top row first.
fn render(n: usize) -> Vec<[u8; 4]> {
    let (bw, bh, px) = base();
    let k = (n / bw).max(1);
    let (w, h) = (bw * k, bh * k);
    let (ox, oy) = ((n as isize - w as isize) / 2, (n as isize - h as isize) / 2);
    let mut img = vec![[0u8; 4]; n * n];
    let at = |x: isize, y: isize| -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            return None;
        }
        px[(y as usize / k) * bw + x as usize / k]
    };
    for y in 0..n as isize {
        for x in 0..n as isize {
            let (sx, sy) = (x - ox, y - oy);
            let c = match at(sx, sy) {
                Some(c) => c,
                None if n <= OUTLINE_UP_TO && (-1..=1).any(|dy| (-1..=1).any(|dx| at(sx + dx, sy + dy).is_some())) => {
                    OUTLINE
                }
                None => continue,
            };
            img[y as usize * n + x as usize] = c;
        }
    }
    img
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// LSB-first bit writer for deflate.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, len: u32) {
        self.acc |= v << self.n;
        self.n += len;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman codes go most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let mut r = 0;
        for i in 0..len {
            r |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(r, len);
    }
    fn sym(&mut self, s: u32) {
        match s {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + s - 144, 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xc0 + s - 280, 8),
        }
    }
}

const LEN_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

/// zlib stream, one fixed-Huffman block, matches only at a few fixed
/// distances (previous byte, previous pixel, the pixel above). Plenty for
/// flat pixel art.
fn zlib(data: &[u8], dists: &[usize]) -> Vec<u8> {
    let mut b = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        n: 0,
    };
    b.put(1, 1); // final block
    b.put(1, 2); // fixed Huffman
    let mut i = 0;
    while i < data.len() {
        let mut best = (0usize, 0usize);
        for &d in dists {
            if d > i {
                continue;
            }
            let mut l = 0;
            while l < 258 && i + l < data.len() && data[i + l] == data[i + l - d] {
                l += 1;
            }
            if l > best.0 {
                best = (l, d);
            }
        }
        let (len, dist) = best;
        if len >= 3 {
            let c = LEN_BASE.iter().rposition(|&x| x <= len as u32).unwrap();
            b.sym(257 + c as u32);
            b.put(len as u32 - LEN_BASE[c], LEN_EXTRA[c]);
            let d = DIST_BASE.iter().rposition(|&x| x <= dist as u32).unwrap();
            b.code(d as u32, 5);
            b.put(dist as u32 - DIST_BASE[d], DIST_EXTRA[d]);
            i += len;
        } else {
            b.sym(data[i] as u32);
            i += 1;
        }
    }
    b.sym(256);
    if b.n > 0 {
        b.out.push(b.acc as u8);
    }
    let (mut s1, mut s2) = (1u32, 0u32);
    for &x in data {
        s1 = (s1 + x as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    b.out.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
    b.out
}

fn png(n: usize, img: &[[u8; 4]]) -> Vec<u8> {
    let stride = 1 + 4 * n;
    let mut raw = Vec::with_capacity(stride * n);
    for row in img.chunks(n) {
        raw.push(0);
        for p in row {
            raw.extend_from_slice(p);
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&(n as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &zlib(&raw, &[1, 4, stride]));
    chunk(b"IEND", &[]);
    out
}

/// A 32-bit BMP icon image: header, bottom-up BGRA, then the AND mask.
fn bmp(n: usize, img: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::new();
    let u32le = |v: u32, o: &mut Vec<u8>| o.extend_from_slice(&v.to_le_bytes());
    u32le(40, &mut out);
    u32le(n as u32, &mut out);
    u32le(2 * n as u32, &mut out);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    for _ in 0..6 {
        u32le(0, &mut out);
    }
    for y in (0..n).rev() {
        for x in 0..n {
            let [r, g, b, a] = img[y * n + x];
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_stride = n.div_ceil(32) * 4;
    for y in (0..n).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in 0..n {
            if img[y * n + x][3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    std::fs::create_dir_all(&dir).expect("assets dir");
    let images: Vec<(usize, Vec<u8>)> = SIZES
        .iter()
        .map(|&n| {
            let img = render(n);
            (n, if n >= 256 { png(n, &img) } else { bmp(n, &img) })
        })
        .collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (n, data) in &images {
        let dim = if *n >= 256 { 0 } else { *n as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &images {
        ico.extend_from_slice(data);
    }
    std::fs::write(dir.join("clawd.ico"), &ico).expect("write ico");
    std::fs::write(dir.join("clawd-256.png"), png(256, &render(256))).expect("write png");
    for (n, data) in &images {
        println!("{n:>3} px: {} bytes", data.len());
    }
    println!("assets/clawd.ico: {} bytes", ico.len());
}
