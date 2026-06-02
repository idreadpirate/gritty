// Monospace font loading + a lazy glyph raster cache (fontdue).
//
// Font selection (CA-2): tries system monospace candidates first; if none
// are readable, falls back to the embedded VT323 TTF (OFL-1.1, Peter Hull,
// https://fonts.google.com/specimen/VT323) so `FontAtlas::new` never panics.
//
// Missing-glyph behaviour (CA-9): fontdue returns width=0 / advance>0 for
// glyphs not present in the active font.  The glyph cache stores and returns
// those entries as-is (an empty coverage bitmap is a valid, sane result —
// the caller draws nothing for that cell, which is correct).  The lookup
// path is panic-free.  A full multi-font fallback chain (bold/italic faces,
// symbol/CJK fallback) would require a second font slot and out-of-scope
// caller changes; that remains a future CA-9 extension.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};

/// Embedded last-resort font (VT323 Regular, OFL-1.1).
/// Keeps `FontAtlas::new` from panicking when no system font is available.
static FALLBACK_FONT_BYTES: &[u8] = include_bytes!("../assets/fallback.ttf");

pub struct FontAtlas {
    font: Font,
    px: f32,
    pub cell_w: usize,
    pub cell_h: usize,
    pub ascent: f32,
    cache: HashMap<char, (Metrics, Vec<u8>)>,
}

impl FontAtlas {
    pub fn new(px: f32) -> Self {
        let bytes = load_font_bytes();
        // SAFETY: `bytes` is either a successfully-read system TTF or the
        // embedded fallback which is known-good; unwrap is safe in both cases.
        let font =
            Font::from_bytes(bytes.as_slice(), FontSettings::default()).unwrap_or_else(|_| {
                // System font parsed but fontdue rejected it (malformed); fall
                // back to the embedded font which is always valid.
                Font::from_bytes(FALLBACK_FONT_BYTES, FontSettings::default())
                    .expect("embedded fallback font must be valid")
            });

        let lm = font
            .horizontal_line_metrics(px)
            .unwrap_or_else(|| fontdue::LineMetrics {
                ascent: px * 0.8,
                descent: -(px * 0.2),
                line_gap: 0.0,
                new_line_size: px,
            });
        let ascent = lm.ascent;
        let cell_h = (lm.ascent - lm.descent + lm.line_gap).ceil().max(1.0) as usize;

        // Monospace advance width from a representative glyph.
        let (m, _) = font.rasterize('M', px);
        let cell_w = m.advance_width.ceil().max(1.0) as usize;

        Self {
            font,
            px,
            cell_w,
            cell_h,
            ascent,
            cache: HashMap::new(),
        }
    }

    /// Rasterized coverage bitmap for `ch`, cached.
    ///
    /// For glyphs absent from the loaded font fontdue returns a zero-size
    /// bitmap with a non-zero advance; this method stores and returns that
    /// entry so the caller can decide how to render it (typically: skip the
    /// blit, advance by `cell_w`).  The method never panics.
    pub fn glyph(&mut self, ch: char) -> &(Metrics, Vec<u8>) {
        // Bound the cache so hostile/long output emitting many distinct
        // codepoints can't grow it without limit (RT-12). ASCII fits easily;
        // when the dynamic tail overflows, drop it wholesale (cheap, rare).
        if self.cache.contains_key(&ch) {
            return &self.cache[&ch];
        }
        if self.cache.len() >= GLYPH_CACHE_CAP {
            self.cache.clear();
        }
        let g = self.font.rasterize(ch, self.px);
        self.cache.insert(ch, g);
        // Key is now present — indexing cannot panic here.
        &self.cache[&ch]
    }
}

/// Max distinct glyphs kept rasterized. ~4k covers ASCII + a working set of
/// CJK/symbols; beyond it we reset rather than grow toward all of Unicode.
const GLYPH_CACHE_CAP: usize = 4096;

/// Returns font bytes, preferring system fonts for quality; embedded VT323
/// as the guaranteed last resort (CA-2: no panic on missing system font).
fn load_font_bytes() -> Vec<u8> {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\CascadiaMono.ttf",
        r"C:\Windows\Fonts\CascadiaCode.ttf",
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
        r"C:\Windows\Fonts\cour.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return bytes;
        }
    }
    // No system font found — use the embedded fallback (VT323, OFL-1.1).
    FALLBACK_FONT_BYTES.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_cache_is_bounded() {
        let mut atlas = FontAtlas::new(18.0);
        // Rasterize far more distinct codepoints than the cap.
        for cp in 0x4E00u32..0x4E00 + (GLYPH_CACHE_CAP as u32 + 2000) {
            if let Some(c) = char::from_u32(cp) {
                let _ = atlas.glyph(c);
            }
        }
        assert!(
            atlas.cache.len() <= GLYPH_CACHE_CAP,
            "cache {} exceeded cap {}",
            atlas.cache.len(),
            GLYPH_CACHE_CAP
        );
    }

    /// CA-2: atlas must build even when no system font path exists.
    /// We exercise this by constructing directly from the embedded bytes.
    #[test]
    fn atlas_builds_from_embedded_fallback() {
        let font = Font::from_bytes(FALLBACK_FONT_BYTES, FontSettings::default())
            .expect("embedded fallback must parse");
        let lm = font.horizontal_line_metrics(18.0);
        assert!(lm.is_some(), "embedded font must have line metrics");
        let (m, _) = font.rasterize('A', 18.0);
        // advance_width must be positive for the fallback to be usable
        assert!(
            m.advance_width > 0.0,
            "embedded font advance_width must be positive"
        );
    }

    /// CA-9: glyph() must not panic and must return a sane entry for a
    /// missing glyph (width may be 0 but advance_width should be >= 0).
    #[test]
    fn missing_glyph_does_not_panic() {
        let mut atlas = FontAtlas::new(18.0);
        // U+FFFD REPLACEMENT CHARACTER is rarely in monospace fonts.
        let (metrics, bitmap) = atlas.glyph('\u{FFFD}');
        // Must not panic; advance_width >= 0 is the minimal sanity check.
        assert!(
            metrics.advance_width >= 0.0,
            "missing glyph advance must be >= 0"
        );
        // bitmap length must match metrics dimensions (fontdue invariant)
        assert_eq!(
            bitmap.len(),
            metrics.width * metrics.height,
            "bitmap length must equal width*height"
        );
    }

    /// CA-9: glyph() must never panic for any ASCII character.
    #[test]
    fn ascii_glyphs_never_panic() {
        let mut atlas = FontAtlas::new(16.0);
        for c in (' '..='~').chain(['\n', '\r', '\t']) {
            let (metrics, _) = atlas.glyph(c);
            assert!(metrics.advance_width >= 0.0);
        }
    }
}
