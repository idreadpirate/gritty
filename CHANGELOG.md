# Changelog

All notable changes to gritty.

## [Unreleased]

### Added
- Session save/restore: layout, names, and colors persist across restarts
  (auto-save on close, auto-restore on launch, plus palette commands).
- Process-aware pane headers showing the foreground process (e.g. `editor: nvim`).
- Command palette (`Ctrl+Shift+P`) with fuzzy search.
- Broadcast input to all panes in a tab.
- Seamless mode (hide chrome; glow on focused pane only).
- Mouse drag-to-resize pane borders; click tabs to switch.
- Per-tab neon accent colors.
- Pane resize via `Ctrl+Alt+Arrows`.
- Auto-close panes/tabs when their shell exits.

### Core
- Tabs and split panes (binary split-tree) with per-pane names.
- Scrollback (wheel; jump-to-bottom on input).
- Copy/paste: drag auto-copy, `Ctrl+Shift+C/V`, right-click, bracketed paste.
- Railway-style cached glow + dotted-grid background (zero per-frame cost).
- Branding: window + embedded exe icon, pink accent.
- Interactive terminal: ConPTY shell, alacritty VT engine, CPU glyph rendering.
