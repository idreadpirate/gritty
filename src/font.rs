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
    /// System fallback faces for glyphs the primary monospace font lacks —
    /// UI badges (★◆), symbols, emoji, CJK. Lazily loaded (see [`Fallback`]):
    /// a session that never prints such a glyph never reads a font file.
    fallbacks: Vec<Fallback>,
}

/// One lazily-loaded fallback face. The file is read+parsed the first time a
/// glyph missing from every earlier face forces it, then kept for the process
/// lifetime (parsed faces are px-independent; only rasterization is). A failed
/// load (file absent on this Windows edition, unparseable) is remembered so
/// the disk is probed at most once per slot.
struct Fallback {
    path: &'static str,
    /// Font index inside a .ttc collection (0 for plain .ttf).
    collection_index: u32,
    state: FallbackState,
}

enum FallbackState {
    Unloaded,
    Failed,
    Loaded(Font),
}

impl Fallback {
    fn new(path: &'static str, collection_index: u32) -> Self {
        Self {
            path,
            collection_index,
            state: FallbackState::Unloaded,
        }
    }

    /// The parsed face, loading it on first use. `None` once a load has failed.
    fn face(&mut self) -> Option<&Font> {
        if matches!(self.state, FallbackState::Unloaded) {
            let settings = FontSettings {
                collection_index: self.collection_index,
                ..FontSettings::default()
            };
            self.state = std::fs::read(self.path)
                .ok()
                .and_then(|bytes| Font::from_bytes(bytes, settings).ok())
                .map(FallbackState::Loaded)
                .unwrap_or(FallbackState::Failed);
        }
        match &self.state {
            FallbackState::Loaded(f) => Some(f),
            _ => None,
        }
    }
}

/// Fallback faces tried in order for a glyph the primary font lacks. Ordered
/// small/common first so the big CJK collections are only parsed when CJK
/// text actually appears (msyh.ttc is ~19 MB resident once loaded — a cost
/// only CJK-printing sessions pay).
fn fallback_candidates() -> Vec<Fallback> {
    vec![
        // Symbols: stars, geometric shapes, arrows, misc technical.
        Fallback::new(r"C:\Windows\Fonts\seguisym.ttf", 0),
        // General UI face — broad Latin/Greek/Cyrillic + punctuation coverage.
        Fallback::new(r"C:\Windows\Fonts\segoeui.ttf", 0),
        // Emoji (rasterized from the monochrome outlines; no color layers).
        Fallback::new(r"C:\Windows\Fonts\seguiemj.ttf", 0),
        // CJK: Simplified Chinese, then Japanese, then Korean.
        Fallback::new(r"C:\Windows\Fonts\msyh.ttc", 0),
        Fallback::new(r"C:\Windows\Fonts\msgothic.ttc", 0),
        Fallback::new(r"C:\Windows\Fonts\malgun.ttf", 0),
    ]
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

        let (cell_w, cell_h, ascent) = metrics_for(&font, px);

        Self {
            font,
            px,
            cell_w,
            cell_h,
            ascent,
            cache: HashMap::new(),
            fallbacks: fallback_candidates(),
        }
    }

    /// CA-103: re-derive size-dependent metrics for a new pixel size, reusing the
    /// already-parsed font face. Clears the glyph cache (rasterization is
    /// px-specific) but does NOT re-read or re-parse the font file from disk — so
    /// repeated font-zoom keystrokes no longer hammer disk I/O + fontdue parsing.
    pub fn set_px(&mut self, px: f32) {
        // Exact bit compare avoids a needless cache clear and the float-equality
        // lint; zoom uses discrete px steps so this is a true no-op guard.
        if px.to_bits() == self.px.to_bits() {
            return;
        }
        let (cell_w, cell_h, ascent) = metrics_for(&self.font, px);
        self.px = px;
        self.cell_w = cell_w;
        self.cell_h = cell_h;
        self.ascent = ascent;
        self.cache.clear();
    }

    /// Rasterized coverage bitmap for `ch`, cached.
    ///
    /// For glyphs absent from the loaded font fontdue returns a zero-size
    /// bitmap with a non-zero advance; this method stores and returns that
    /// entry so the caller can decide how to render it (typically: skip the
    /// blit, advance by `cell_w`).  The method never panics.
    pub fn glyph(&mut self, ch: char) -> &(Metrics, Vec<u8>) {
        // Bound the cache so hostile/long output emitting many distinct
        // codepoints can't grow it without limit (RT-12). The hot ASCII working
        // set is kept permanently; only the dynamic (non-ASCII) tail is evicted
        // when the cap is hit (CA-122).
        if self.cache.contains_key(&ch) {
            return &self.cache[&ch];
        }
        if self.cache.len() >= GLYPH_CACHE_CAP {
            // CA-122: don't `clear()` the whole cache — that drops the ASCII
            // working set too, forcing a re-rasterize of every visible ASCII
            // glyph on the next frame (clear→refill→clear thrash under a Unicode
            // flood). Retain ASCII; evict only the dynamic tail.
            self.cache.retain(|k, _| k.is_ascii());
        }
        let g = self.rasterize_with_fallback(ch);
        self.cache.insert(ch, g);
        // Key is now present — indexing cannot panic here.
        &self.cache[&ch]
    }

    /// Rasterize `ch` from the primary face, or from the first fallback face
    /// that has it. Glyphs no face covers rasterize from the primary as before
    /// (an empty bitmap — the caller draws nothing, CA-9). Agents constantly
    /// print badges, box glyphs, emoji, and CJK that no single monospace font
    /// carries; before this, every such cell rendered blank.
    fn rasterize_with_fallback(&mut self, ch: char) -> (Metrics, Vec<u8>) {
        // ASCII is always in the primary monospace face — skip the lookup.
        // `lookup_glyph_index == 0` is fontdue's .notdef (glyph absent).
        if ch.is_ascii() || self.font.lookup_glyph_index(ch) != 0 {
            return self.font.rasterize(ch, self.px);
        }
        for fb in &mut self.fallbacks {
            if let Some(face) = fb.face() {
                if face.lookup_glyph_index(ch) != 0 {
                    return face.rasterize(ch, self.px);
                }
            }
        }
        self.font.rasterize(ch, self.px)
    }
}

/// Max distinct glyphs kept rasterized. ~4k covers ASCII + a working set of
/// CJK/symbols; beyond it we reset rather than grow toward all of Unicode.
const GLYPH_CACHE_CAP: usize = 4096;

/// Size-dependent monospace cell metrics — advance width, line height, and
/// ascent — for `font` at `px`. Shared by `FontAtlas::new` and `set_px`
/// (CA-103) so the two paths can never derive metrics differently.
fn metrics_for(font: &Font, px: f32) -> (usize, usize, f32) {
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
    (cell_w, cell_h, ascent)
}

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

    /// CA-122: overflowing the cache with non-ASCII glyphs must NOT evict the
    /// hot ASCII working set — otherwise every overflow re-rasterizes visible
    /// ASCII (clear→refill thrash). An ASCII glyph cached before a Unicode flood
    /// must still be present afterward, and the cache must stay bounded.
    #[test]
    fn ascii_glyph_survives_cache_overflow() {
        let mut atlas = FontAtlas::new(18.0);
        let _ = atlas.glyph('A');
        assert!(atlas.cache.contains_key(&'A'), "prime failed");
        for cp in 0x4E00u32..0x4E00 + (GLYPH_CACHE_CAP as u32 + 2000) {
            if let Some(c) = char::from_u32(cp) {
                let _ = atlas.glyph(c);
            }
        }
        assert!(atlas.cache.len() <= GLYPH_CACHE_CAP, "must stay bounded");
        assert!(
            atlas.cache.contains_key(&'A'),
            "ASCII 'A' must survive a non-ASCII cache overflow"
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

    /// Fallback chain: glyphs the primary monospace font lacks (UI badges,
    /// CJK) must rasterize with real coverage via the system fallback faces
    /// instead of rendering blank. Runs only when at least one fallback face
    /// exists on this machine (they ship with every desktop Windows).
    #[test]
    fn fallback_covers_badge_and_cjk_glyphs() {
        let mut atlas = FontAtlas::new(18.0);
        let any_fallback = atlas.fallbacks.iter_mut().any(|f| f.face().is_some());
        if !any_fallback {
            return; // headless/stripped environment — nothing to assert against
        }
        // ★ (agent attention badge) and 中 (CJK) are absent from every
        // monospace primary candidate; both must now produce coverage.
        for ch in ['★', '中'] {
            let (m, bitmap) = atlas.glyph(ch);
            assert!(
                m.width > 0 && bitmap.iter().any(|&c| c > 0),
                "{ch} should rasterize via a fallback face, got {}x{}",
                m.width,
                m.height
            );
        }
    }

    /// A glyph no face covers must still return a sane empty entry (no panic),
    /// and the cache must stay bounded with fallback glyphs in it.
    #[test]
    fn uncovered_glyph_still_sane_and_cache_stays_bounded() {
        let mut atlas = FontAtlas::new(18.0);
        // U+E0001 (deprecated tag char) exists in no shipped font face.
        let (m, bitmap) = atlas.glyph('\u{E0001}');
        assert_eq!(bitmap.len(), m.width * m.height);
        for cp in 0x4E00u32..0x4E00 + (GLYPH_CACHE_CAP as u32 + 500) {
            if let Some(c) = char::from_u32(cp) {
                let _ = atlas.glyph(c);
            }
        }
        assert!(atlas.cache.len() <= GLYPH_CACHE_CAP);
    }

    /// A failed fallback load is probed once, then remembered — no disk
    /// hammering for a font this Windows edition doesn't ship.
    #[test]
    fn failed_fallback_load_is_remembered() {
        let mut fb = Fallback::new(r"C:\Windows\Fonts\does-not-exist.ttf", 0);
        assert!(fb.face().is_none());
        assert!(
            matches!(fb.state, FallbackState::Failed),
            "a missing file must latch Failed, not stay Unloaded"
        );
        assert!(fb.face().is_none(), "second probe stays None (no re-read)");
    }

    /// CA-103: `set_px` must re-derive metrics for a new size and clear the
    /// px-specific glyph cache, reusing the already-parsed face (no disk
    /// reload). A repeat to the same size is a no-op that keeps the cache.
    #[test]
    fn set_px_rescales_metrics_and_clears_cache() {
        let mut atlas = FontAtlas::new(16.0);
        let (w0, h0) = (atlas.cell_w, atlas.cell_h);
        let _ = atlas.glyph('A');
        assert!(atlas.cache.contains_key(&'A'), "prime failed");

        atlas.set_px(32.0);
        assert!(
            atlas.cell_w > w0 && atlas.cell_h > h0,
            "metrics must scale up with px ({}x{} !> {}x{})",
            atlas.cell_w,
            atlas.cell_h,
            w0,
            h0
        );
        assert!(
            atlas.cache.is_empty(),
            "glyph cache must clear on a size change"
        );

        // Re-priming then setting the SAME size must not clear the cache.
        let _ = atlas.glyph('A');
        atlas.set_px(32.0);
        assert!(
            atlas.cache.contains_key(&'A'),
            "set_px to the same size must be a no-op (cache retained)"
        );
    }
}
