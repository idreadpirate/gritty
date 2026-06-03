// Map alacritty/vte colors to 0x00RRGGBB framebuffer pixels.

use std::sync::OnceLock;

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

// gritty "gunmetal & amber" theme — pulled from the industrial skull icon.
// FG/BG/ACCENT are the *compiled-in defaults*; the live values (which a user's
// config.toml can override, CA-37) are read through `fg()`/`bg()`/`accent()`.
pub const FG: u32 = 0x00c9_d1d9; // steel light-grey text
pub const BG: u32 = 0x0016_151f; // deep indigo-charcoal
pub const CURSOR: u32 = 0x00ff_7b00; // molten orange (the skull's inner glow)
pub const SELECTION_BG: u32 = 0x003a_2e20; // warm gunmetal
pub const ACCENT: u32 = 0x00ff_7b00; // focused pane / active tab — molten orange
pub const UI_BAR_BG: u32 = 0x0016_151f; // tab bar — matches BG (seamless top)
pub const UI_TITLE_BG: u32 = 0x001e_1c28; // inactive pane title
                                          // UI_DIM: bumped from #8c6d47 (~3.9:1) to #b08050 to reach ~5.49:1 contrast
                                          // against BG (#16151f), satisfying WCAG AA (4.5:1) for inactive UI text.
pub const UI_DIM: u32 = 0x00b0_8050; // inactive UI text — warm bronze, ~5.49:1 vs BG
/// Subtle 1px separator between unfocused panes and below the tab strip (CA-24/CA-29).
pub const PANE_SEP: u32 = 0x002d_2b3d; // muted indigo line

/// The three user-overridable theme colors (CA-37). Each falls back to the
/// compiled-in default when `config.toml` omits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub fg: u32,
    pub bg: u32,
    pub accent: u32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: FG,
            bg: BG,
            accent: ACCENT,
        }
    }
}

impl Theme {
    /// Resolve a theme from optional config overrides, masking to the
    /// `0x00RRGGBB` framebuffer-pixel space so a stray alpha byte can't leak in.
    pub fn from_overrides(fg: Option<u32>, bg: Option<u32>, accent: Option<u32>) -> Self {
        let d = Theme::default();
        Self {
            fg: fg.map(|c| c & 0x00ff_ffff).unwrap_or(d.fg),
            bg: bg.map(|c| c & 0x00ff_ffff).unwrap_or(d.bg),
            accent: accent.map(|c| c & 0x00ff_ffff).unwrap_or(d.accent),
        }
    }
}

/// Process-wide active theme. Set once at startup from config (CA-37); read
/// everywhere through the accessors below. Immutable after the single init —
/// `set` only on the first call, later calls are ignored.
static THEME: OnceLock<Theme> = OnceLock::new();

/// Install the runtime theme (idempotent: only the first call wins).
pub fn init_theme(theme: Theme) {
    let _ = THEME.set(theme);
}

/// The active theme (defaults until `init_theme` runs).
fn theme() -> Theme {
    THEME.get().copied().unwrap_or_default()
}

/// Live foreground color (config override or default).
pub fn fg() -> u32 {
    theme().fg
}

/// Live background color (config override or default).
pub fn bg() -> u32 {
    theme().bg
}

/// Live accent color (config override or default).
pub fn accent() -> u32 {
    theme().accent
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Standard 16-color ANSI palette (indices 0..=15).
const ANSI16: [u32; 16] = [
    0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
    0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
];

/// Resolve a cell color; `default` is used for the Foreground/Background defaults.
pub fn to_rgb(c: Color, default: u32) -> u32 {
    match c {
        Color::Spec(Rgb { r, g, b }) => rgb(r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Named(n) => named(n, default),
    }
}

fn named(n: NamedColor, _default: u32) -> u32 {
    use NamedColor::*;
    match n {
        Black => ANSI16[0],
        Red => ANSI16[1],
        Green => ANSI16[2],
        Yellow => ANSI16[3],
        Blue => ANSI16[4],
        Magenta => ANSI16[5],
        Cyan => ANSI16[6],
        White => ANSI16[7],
        BrightBlack => ANSI16[8],
        BrightRed => ANSI16[9],
        BrightGreen => ANSI16[10],
        BrightYellow => ANSI16[11],
        BrightBlue => ANSI16[12],
        BrightMagenta => ANSI16[13],
        BrightCyan => ANSI16[14],
        BrightWhite => ANSI16[15],
        DimBlack => ANSI16[0],
        DimRed => 0x800000,
        DimGreen => 0x008000,
        DimYellow => 0x808000,
        DimBlue => 0x000080,
        DimMagenta => 0x800080,
        DimCyan => 0x008080,
        DimWhite => 0x808080,
        Foreground | BrightForeground => fg(),
        DimForeground => 0x808080,
        Background => bg(),
        Cursor => CURSOR,
    }
}

/// xterm 256-color palette.
fn indexed(i: u8) -> u32 {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let i = i - 16;
            let conv = |v: u8| -> u8 {
                if v == 0 {
                    0
                } else {
                    55 + v * 40
                }
            };
            rgb(conv(i / 36), conv((i % 36) / 6), conv(i % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            rgb(v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_packs_channels() {
        assert_eq!(rgb(0x12, 0x34, 0x56), 0x0012_3456);
    }

    #[test]
    fn spec_color_is_passed_through() {
        let c = Color::Spec(Rgb {
            r: 0xab,
            g: 0xcd,
            b: 0xef,
        });
        assert_eq!(to_rgb(c, FG), 0x00ab_cdef);
    }

    #[test]
    fn indexed_low_16_use_ansi_table() {
        for (i, &expected) in ANSI16.iter().enumerate() {
            assert_eq!(to_rgb(Color::Indexed(i as u8), FG), expected);
        }
    }

    #[test]
    fn indexed_color_cube_endpoints() {
        // index 16 is the cube origin: all channels zero (the v==0 branch).
        assert_eq!(to_rgb(Color::Indexed(16), FG), 0x0000_0000);
        // index 231 is the cube max: all channels 55 + 5*40 = 255.
        assert_eq!(to_rgb(Color::Indexed(231), FG), 0x00ff_ffff);
        // a mid cube value: index 53 = 16 + 37 → i=37=1*36+1 → r=1,g=0,b=1 → rgb(95,0,95).
        assert_eq!(to_rgb(Color::Indexed(16 + 37), FG), {
            let v = 55 + 40; // step 1 on r and b, step 0 on g
            rgb(v, 0, v)
        });
    }

    #[test]
    fn indexed_grayscale_ramp() {
        assert_eq!(to_rgb(Color::Indexed(232), FG), rgb(8, 8, 8));
        assert_eq!(to_rgb(Color::Indexed(255), FG), {
            let v = 8 + 23 * 10;
            rgb(v, v, v)
        });
    }

    #[test]
    fn named_standard_and_bright() {
        assert_eq!(to_rgb(Color::Named(NamedColor::Black), FG), ANSI16[0]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Red), FG), ANSI16[1]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Green), FG), ANSI16[2]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Yellow), FG), ANSI16[3]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Blue), FG), ANSI16[4]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Magenta), FG), ANSI16[5]);
        assert_eq!(to_rgb(Color::Named(NamedColor::Cyan), FG), ANSI16[6]);
        assert_eq!(to_rgb(Color::Named(NamedColor::White), FG), ANSI16[7]);
        assert_eq!(to_rgb(Color::Named(NamedColor::BrightBlack), FG), ANSI16[8]);
        assert_eq!(to_rgb(Color::Named(NamedColor::BrightRed), FG), ANSI16[9]);
        assert_eq!(
            to_rgb(Color::Named(NamedColor::BrightGreen), FG),
            ANSI16[10]
        );
        assert_eq!(
            to_rgb(Color::Named(NamedColor::BrightYellow), FG),
            ANSI16[11]
        );
        assert_eq!(to_rgb(Color::Named(NamedColor::BrightBlue), FG), ANSI16[12]);
        assert_eq!(
            to_rgb(Color::Named(NamedColor::BrightMagenta), FG),
            ANSI16[13]
        );
        assert_eq!(to_rgb(Color::Named(NamedColor::BrightCyan), FG), ANSI16[14]);
        assert_eq!(
            to_rgb(Color::Named(NamedColor::BrightWhite), FG),
            ANSI16[15]
        );
    }

    #[test]
    fn named_dim_variants() {
        assert_eq!(to_rgb(Color::Named(NamedColor::DimBlack), FG), ANSI16[0]);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimRed), FG), 0x80_0000);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimGreen), FG), 0x00_8000);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimYellow), FG), 0x80_8000);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimBlue), FG), 0x00_0080);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimMagenta), FG), 0x80_0080);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimCyan), FG), 0x00_8080);
        assert_eq!(to_rgb(Color::Named(NamedColor::DimWhite), FG), 0x80_8080);
        assert_eq!(
            to_rgb(Color::Named(NamedColor::DimForeground), FG),
            0x80_8080
        );
    }

    /// WCAG 2.1 relative luminance for a 0x00RRGGBB color (test helper).
    fn wcag_luminance(c: u32) -> f64 {
        let linearize = |raw: u32| -> f64 {
            let s = raw as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        let r = linearize((c >> 16) & 0xff);
        let g = linearize((c >> 8) & 0xff);
        let b = linearize(c & 0xff);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// WCAG 2.1 contrast ratio between two colors (test helper).
    fn wcag_contrast(c1: u32, c2: u32) -> f64 {
        let l1 = wcag_luminance(c1);
        let l2 = wcag_luminance(c2);
        let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn ui_dim_meets_wcag_aa_vs_bg() {
        // CA-30: UI_DIM must have at least 4.5:1 contrast against BG for WCAG AA.
        let ratio = wcag_contrast(UI_DIM, BG);
        assert!(
            ratio >= 4.5,
            "UI_DIM vs BG contrast {ratio:.2}:1 is below WCAG AA 4.5:1"
        );
    }

    #[test]
    fn named_theme_colors() {
        assert_eq!(to_rgb(Color::Named(NamedColor::Foreground), 0), FG);
        assert_eq!(to_rgb(Color::Named(NamedColor::BrightForeground), 0), FG);
        assert_eq!(to_rgb(Color::Named(NamedColor::Background), 0), BG);
        assert_eq!(to_rgb(Color::Named(NamedColor::Cursor), 0), CURSOR);
    }

    /// CA-37: a config override sets the corresponding theme color; an omitted
    /// override falls back to the compiled-in default, and a stray alpha byte is
    /// masked away so only `0x00RRGGBB` reaches the framebuffer.
    #[test]
    fn theme_overrides_apply_mask_and_default() {
        // All present (with a high alpha byte that must be stripped).
        let t = Theme::from_overrides(Some(0xFF12_3456), Some(0x00ab_cdef), Some(0x0000_00ff));
        assert_eq!(t.fg, 0x0012_3456, "alpha byte must be masked off fg");
        assert_eq!(t.bg, 0x00ab_cdef);
        assert_eq!(t.accent, 0x0000_00ff);

        // Partial override: only accent set, fg/bg fall back to defaults.
        let t = Theme::from_overrides(None, None, Some(0x0010_2030));
        assert_eq!(t.fg, FG);
        assert_eq!(t.bg, BG);
        assert_eq!(t.accent, 0x0010_2030);

        // No overrides at all == the default theme.
        assert_eq!(Theme::from_overrides(None, None, None), Theme::default());
    }
}

fn chan(c: u32, sh: u32) -> u32 {
    (c >> sh) & 0xff
}

/// Brighten toward white (for BOLD).
fn brighten(c: u32) -> u32 {
    let f = |sh| (chan(c, sh) * 5 / 4).min(255);
    (f(16) << 16) | (f(8) << 8) | f(0)
}

/// Average two colors (for DIM — pull fg toward bg).
fn mix(a: u32, b: u32) -> u32 {
    let f = |sh| (chan(a, sh) + chan(b, sh)) / 2;
    (f(16) << 16) | (f(8) << 8) | f(0)
}

/// Apply SGR cell flags to a (fg, bg) pair (CA-4). Returns the adjusted colors
/// plus whether an underline should be drawn.
pub fn style_flags(mut fg: u32, mut bg: u32, flags: Flags) -> (u32, u32, bool) {
    if flags.contains(Flags::DIM) {
        fg = mix(fg, bg);
    }
    if flags.contains(Flags::BOLD) {
        fg = brighten(fg);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    let underline = flags.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL);
    (fg, bg, underline)
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn inverse_swaps_fg_bg() {
        let (fg, bg, _) = style_flags(0x111111, 0x222222, Flags::INVERSE);
        assert_eq!((fg, bg), (0x222222, 0x111111));
    }

    #[test]
    fn bold_brightens_dim_darkens() {
        let (bold, _, _) = style_flags(0x808080, 0x000000, Flags::BOLD);
        assert!(chan(bold, 0) > 0x80);
        let (dim, _, _) = style_flags(0xffffff, 0x000000, Flags::DIM);
        assert!(chan(dim, 0) < 0xff);
    }

    #[test]
    fn hidden_makes_fg_match_bg() {
        let (fg, bg, _) = style_flags(0xffffff, 0x123456, Flags::HIDDEN);
        assert_eq!(fg, bg);
    }

    #[test]
    fn underline_flag_reported() {
        let (_, _, ul) = style_flags(0xfff, 0x000, Flags::UNDERLINE);
        assert!(ul);
        let (_, _, none) = style_flags(0xfff, 0x000, Flags::empty());
        assert!(!none);
    }
}
