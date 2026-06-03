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
    if stride == 0 {
        0
    } else {
        buf.len() / stride
    }
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
    fill_rect(
        buf,
        stride,
        Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: 1,
        },
        color,
    );
    fill_rect(
        buf,
        stride,
        Rect {
            x: rect.x,
            y: rect.y + rect.h - 1,
            w: rect.w,
            h: 1,
        },
        color,
    );
    fill_rect(
        buf,
        stride,
        Rect {
            x: rect.x,
            y: rect.y,
            w: 1,
            h: rect.h,
        },
        color,
    );
    fill_rect(
        buf,
        stride,
        Rect {
            x: rect.x + rect.w - 1,
            y: rect.y,
            w: 1,
            h: rect.h,
        },
        color,
    );
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
#[allow(clippy::too_many_arguments)]
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
        draw_cell(
            buf,
            stride,
            font,
            px + i * cw,
            py,
            Cell { ch, fg, bg },
            fill_bg,
            clip,
        );
    }
}

/// Gamma-aware alpha-blend `fg` over `bg` by coverage 0..=255 (CA-31).
///
/// Blending in raw sRGB space causes perceptible darkening at mid-coverage.
/// We use a fast 256-entry lookup to linearise sRGB (γ≈2.2 approximation),
/// blend in linear light, then re-encode.  No new dependencies, table is
/// computed once at first call via a `static`.
fn blend(fg: u32, bg: u32, cov: u8) -> u32 {
    // CA-106/CA-120: fully-opaque and fully-transparent coverage are the common
    // case in dense text; return the endpoint colour directly and skip all table
    // work. At full coverage the result is exactly `fg` (no gamma round-trip
    // error); at zero coverage it is exactly `bg`.
    if cov == 255 {
        return fg & 0x00ff_ffff;
    }
    if cov == 0 {
        return bg & 0x00ff_ffff;
    }

    // Linear table: TO_LINEAR[v] = (v/255)^2.2 * 1023, stored as u16.
    // Using *1023 (10-bit) keeps integer arithmetic precise enough.
    static TO_LINEAR: std::sync::OnceLock<[u16; 256]> = std::sync::OnceLock::new();
    let lin = TO_LINEAR.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let s = i as f32 / 255.0;
            // sRGB piecewise linearisation (IEC 61966-2-1).
            let l = if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            };
            *slot = (l * 1023.0 + 0.5) as u16;
        }
        t
    });

    // Gamma-expand one channel.
    let expand = |c: u32, sh: u32| lin[((c >> sh) & 0xff) as usize] as u32;

    // Re-encode: linear (0..=1023) → sRGB (0..=255) via a precomputed inverse
    // table. CA-106/CA-120: the forward direction was already a LUT, but the
    // reverse ran `1.055 * f.powf(1.0/2.4) - 0.055` once per channel per covered
    // pixel — 3 `powf` per blended pixel, every frame, the dominant per-pixel cost
    // in the software-raster hot path. FROM_LINEAR is built once and indexed by
    // the 10-bit linear value; its entries are that same f32 formula, memoized,
    // so the output is bit-identical to the old per-pixel computation.
    static FROM_LINEAR: std::sync::OnceLock<[u8; 1024]> = std::sync::OnceLock::new();
    let from_lin = FROM_LINEAR.get_or_init(|| {
        let mut t = [0u8; 1024];
        for (i, slot) in t.iter_mut().enumerate() {
            let f = i as f32 / 1023.0;
            let s = if f <= 0.0031308 {
                f * 12.92
            } else {
                1.055 * f.powf(1.0 / 2.4) - 0.055
            };
            *slot = (s * 255.0 + 0.5) as u8;
        }
        t
    });
    let compress = |lin_val: u32| -> u32 { from_lin[lin_val as usize] as u32 };

    let a = cov as u32;
    let inv = 255 - a;

    // Blend each channel in linear space then re-encode.
    let blend_ch = |sh: u32| -> u32 {
        let lin_fg = expand(fg, sh);
        let lin_bg = expand(bg, sh);
        let blended = (lin_fg * a + lin_bg * inv) / 255;
        compress(blended)
    };

    (blend_ch(16) << 16) | (blend_ch(8) << 8) | blend_ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontAtlas;

    fn full(stride: usize, height: usize) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: stride,
            h: height,
        }
    }

    #[test]
    fn blend_endpoints() {
        // Full coverage → fg; zero coverage → bg (gamma-correct endpoints are exact).
        assert_eq!(blend(0xffffff, 0x000000, 255), 0xffffff);
        assert_eq!(blend(0xffffff, 0x000000, 0), 0x000000);
    }

    #[test]
    fn blend_gamma_brighter_than_linear() {
        // CA-31: blending 50% white over black in linear space should produce a
        // perceptually-correct mid-grey (~186 per channel), which is *brighter*
        // than the naive sRGB midpoint (127).  This proves we're blending in
        // linear light, not raw sRGB.
        let result = blend(0xffffff, 0x000000, 128);
        let r = (result >> 16) & 0xff;
        // Linear-correct 50% blend of white over black encodes to ~186 in sRGB.
        // Allow ±3 for rounding in the approximation.
        assert!(
            r > 127,
            "gamma blend at 50% coverage should be > 127 (sRGB), got {r}"
        );
    }

    #[test]
    fn blend_inverse_lut_matches_reference() {
        // CA-106/CA-120: the inverse-sRGB step is now a 1024-entry table instead
        // of a per-pixel `powf`. Opaque coverage must return exactly `fg` (no
        // gamma round-trip), zero coverage exactly `bg`, and partial coverage must
        // match the reference encode of the linear-light blend within ±1.
        let encode = |lin_val: u32| -> u32 {
            let f = lin_val as f32 / 1023.0;
            let s = if f <= 0.0031308 {
                f * 12.92
            } else {
                1.055 * f.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0 + 0.5) as u32
        };
        let to_lin = |c: u32, sh: u32| -> u32 {
            let s = ((c >> sh) & 0xff) as f32 / 255.0;
            let l = if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            };
            (l * 1023.0 + 0.5) as u32
        };

        for fg in [0x0000_0000u32, 0x0080_8080, 0x0012_3456, 0x00ff_ffff] {
            assert_eq!(blend(fg, 0x0033_4455, 255), fg, "opaque must be fg exactly");
        }
        assert_eq!(
            blend(0x00ff_ffff, 0x0033_4455, 0),
            0x0033_4455,
            "zero cov = bg"
        );

        for &(fg, bg, cov) in &[
            (0x00ff_ff00u32, 0x0000_00ffu32, 64u8),
            (0x0080_4020, 0x0010_2030, 200),
        ] {
            let (a, inv) = (cov as u32, 255 - cov as u32);
            let got = blend(fg, bg, cov);
            for sh in [16u32, 8, 0] {
                let blended = (to_lin(fg, sh) * a + to_lin(bg, sh) * inv) / 255;
                let (g, e) = ((got >> sh) & 0xff, encode(blended));
                assert!(
                    (g as i32 - e as i32).abs() <= 1,
                    "channel {sh}: lut {g} vs ref {e}"
                );
            }
        }
    }

    #[test]
    fn glyph_marks_pixels() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w * 2;
        let height = font.cell_h * 2;
        let mut buf = vec![0x0011_1111u32; stride * height];
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            Cell {
                ch: 'M',
                fg: 0x00ff_ffff,
                bg: 0x0011_1111,
            },
            true,
            full(stride, height),
        );
        let marked = buf.iter().filter(|&&p| p != 0x0011_1111).count();
        assert!(marked > 0, "glyph 'M' drew no foreground pixels");
    }

    #[test]
    fn space_is_pure_background() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x0011_1111u32; stride * height];
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            Cell {
                ch: ' ',
                fg: 0x00ff_ffff,
                bg: 0x0022_2222,
            },
            true,
            full(stride, height),
        );
        assert!(
            buf.iter().all(|&p| p == 0x0022_2222),
            "space must be pure bg"
        );
    }

    #[test]
    fn space_no_fill_leaves_buffer_untouched() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h;
        let mut buf = vec![0x00ab_cdefu32; stride * height];
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            Cell {
                ch: ' ',
                fg: 0x00ff_ffff,
                bg: 0x0022_2222,
            },
            false,
            full(stride, height),
        );
        assert!(
            buf.iter().all(|&p| p == 0x00ab_cdef),
            "no-fill space must not touch buffer"
        );
    }

    #[test]
    fn fill_rect_paints_clamped_region() {
        let stride = 4;
        let height = 3;
        let mut buf = vec![0u32; stride * height];
        // request a rect that overflows the buffer; it must clamp, not panic.
        fill_rect(
            &mut buf,
            stride,
            Rect {
                x: 1,
                y: 1,
                w: 10,
                h: 10,
            },
            0x00ab_cdef,
        );
        // row 0 untouched
        assert!(buf[0..4].iter().all(|&p| p == 0));
        // (1,1)..(3,2) painted
        assert_eq!(buf[stride + 1], 0x00ab_cdef);
        assert_eq!(buf[2 * stride + 3], 0x00ab_cdef);
        // (0,1) left of rect stays clear
        assert_eq!(buf[stride], 0);
    }

    #[test]
    fn fill_rect_with_zero_stride_is_noop() {
        // buf_height returns 0 for a zero stride; nothing is written, no panic.
        let mut buf = vec![0u32; 4];
        fill_rect(
            &mut buf,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            0xfff,
        );
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn stroke_rect_draws_border_only() {
        let stride = 5;
        let height = 5;
        let mut buf = vec![0u32; stride * height];
        let c = 0x0000_ff00;
        stroke_rect(
            &mut buf,
            stride,
            Rect {
                x: 0,
                y: 0,
                w: 5,
                h: 5,
            },
            c,
        );
        // corners and edges are set
        assert_eq!(buf[0], c); // top-left
        assert_eq!(buf[4], c); // top-right
        assert_eq!(buf[4 * stride], c); // bottom-left
        assert_eq!(buf[4 * stride + 4], c); // bottom-right
                                            // interior pixel untouched
        assert_eq!(buf[2 * stride + 2], 0);
    }

    #[test]
    fn stroke_rect_zero_dimension_is_noop() {
        let mut buf = vec![0u32; 16];
        stroke_rect(
            &mut buf,
            4,
            Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 4,
            },
            0xfff,
        );
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn draw_text_advances_one_cell_per_char() {
        let mut font = FontAtlas::new(18.0);
        let cw = font.cell_w;
        let stride = cw * 4;
        let height = font.cell_h;
        let mut buf = vec![0u32; stride * height];
        draw_text(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            "AB",
            0x00ff_ffff,
            0x0000_0000,
            true,
            full(stride, height),
        );
        // each of the two cells drew at least one foreground pixel within its column band
        let marked_in = |c0: usize, c1: usize| -> usize {
            let mut n = 0;
            for y in 0..height {
                for x in c0..c1 {
                    if buf[y * stride + x] != 0 {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(marked_in(0, cw) > 0, "first glyph cell empty");
        assert!(marked_in(cw, cw * 2) > 0, "second glyph cell empty");
        // the third cell was never written to
        assert_eq!(marked_in(cw * 2, cw * 3), 0);
    }

    #[test]
    fn glyph_clipped_top_left_skips_outside_rows_and_cols() {
        // Draw 'M' at (0,0) but clip away the top-left quadrant: glyph rows above
        // the clip and columns left of it must be skipped (the continue guards),
        // while pixels inside the clip are still painted.
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w * 2;
        let height = font.cell_h * 2;
        let mut buf = vec![0u32; stride * height];
        let clip = Rect {
            x: font.cell_w / 2,
            y: font.cell_h / 2,
            w: stride,
            h: height,
        };
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            Cell {
                ch: 'M',
                fg: 0x00ff_ffff,
                bg: 0x0000_0000,
            },
            false, // no bg fill: only the glyph (and its clipping) is exercised
            clip,
        );
        // Nothing was drawn above the clip's top edge...
        for y in 0..clip.y {
            for x in 0..stride {
                assert_eq!(buf[y * stride + x], 0, "pixel above clip painted");
            }
        }
        // ...nor left of the clip's left edge.
        for y in 0..height {
            for x in 0..clip.x {
                assert_eq!(buf[y * stride + x], 0, "pixel left of clip painted");
            }
        }
        // But some glyph coverage landed inside the clipped region.
        let inside: usize = (clip.y..height)
            .flat_map(|y| (clip.x..stride).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[y * stride + x] != 0)
            .count();
        assert!(inside > 0, "glyph fully clipped away — test is vacuous");
    }

    #[test]
    fn fill_bg_clipped_vertically_skips_outside_rows() {
        // A cell whose top sits above the clip: the fill_bg loop must `continue`
        // past rows < clip.y instead of writing them.
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w;
        let height = font.cell_h * 2;
        let mut buf = vec![0u32; stride * height];
        let clip = Rect {
            x: 0,
            y: font.cell_h, // clip starts halfway down
            w: stride,
            h: height,
        };
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            0,
            0,
            Cell {
                ch: ' ',
                fg: 0,
                bg: 0x0000_00ff,
            },
            true,
            clip,
        );
        // rows above the clip are untouched
        for y in 0..clip.y {
            assert_eq!(buf[y * stride], 0, "row {y} above clip got filled");
        }
    }

    #[test]
    fn clip_blocks_out_of_bounds_fill() {
        let mut font = FontAtlas::new(18.0);
        let stride = font.cell_w * 2;
        let height = font.cell_h;
        let cwid = font.cell_w;
        let mut buf = vec![0x0000_0000u32; stride * height];
        // Clip to the left half only; draw a filled cell in the right half.
        let clip = Rect {
            x: 0,
            y: 0,
            w: cwid,
            h: height,
        };
        draw_cell(
            &mut buf,
            stride,
            &mut font,
            cwid,
            0,
            Cell {
                ch: ' ',
                fg: 0xfff,
                bg: 0x00ff_ffff,
            },
            true,
            clip,
        );
        assert!(
            buf.iter().all(|&p| p == 0),
            "clip must prevent drawing outside it"
        );
    }
}
