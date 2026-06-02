// Map alacritty/vte colors to 0x00RRGGBB framebuffer pixels.

use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

pub const FG: u32 = 0x00d0_d0d0;
pub const BG: u32 = 0x0018_1818;
pub const CURSOR: u32 = 0x00ff_3d9a; // gritty pink accent (from the logo)
pub const SELECTION_BG: u32 = 0x0050_2a44;
pub const ACCENT: u32 = 0x00ff_3d9a; // focused pane / active tab
pub const UI_BAR_BG: u32 = 0x0014_141a; // tab bar
pub const UI_TITLE_BG: u32 = 0x001e_1e26; // inactive pane title
pub const UI_DIM: u32 = 0x00a0_a0a8; // inactive UI text

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Standard 16-color ANSI palette (indices 0..=15).
const ANSI16: [u32; 16] = [
    0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
    0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
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
        Foreground | BrightForeground => FG,
        DimForeground => 0x808080,
        Background => BG,
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
                if v == 0 { 0 } else { 55 + v * 40 }
            };
            rgb(conv(i / 36), conv((i % 36) / 6), conv(i % 6))
        }
        232..=255 => {
            let v = 8 + (i - 232) * 10;
            rgb(v, v, v)
        }
    }
}
