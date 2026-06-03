// Configuration surface for gritty (CA-10).
// Reads an optional `%APPDATA%\gritty\config.toml` at startup.
// Missing or invalid files fall back to compiled-in defaults — the
// "just works" story is preserved.  Loaded once at startup (CA-37) and threaded
// into the runtime theme, initial font size, and per-pane shell/scrollback.

use std::path::PathBuf;

use serde::Deserialize;

/// Cap the config file at 64 KiB before parsing to guard against a crafted or
/// accidentally-huge file causing a large allocation at startup.
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// All user-tunable settings.  Every field is optional at the TOML level —
/// `#[serde(default)]` fills in `Default::default()` for anything absent, so
/// a partial file (e.g. just `font_size = 20.0`) works without listing every
/// key.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Rasterisation size for the primary font, in pixels.  Mirrors the
    /// `FontAtlas::new(18.0)` call in `app.rs`.
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
            font_size: 18.0,
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
/// Any problem (file absent, unreadable, oversized, or invalid TOML) silently
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
    toml::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A partial TOML file should override the fields it mentions and leave the
    /// rest at their defaults.
    #[test]
    fn partial_toml_overrides_and_defaults() {
        let toml = r#"
            font_size = 24.0
            fg = 0xFFFFFF
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.font_size, 24.0);
        assert_eq!(cfg.fg, Some(0x00FF_FFFF));
        // Unmentioned fields fall back to defaults.
        assert_eq!(cfg.scrollback, Config::default().scrollback);
        assert_eq!(cfg.shell, None);
        assert_eq!(cfg.bg, None);
        assert_eq!(cfg.accent, None);
    }

    /// A fully-specified TOML file should round-trip every field.
    #[test]
    fn full_toml_roundtrip() {
        let toml = r#"
            font_size = 16.0
            scrollback = 10000
            shell = "C:\\Windows\\System32\\pwsh.exe"
            fg = 0xF0F0F0
            bg = 0x181818
            accent = 0xFF7B00
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
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

    /// An empty TOML file should give all defaults.
    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").expect("parse");
        assert_eq!(cfg, Config::default());
    }

    /// Garbage input must not panic — `load_from` returns `None`.
    #[test]
    fn garbage_toml_yields_default() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gritty_cfg_garbage_{}.toml", std::process::id()));
        std::fs::write(&path, b"[[[[not valid toml").unwrap();
        assert!(load_from(&path).is_none());
        std::fs::remove_file(&path).ok();
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
        // If APPDATA is unset the function returns None — that's fine.
    }
}
