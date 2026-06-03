// Decorative base layer: a soft radial glow + faint dotted grid (Railway feel).
// Baked into a cache and recomputed only on resize, so per-frame cost is a
// single memcpy. Text with the default background is drawn transparently over
// this, so the glow shows through behind the terminal content.

pub struct Background {
    pub px: Vec<u32>,
    w: usize,
    h: usize,
    // CA-36: the radial glow is O(w·h) per-pixel math. Recomputing it for every
    // intermediate size of a live drag-resize pegs a core on a maximized 4K
    // window. To debounce, we keep the last *exact* glow (`base`/`base_w`/`base_h`)
    // and, while the size is still moving, cheaply scale it to fit instead of
    // recomputing. The exact recompute only runs once the requested size has held
    // steady for `SETTLE_FRAMES` consecutive frames (drag has stopped) or on the
    // very first sizing (no base to scale from yet).
    base: Vec<u32>,
    base_w: usize,
    base_h: usize,
    // `true` when `px` is the exact glow for `(w, h)`; `false` when `px` is a
    // scaled approximation awaiting the settle recompute.
    exact: bool,
    // Consecutive frames the requested size has been unchanged while still
    // approximate. Resets to 0 whenever the size moves.
    settle: u8,
}

// Edge (deep indigo-charcoal, matches BG) and center glow (desaturated plum).
const EDGE: (i32, i32, i32) = (0x12, 0x11, 0x1a);
const GLOW: (i32, i32, i32) = (0x22, 0x1e, 0x2e);
// Dotted grid.
const DOT_ADD: (i32, i32, i32) = (0x16, 0x12, 0x1c);
const DOT_SPACING: usize = 22;
// CA-36: number of consecutive same-size frames after which the size is treated
// as settled and the exact glow is recomputed. At ~60fps a drag pushes a fresh
// size every frame, so a couple of identical frames reliably means the drag
// stopped.
const SETTLE_FRAMES: u8 = 2;

impl Background {
    pub fn new() -> Self {
        Self {
            px: Vec::new(),
            w: 0,
            h: 0,
            base: Vec::new(),
            base_w: 0,
            base_h: 0,
            exact: false,
            settle: 0,
        }
    }

    /// Make `px` a `w×h` buffer holding the glow for that size.
    ///
    /// On a cold start, or once the requested size has held steady for
    /// `SETTLE_FRAMES` frames, this is the exact O(w·h) radial-glow compute.
    /// While the size is still moving (continuous drag-resize) it instead cheaply
    /// scales the last exact glow to fit, so a drag does at most one expensive
    /// recompute when it stops rather than one per intermediate size (CA-36).
    pub fn resize(&mut self, w: usize, h: usize) {
        if w == self.w && h == self.h && !self.px.is_empty() {
            if self.exact {
                return; // exact glow already cached for this size — nothing to do.
            }
            // Size has held steady while still approximate: count toward settle,
            // and recompute exactly once it's been stable long enough.
            self.settle = self.settle.saturating_add(1);
            if self.settle >= SETTLE_FRAMES {
                self.recompute_exact();
            }
            return;
        }

        // Size changed (or first sizing).
        self.w = w;
        self.h = h;
        self.settle = 0;

        if self.base.is_empty() || w == 0 || h == 0 {
            // No exact glow to scale from yet (cold start) — compute it directly.
            self.recompute_exact();
        } else {
            // Mid-drag: cheap nearest-neighbour scale of the last exact glow.
            self.px = scale_nearest(&self.base, self.base_w, self.base_h, w, h);
            self.exact = false;
        }
    }

    /// Compute the exact radial glow for the current `(w, h)` and cache it as the
    /// new scaling base.
    fn recompute_exact(&mut self) {
        self.px = compute_glow(self.w, self.h);
        self.base.clone_from(&self.px);
        self.base_w = self.w;
        self.base_h = self.h;
        self.exact = true;
        self.settle = 0;
    }
}

/// The exact radial glow + dotted grid for a `w×h` buffer. Pure: depends only on
/// the dimensions, so it is trivially unit-testable.
fn compute_glow(w: usize, h: usize) -> Vec<u32> {
    let mut px = vec![0u32; w * h];

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

            px[y * w + x] = rgb(r, g, b);
        }
    }
    px
}

/// Nearest-neighbour resample of a `sw×sh` pixel buffer into a fresh `dw×dh`
/// buffer. Cheap (one read + one write per destination pixel, no float math) and
/// used as the mid-drag approximation of the glow (CA-36). Pure.
fn scale_nearest(src: &[u32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u32> {
    let mut dst = vec![0u32; dw * dh];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh {
        return dst;
    }
    for y in 0..dh {
        // Map destination row to nearest source row, clamped into range.
        let sy = (y * sh / dh).min(sh - 1);
        let src_row = sy * sw;
        let dst_row = y * dw;
        for x in 0..dw {
            let sx = (x * sw / dw).min(sw - 1);
            dst[dst_row + x] = src[src_row + sx];
        }
    }
    dst
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

    #[test]
    fn resize_to_same_size_is_cached_noop() {
        let mut bg = Background::new();
        bg.resize(40, 30);
        let snapshot = bg.px.clone();
        // Re-resizing to the identical dimensions hits the early return and must
        // not reallocate or alter the cached pixels.
        bg.resize(40, 30);
        assert_eq!(bg.px, snapshot);
        // A genuine size change does recompute (length follows the new size).
        bg.resize(20, 20);
        assert_eq!(bg.px.len(), 20 * 20);
    }

    #[test]
    fn rgb_and_lerp_clamp_and_interpolate() {
        assert_eq!(lerp(0, 100, 0.0), 0);
        assert_eq!(lerp(0, 100, 1.0), 100);
        assert_eq!(lerp(0, 100, 0.5), 50);
        // rgb clamps out-of-range channels into 0..=255.
        assert_eq!(rgb(-5, 300, 128), (255 << 8) | 128);
    }

    // --- CA-36: glow recompute is debounced during continuous resize ----------

    #[test]
    fn cold_start_computes_exact_glow() {
        // First sizing has no base to scale from, so it must compute exactly.
        let mut bg = Background::new();
        bg.resize(40, 30);
        assert!(bg.exact, "cold start must be an exact glow");
        assert_eq!(bg.px, compute_glow(40, 30));
    }

    #[test]
    fn mid_drag_scales_instead_of_recomputing() {
        // After an exact glow, a fresh size during a drag must NOT recompute the
        // exact O(w·h) glow — it scales the cached base and stays approximate.
        let mut bg = Background::new();
        bg.resize(40, 30); // exact base
        bg.resize(80, 60); // first frame at a new size: scale, don't recompute
        assert_eq!(bg.px.len(), 80 * 60, "buffer must match requested size");
        assert!(
            !bg.exact,
            "a fresh drag size must stay approximate (scaled)"
        );
        // It is the cheap scale, not the exact glow.
        assert_ne!(
            bg.px,
            compute_glow(80, 60),
            "mid-drag frame must not be the exact recompute"
        );
        assert_eq!(bg.px, scale_nearest(&compute_glow(40, 30), 40, 30, 80, 60));
    }

    #[test]
    fn settled_size_recomputes_exact_glow() {
        // Once the requested size holds steady for SETTLE_FRAMES, the exact glow
        // is recomputed so the final rendered frame is pixel-correct.
        let mut bg = Background::new();
        bg.resize(40, 30); // exact base
        bg.resize(80, 60); // size changed: scale (approximate)
        assert!(!bg.exact);
        // Hold the size steady; after SETTLE_FRAMES same-size calls it recomputes.
        for _ in 0..SETTLE_FRAMES {
            bg.resize(80, 60);
        }
        assert!(bg.exact, "a settled size must recompute the exact glow");
        assert_eq!(bg.px, compute_glow(80, 60));
    }

    #[test]
    fn scale_nearest_preserves_corners_and_size() {
        // 2x2 -> 4x4 nearest scale keeps the corner colours and fills the buffer.
        let src = vec![0x01u32, 0x02, 0x03, 0x04]; // [a b / c d]
        let dst = scale_nearest(&src, 2, 2, 4, 4);
        assert_eq!(dst.len(), 16);
        assert_eq!(dst[0], 0x01, "top-left corner");
        assert_eq!(dst[4 * 4 - 1], 0x04, "bottom-right corner");
    }

    #[test]
    fn scale_nearest_degenerate_sizes_are_safe() {
        // Zero source/dest dimensions must not panic and yield the right length.
        assert_eq!(scale_nearest(&[], 0, 0, 4, 4).len(), 16);
        assert!(scale_nearest(&[1, 2, 3, 4], 2, 2, 0, 5).is_empty());
    }
}
