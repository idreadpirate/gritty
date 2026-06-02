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

        Self { font, px, cell_w, cell_h, ascent, cache: HashMap::new() }
    }

    /// Rasterized coverage bitmap for `ch`, cached.
    pub fn glyph(&mut self, ch: char) -> &(Metrics, Vec<u8>) {
        if !self.cache.contains_key(&ch) {
            let g = self.font.rasterize(ch, self.px);
            self.cache.insert(ch, g);
        }
        self.cache.get(&ch).expect("just inserted")
    }
}

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
