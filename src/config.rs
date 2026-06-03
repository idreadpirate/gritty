// Configuration surface for gritty (CA-10).
// Reads an optional `%APPDATA%\gritty\config.toml` at startup.
// Missing or invalid files fall back to compiled-in defaults — the
// "just works" story is preserved.  Loaded once at startup (CA-37) and threaded
// into the runtime theme, initial font size, and per-pane shell/scrollback.

use std::path::PathBuf;

/// Cap the config file at 64 KiB before parsing to guard against a crafted or
/// accidentally-huge file causing a large allocation at startup.
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// All user-tunable settings.  Every key is optional in the file — anything
/// absent keeps its `Default::default()` value, so a partial file (e.g. just
/// `font_size = 20.0`) works without listing every key.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Rasterisation size for the primary font, in logical pixels.  Defaults to
    /// `app::DEFAULT_FONT_PX` (the single source of truth); tune live via
    /// `Ctrl +/-/0` or set `font_size` here.
    pub font_size: f32,

    /// Lines of scrollback kept per pane.  Mirrors `scrolling_history: 5000`
    /// in `term.rs`.
    pub scrollback: usize,

    /// Override the shell launched for each pane.  `None` → let
    /// `portable-pty` pick the system default (`COMSPEC` / `cmd.exe`).
    pub shell: Option<String>,

    /// Foreground text colour as `0x00RRGGBB`.  `None` → built-in default.
    pub fg: Option<u32>,

    /// Background fill colour as `0x00RRGGBB`.  `None` → built-in default.
    pub bg: Option<u32>,

    /// Accent / border colour as `0x00RRGGBB`.  `None` → built-in default.
    pub accent: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: crate::app::DEFAULT_FONT_PX,
            scrollback: 5000,
            shell: None,
            fg: None,
            bg: None,
            accent: None,
        }
    }
}

/// `%APPDATA%\gritty\config.toml`.
/// Returns `None` when neither `%APPDATA%` nor a temp-dir fallback can be
/// constructed (practically impossible on Windows, but we don't panic).
fn config_path() -> Option<PathBuf> {
    let mut dir = std::env::var_os("APPDATA").map(PathBuf::from)?;
    dir.push("gritty");
    dir.push("config.toml");
    Some(dir)
}

/// Load configuration from `%APPDATA%\gritty\config.toml`.
///
/// Any problem (file absent, unreadable, oversized, or invalid) silently
/// returns `Config::default()` so the app always starts with known-good values.
pub fn load() -> Config {
    config_path()
        .and_then(|p| load_from(&p))
        .unwrap_or_default()
}

/// Testable inner: load from an explicit path.
fn load_from(path: &std::path::Path) -> Option<Config> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_CONFIG_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    parse(&text)
}

/// Minimal parser for gritty's flat `key = value` config — a tiny TOML subset:
/// blank lines, `#` comments (incl. inline), and `key = value` scalar pairs (no
/// tables/arrays). Hand-rolled so the runtime binary doesn't link the full
/// `toml` crate (+`toml_edit`+`winnow`) just to read six scalars — that pulled
/// ~100 KB into a "lightweight" terminal. Returns `None` on a syntactically
/// invalid line (mirrors the strict reject the old `toml::from_str` gave on
/// garbage); unknown keys are ignored, as TOML would.
fn parse(text: &str) -> Option<Config> {
    let mut cfg = Config::default();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let (key, val) = line.split_once('=')?; // no '=' on a non-blank line → invalid
        let (key, val) = (key.trim(), val.trim());
        match key {
            "font_size" => cfg.font_size = val.parse().ok()?,
            "scrollback" => cfg.scrollback = val.parse().ok()?,
            "shell" => cfg.shell = Some(parse_string(val)?),
            "fg" => cfg.fg = Some(parse_u32(val)?),
            "bg" => cfg.bg = Some(parse_u32(val)?),
            "accent" => cfg.accent = Some(parse_u32(val)?),
            _ => {} // unknown key: ignore
        }
    }
    Some(cfg)
}

/// Trim an inline `#` comment, but not one inside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, b) in line.bytes().enumerate() {
        match b {
            b'"' => in_str = !in_str,
            b'#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Parse a TOML basic string `"..."` with `\\`, `\"`, `\n`, `\t` escapes.
fn parse_string(v: &str) -> Option<String> {
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Parse a `u32` written as decimal or `0x`-prefixed hex.
fn parse_u32(v: &str) -> Option<u32> {
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => v.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A partial file should override the keys it mentions and leave the rest
    /// at their defaults.
    #[test]
    fn partial_config_overrides_and_defaults() {
        let src = r#"
            font_size = 24.0
            fg = 0xFFFFFF
        "#;
        let cfg = parse(src).expect("parse");
        assert_eq!(cfg.font_size, 24.0);
        assert_eq!(cfg.fg, Some(0x00FF_FFFF));
        // Unmentioned fields fall back to defaults.
        assert_eq!(cfg.scrollback, Config::default().scrollback);
        assert_eq!(cfg.shell, None);
        assert_eq!(cfg.bg, None);
        assert_eq!(cfg.accent, None);
    }

    /// A fully-specified file should round-trip every field.
    #[test]
    fn full_config_roundtrip() {
        let src = r#"
            font_size = 16.0
            scrollback = 10000
            shell = "C:\\Windows\\System32\\pwsh.exe"
            fg = 0xF0F0F0
            bg = 0x181818
            accent = 0xFF7B00
        "#;
        let cfg = parse(src).expect("parse");
        assert_eq!(cfg.font_size, 16.0);
        assert_eq!(cfg.scrollback, 10000);
        assert_eq!(
            cfg.shell.as_deref(),
            Some("C:\\Windows\\System32\\pwsh.exe")
        );
        assert_eq!(cfg.fg, Some(0xF0_F0F0));
        assert_eq!(cfg.bg, Some(0x18_1818));
        assert_eq!(cfg.accent, Some(0xFF_7B00));
    }

    /// An empty file gives all defaults.
    #[test]
    fn empty_config_gives_defaults() {
        let cfg = parse("").expect("parse");
        assert_eq!(cfg, Config::default());
    }

    /// An inline `#` comment is ignored; a `#` inside a quoted string is kept.
    #[test]
    fn inline_comment_stripped_but_not_in_string() {
        let cfg = parse("font_size = 20.0 # cozy\nshell = \"C:\\\\a#b\\\\sh.exe\"").expect("parse");
        assert_eq!(cfg.font_size, 20.0);
        assert_eq!(cfg.shell.as_deref(), Some("C:\\a#b\\sh.exe"));
    }

    /// Garbage input must not panic — `parse` returns `None`.
    #[test]
    fn garbage_yields_none() {
        assert!(parse("[[[[not valid").is_none());
        // A known key with an unparseable value is also rejected.
        assert!(parse("scrollback = not_a_number").is_none());
    }

    /// Unknown keys are ignored (forward-compat), not errors.
    #[test]
    fn unknown_keys_ignored() {
        let cfg = parse("future_knob = 7\nfont_size = 12.0").expect("parse");
        assert_eq!(cfg.font_size, 12.0);
    }

    /// A file larger than MAX_CONFIG_BYTES must be rejected before parsing.
    #[test]
    fn oversize_file_yields_default() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gritty_cfg_big_{}.toml", std::process::id()));
        std::fs::write(&path, vec![b'#'; (MAX_CONFIG_BYTES + 1) as usize]).unwrap();
        assert!(load_from(&path).is_none());
        std::fs::remove_file(&path).ok();
    }

    /// A valid file round-trips through `load_from`.
    #[test]
    fn valid_file_loads() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gritty_cfg_ok_{}.toml", std::process::id()));
        std::fs::write(&path, b"scrollback = 1234\n").unwrap();
        let cfg = load_from(&path).expect("load_from should succeed");
        assert_eq!(cfg.scrollback, 1234);
        std::fs::remove_file(&path).ok();
    }

    /// `config_path()` must end with the expected relative suffix.
    #[test]
    fn config_path_suffix() {
        if let Some(p) = config_path() {
            let ok = p.ends_with("gritty/config.toml") || p.ends_with("gritty\\config.toml");
            assert!(ok, "unexpected path: {p:?}");
        }
    }
}
