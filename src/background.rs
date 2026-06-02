// Decorative base layer: a soft radial glow + faint dotted grid (Railway feel).
// Baked into a cache and recomputed only on resize, so per-frame cost is a
// single memcpy. Text with the default background is drawn transparently over
// this, so the glow shows through behind the terminal content.

pub struct Background {
    pub px: Vec<u32>,
    w: usize,
    h: usize,
}

// Edge color (deep near-black) and center glow (subtle indigo).
const EDGE: (i32, i32, i32) = (0x10, 0x10, 0x16);
const GLOW: (i32, i32, i32) = (0x20, 0x1e, 0x30);
// Dotted grid.
const DOT_ADD: (i32, i32, i32) = (0x12, 0x12, 0x1c);
const DOT_SPACING: usize = 22;

impl Background {
    pub fn new() -> Self {
        Self {
            px: Vec::new(),
            w: 0,
            h: 0,
        }
    }

    /// Recompute the cache if the size changed.
    pub fn resize(&mut self, w: usize, h: usize) {
        if w == self.w && h == self.h && !self.px.is_empty() {
            return;
        }
        self.w = w;
        self.h = h;
        self.px = vec![0u32; w * h];

        // Glow centered horizontally, biased toward the top third.
        let cx = w as f32 * 0.5;
        let cy = h as f32 * 0.32;
        // Farthest-corner squared distance for normalization (no sqrt per pixel).
        let max_d2 = [
            (0.0, 0.0),
            (w as f32, 0.0),
            (0.0, h as f32),
            (w as f32, h as f32),
        ]
        .iter()
        .map(|(x, y)| {
            let dx = x - cx;
            let dy = y - cy;
            dx * dx + dy * dy
        })
        .fold(1.0f32, f32::max);

        for y in 0..h {
            for x in 0..w {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let n = (dx * dx + dy * dy) / max_d2; // 0 center .. 1 edge
                let t = (1.0 - n).clamp(0.0, 1.0);
                let t = t * t; // concentrate the glow toward the center

                let mut r = lerp(EDGE.0, GLOW.0, t);
                let mut g = lerp(EDGE.1, GLOW.1, t);
                let mut b = lerp(EDGE.2, GLOW.2, t);

                if x % DOT_SPACING == 0 && y % DOT_SPACING == 0 {
                    r = (r + DOT_ADD.0).min(255);
                    g = (g + DOT_ADD.1).min(255);
                    b = (b + DOT_ADD.2).min(255);
                }

                self.px[y * w + x] = rgb(r, g, b);
            }
        }
    }
}

fn lerp(a: i32, b: i32, t: f32) -> i32 {
    a + ((b - a) as f32 * t).round() as i32
}

fn rgb(r: i32, g: i32, b: i32) -> u32 {
    let c = |v: i32| v.clamp(0, 255) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_fills_cache() {
        let mut bg = Background::new();
        bg.resize(40, 30);
        assert_eq!(bg.px.len(), 40 * 30);
        // Glow falloff: a center pixel is brighter than an edge pixel.
        // Both chosen off the dot grid (coords not multiples of DOT_SPACING).
        let center = bg.px[10 * 40 + 20]; // (20,10)
        let edge = bg.px[29 * 40 + 39]; // (39,29)
        assert!(
            center > edge,
            "center {center:#x} not brighter than edge {edge:#x}"
        );
    }
}
