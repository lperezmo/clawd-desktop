//! Which screen pixels are Clawd: the overlay's input region is built from
//! the opaque pixels of the frame it shows, so only his pixels take the mouse.

use crate::surfaces::Rect;

/// Opaque pixels of a premultiplied BGRA image (`w` by `h`, rows `stride`
/// apart) as rectangles: one per run of opaque pixels in a row, with rows
/// whose runs are identical merged into one taller rectangle. At Clawd's
/// scale that is a few dozen rectangles a frame.
pub fn opaque_rects(px: &[u32], stride: usize, w: usize, h: usize) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    // Rectangles that the previous row ended, open for merging with this one.
    let mut open: Vec<usize> = Vec::new();
    let mut runs: Vec<(i32, i32)> = Vec::new();
    let mut prev_runs: Vec<(i32, i32)> = Vec::new();
    for y in 0..h {
        runs.clear();
        let row = &px[y * stride..y * stride + w];
        let mut x = 0;
        while x < w {
            if row[x] >> 24 == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && row[x] >> 24 != 0 {
                x += 1;
            }
            runs.push((start as i32, x as i32));
        }
        if !runs.is_empty() && runs == prev_runs {
            for &i in &open {
                out[i].bottom = y as i32 + 1;
            }
        } else {
            open.clear();
            for &(a, b) in &runs {
                open.push(out.len());
                out.push(Rect::new(a, y as i32, b, y as i32 + 1));
            }
        }
        std::mem::swap(&mut runs, &mut prev_runs);
    }
    out
}

/// The pixel of an image whose top-left is at `origin` on screen that the
/// screen point `p` falls on, or None when it is outside the `w` by `h` image.
pub fn pixel_at(origin: (i32, i32), w: i32, h: i32, p: (i32, i32)) -> Option<(usize, usize)> {
    let (x, y) = (p.0 - origin.0, p.1 - origin.1);
    (x >= 0 && y >= 0 && x < w && y < h).then_some((x as usize, y as usize))
}

/// Whether the screen point `p` is on an opaque pixel of the image.
pub fn opaque_at(px: &[u32], origin: (i32, i32), w: i32, h: i32, p: (i32, i32)) -> bool {
    pixel_at(origin, w, h, p).is_some_and(|(x, y)| px.get(y * w as usize + x).is_some_and(|&c| c >> 24 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sprites::{SpriteRef, frame, sheet_index};

    fn inside(rects: &[Rect], x: i32, y: i32) -> bool {
        rects
            .iter()
            .any(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    }

    #[test]
    fn rects_cover_exactly_the_opaque_pixels() {
        let walk = SpriteRef::Sheet(sheet_index("walk"));
        for (n, scale, mirrored) in [(0, 4, false), (7, 3, true), (12, 2, false)] {
            let f = frame(walk, n);
            let mut buf = Vec::new();
            f.render(scale, mirrored, &mut buf);
            let (w, h) = (f.w * scale, f.h * scale);
            let rects = opaque_rects(&buf, w, w, h);
            for y in 0..h {
                for x in 0..w {
                    let opaque = buf[y * w + x] >> 24 != 0;
                    assert_eq!(inside(&rects, x as i32, y as i32), opaque, "({x}, {y})");
                }
            }
            // Scaled rows merge: far fewer rectangles than rows times runs.
            assert!(rects.len() < h * 2, "{} rects for {h} rows", rects.len());
        }
    }

    #[test]
    fn rects_respect_stride() {
        // A 3 by 2 image inside rows 5 wide; the padding is opaque and must be ignored.
        let o = 0xff00_0000u32;
        let px = [o, 0, o, o, o, 0, 0, o, o, o];
        let r = opaque_rects(&px, 5, 3, 2);
        assert_eq!(
            r,
            vec![Rect::new(0, 0, 1, 1), Rect::new(2, 0, 3, 1), Rect::new(2, 1, 3, 2)]
        );
        assert!(opaque_rects(&[0; 6], 3, 3, 2).is_empty());
    }

    #[test]
    fn cursor_maps_to_the_pixel_under_it() {
        let f = frame(SpriteRef::Sheet(sheet_index("walk")), 0);
        let scale = 4;
        let mut buf = Vec::new();
        f.render(scale, false, &mut buf);
        let (w, h) = ((f.w * scale) as i32, (f.h * scale) as i32);
        let origin = (1000, 500);
        assert_eq!(pixel_at(origin, w, h, (1000, 500)), Some((0, 0)));
        assert_eq!(
            pixel_at(origin, w, h, (1000 + w - 1, 500 + h - 1)),
            Some((w as usize - 1, h as usize - 1))
        );
        assert_eq!(pixel_at(origin, w, h, (999, 520)), None);
        assert_eq!(pixel_at(origin, w, h, (1000 + w, 520)), None);
        // Row 6 (the arms) is opaque across the whole width; rows 0 and 1 are clear.
        assert!(opaque_at(&buf, origin, w, h, (1000 + 2, 500 + 6 * scale as i32 + 1)));
        assert!(opaque_at(&buf, origin, w, h, (1000 + w - 1, 500 + 6 * scale as i32)));
        assert!(!opaque_at(&buf, origin, w, h, (1000 + w / 2, 500 + 3)));
        // Agrees with the region built from the same frame.
        let rects = opaque_rects(&buf, w as usize, w as usize, h as usize);
        for y in 0..h {
            for x in 0..w {
                assert_eq!(opaque_at(&buf, origin, w, h, (1000 + x, 500 + y)), inside(&rects, x, y));
            }
        }
    }
}
