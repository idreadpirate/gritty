// Composite character cells into a CPU framebuffer (0x00RRGGBB per pixel).

use crate::font::FontAtlas;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
}

/// Draw one cell at grid position (col, row) into `buf` (width = `stride`).
/// When `fill_bg` is false the cell background is left untouched, letting the
/// decorative base layer show through (used for default-background cells).
pub fn draw_cell(
    buf: &mut [u32],
    stride: usize,
    height: usize,
    font: &mut FontAtlas,
    col: usize,
    row: usize,
    cell: Cell,
    fill_bg: bool,
) {
    let cw = font.cell_w;
    let ch_h = font.cell_h;
    let ascent = font.ascent;
    let x0 = col * cw;
    let y0 = row * ch_h;

    // Background fill for the whole cell.
    if fill_bg {
        for yy in 0..ch_h {
            let y = y0 + yy;
            if y >= height {
                break;
            }
            let base = y * stride;
            for xx in 0..cw {
                let x = x0 + xx;
                if x >= stride {
                    break;
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

    let gx = x0 as i32 + m.xmin;
    let baseline = y0 as i32 + ascent.round() as i32;
    let gy_top = baseline - (m.height as i32 + m.ymin);

    for ry in 0..m.height {
        let y = gy_top + ry as i32;
        if y < 0 || y as usize >= height {
            continue;
        }
        let row_base = y as usize * stride;
        for rx in 0..m.width {
            let x = gx + rx as i32;
            if x < 0 || x as usize >= stride {
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
        draw_cell(&mut buf, stride, height, &mut font, 0, 0,
            Cell { ch: 'M', fg: 0x00ff_ffff, bg: 0x0011_1111 }, true);
        let marked = buf.iter().filter(|&&p| p != 0x0011_1111).count();
        assert!(marked > 0, "glyph 'M' drew no foreground pixels");
    }

    #[test]
    fn space_is_pure_background() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x0011_1111u32; stride * height];
        draw_cell(&mut buf, stride, height, &mut font, 0, 0,
            Cell { ch: ' ', fg: 0x00ff_ffff, bg: 0x0022_2222 }, true);
        assert!(buf.iter().all(|&p| p == 0x0022_2222), "space must be pure bg");
    }

    #[test]
    fn space_no_fill_leaves_buffer_untouched() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x00ab_cdefu32; stride * height];
        draw_cell(&mut buf, stride, height, &mut font, 0, 0,
            Cell { ch: ' ', fg: 0x00ff_ffff, bg: 0x0022_2222 }, false);
        assert!(buf.iter().all(|&p| p == 0x00ab_cdef), "no-fill space must not touch buffer");
    }
}
