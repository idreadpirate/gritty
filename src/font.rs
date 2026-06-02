// Monospace font loading + a lazy glyph raster cache (fontdue).
// Loads a system monospace TTF from C:\Windows\Fonts so the binary stays tiny.

use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};

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
        let font = Font::from_bytes(bytes, FontSettings::default()).expect("parse font");

        let lm = font.horizontal_line_metrics(px).expect("line metrics");
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
    pub fn glyph(&mut self, ch: char) -> &(Metrics, Vec<u8>) {
        if !self.cache.contains_key(&ch) {
            // Bound the cache so hostile/long output emitting many distinct
            // codepoints can't grow it without limit (RT-12). ASCII fits easily;
            // when the dynamic tail overflows, drop it wholesale (cheap, rare).
            if self.cache.len() >= GLYPH_CACHE_CAP {
                self.cache.clear();
            }
            let g = self.font.rasterize(ch, self.px);
            self.cache.insert(ch, g);
        }
        self.cache.get(&ch).expect("just inserted")
    }
}

/// Max distinct glyphs kept rasterized. ~4k covers ASCII + a working set of
/// CJK/symbols; beyond it we reset rather than grow toward all of Unicode.
const GLYPH_CACHE_CAP: usize = 4096;

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
    panic!("no monospace font found under C:\\Windows\\Fonts");
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
}
