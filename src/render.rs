// Composite character cells into a CPU framebuffer (0x00RRGGBB per pixel).
// Everything clips to a Rect so panes never bleed into their neighbours.

use crate::font::FontAtlas;
use crate::layout::Rect;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
}

fn buf_height(buf: &[u32], stride: usize) -> usize {
    if stride == 0 { 0 } else { buf.len() / stride }
}

/// Fill a rectangle (clamped to the buffer) with a solid color.
pub fn fill_rect(buf: &mut [u32], stride: usize, rect: Rect, color: u32) {
    let h = buf_height(buf, stride);
    let x1 = (rect.x + rect.w).min(stride);
    let y1 = (rect.y + rect.h).min(h);
    for y in rect.y..y1 {
        let base = y * stride;
        for x in rect.x..x1 {
            buf[base + x] = color;
        }
    }
}

/// Draw a 1px border just inside `rect`.
pub fn stroke_rect(buf: &mut [u32], stride: usize, rect: Rect, color: u32) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    fill_rect(buf, stride, Rect { x: rect.x, y: rect.y, w: rect.w, h: 1 }, color);
    fill_rect(buf, stride, Rect { x: rect.x, y: rect.y + rect.h - 1, w: rect.w, h: 1 }, color);
    fill_rect(buf, stride, Rect { x: rect.x, y: rect.y, w: 1, h: rect.h }, color);
    fill_rect(buf, stride, Rect { x: rect.x + rect.w - 1, y: rect.y, w: 1, h: rect.h }, color);
}

/// Draw one cell with its top-left at pixel (px, py), clipped to `clip`.
/// When `fill_bg` is false the background is left untouched (glow shows through).
#[allow(clippy::too_many_arguments)]
pub fn draw_cell(
    buf: &mut [u32],
    stride: usize,
    font: &mut FontAtlas,
    px: usize,
    py: usize,
    cell: Cell,
    fill_bg: bool,
    clip: Rect,
) {
    let h = buf_height(buf, stride);
    let cx0 = clip.x;
    let cy0 = clip.y;
    let cx1 = (clip.x + clip.w).min(stride);
    let cy1 = (clip.y + clip.h).min(h);

    let cw = font.cell_w;
    let ch_h = font.cell_h;
    let ascent = font.ascent;

    if fill_bg {
        for yy in 0..ch_h {
            let y = py + yy;
            if y < cy0 || y >= cy1 {
                continue;
            }
            let base = y * stride;
            for xx in 0..cw {
                let x = px + xx;
                if x < cx0 || x >= cx1 {
                    continue;
                }
                buf[base + x] = cell.bg;
            }
        }
    }

    if cell.ch == ' ' || cell.ch == '\0' {
        return;
    }

    let (m, bitmap) = {
        let g = font.glyph(cell.ch);
        (g.0, &g.1)
    };
    if m.width == 0 || m.height == 0 {
        return;
    }

    let gx = px as i32 + m.xmin;
    let baseline = py as i32 + ascent.round() as i32;
    let gy_top = baseline - (m.height as i32 + m.ymin);

    for ry in 0..m.height {
        let y = gy_top + ry as i32;
        if y < cy0 as i32 || y >= cy1 as i32 {
            continue;
        }
        let row_base = y as usize * stride;
        for rx in 0..m.width {
            let x = gx + rx as i32;
            if x < cx0 as i32 || x >= cx1 as i32 {
                continue;
            }
            let cov = bitmap[ry * m.width + rx];
            if cov == 0 {
                continue;
            }
            let idx = row_base + x as usize;
            buf[idx] = blend(cell.fg, buf[idx], cov);
        }
    }
}

/// Draw a string starting at pixel (px, py), one monospace cell per char.
pub fn draw_text(
    buf: &mut [u32],
    stride: usize,
    font: &mut FontAtlas,
    px: usize,
    py: usize,
    text: &str,
    fg: u32,
    bg: u32,
    fill_bg: bool,
    clip: Rect,
) {
    let cw = font.cell_w;
    for (i, ch) in text.chars().enumerate() {
        draw_cell(buf, stride, font, px + i * cw, py, Cell { ch, fg, bg }, fill_bg, clip);
    }
}

/// Alpha-blend `fg` over `bg` by coverage 0..=255.
fn blend(fg: u32, bg: u32, cov: u8) -> u32 {
    let a = cov as u32;
    let inv = 255 - a;
    let r = (((fg >> 16) & 0xff) * a + ((bg >> 16) & 0xff) * inv) / 255;
    let g = (((fg >> 8) & 0xff) * a + ((bg >> 8) & 0xff) * inv) / 255;
    let b = ((fg & 0xff) * a + (bg & 0xff) * inv) / 255;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontAtlas;

    fn full(stride: usize, height: usize) -> Rect {
        Rect { x: 0, y: 0, w: stride, h: height }
    }

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(0xffffff, 0x000000, 255), 0xffffff);
        assert_eq!(blend(0xffffff, 0x000000, 0), 0x000000);
    }

    #[test]
    fn glyph_marks_pixels() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w * 2;
        let height = font.cell_h * 2;
        let mut buf = vec![0x0011_1111u32; stride * height];
        draw_cell(&mut buf, stride, &mut font, 0, 0,
            Cell { ch: 'M', fg: 0x00ff_ffff, bg: 0x0011_1111 }, true, full(stride, height));
        let marked = buf.iter().filter(|&&p| p != 0x0011_1111).count();
        assert!(marked > 0, "glyph 'M' drew no foreground pixels");
    }

    #[test]
    fn space_is_pure_background() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x0011_1111u32; stride * height];
        draw_cell(&mut buf, stride, &mut font, 0, 0,
            Cell { ch: ' ', fg: 0x00ff_ffff, bg: 0x0022_2222 }, true, full(stride, height));
        assert!(buf.iter().all(|&p| p == 0x0022_2222), "space must be pure bg");
    }

    #[test]
    fn space_no_fill_leaves_buffer_untouched() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x00ab_cdefu32; stride * height];
        draw_cell(&mut buf, stride, &mut font, 0, 0,
            Cell { ch: ' ', fg: 0x00ff_ffff, bg: 0x0022_2222 }, false, full(stride, height));
        assert!(buf.iter().all(|&p| p == 0x00ab_cdef), "no-fill space must not touch buffer");
    }

    #[test]
    fn clip_blocks_out_of_bounds_fill() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w * 2;
        let height = font.cell_h;
        let cwid = font.cell_w;
        let mut buf = vec![0x0000_0000u32; stride * height];
        // Clip to the left half only; draw a filled cell in the right half.
        let clip = Rect { x: 0, y: 0, w: cwid, h: height };
        draw_cell(&mut buf, stride, &mut font, cwid, 0,
            Cell { ch: ' ', fg: 0xfff, bg: 0x00ff_ffff }, true, clip);
        assert!(buf.iter().all(|&p| p == 0), "clip must prevent drawing outside it");
    }
}
