# Changelog

All notable changes to gritty.

## [Unreleased]

### Install & lifecycle
- One-line PowerShell installer (`scripts/install.ps1`): downloads the released
  exe, installs under `%LOCALAPPDATA%\Programs\gritty`, adds Start Menu + Desktop
  shortcuts and PATH; matching `uninstall.ps1` and `release.ps1` (gate → build →
  publish exe + SHA256).
- Detaches from the launching shell on startup (`DETACHED_PROCESS` +
  break-away-from-job), so closing the terminal that started gritty no longer
  kills its panes.
- Session is now saved on *every* exit path and on rename — tab and pane names
  persist no matter how gritty is closed, not only via the window close button.
- Repaint on window re-focus / un-occlude, fixing stale pixels after alt-tab.

### Multiplexer & UX
- **Multi-window tab tear-off**: drag a tab off the bar (or `Ctrl+Shift+N`, or the
  "move tab to new window" command) to pop it into its own OS window — carrying
  its live panes/PTYs — so tabs can live on different monitors. Each window has
  independent tabs, focus, and broadcast/seamless state; the session save/restore
  reopens every window at its screen position.
- Tabs and recursive split panes (binary layout tree) with per-pane names and
  per-tab accent colors.
- Command palette (`Ctrl+Shift+P`, fuzzy) and keybinding help overlay (`F1`).
- Seamless mode (hide chrome, glow the focused pane).
- Pane resize three ways: drag divider, `Ctrl+Alt+Arrows`, `Ctrl+Mouse-wheel`.
- Font zoom (`Ctrl +/-/0`); double-click word / triple-click line selection.
- Tab `×`/`+` mouse affordances; click-to-switch; resize cursor on divider hover.

### Terminal fidelity
- Full xterm key encoding: F-keys, modified arrows, Alt-as-ESC, Ctrl-masking.
- SGR attributes: reverse, bold, dim, hidden, underline; wide CJK/emoji glyphs.
- Mouse reporting to applications (vim/htop/fzf); OSC-8 Ctrl-click hyperlinks;
  OSC-0/2 title capture; OSC-7 cwd inheritance on split; visual bell.
- Scrollback with a position indicator.

### Persistence & config
- Session save/restore (layout, names, colors, window geometry).
- Optional `%APPDATA%\gritty\config.toml`.

### Security & robustness (audited)
- Paste sanitization (control/escape injection, bracketed-paste end-marker).
- Absolute, existence-checked shell paths (no PATH hijack).
- Session-restore caps + tree/pane/focus reconciliation (no mass-spawn / freeze).
- `Pane::new -> Result` with a native error dialog instead of a silent abort.
- Embedded fallback font + no-panic glyph path; graceful surface device-loss.
- Bounded PTY backpressure; coalesced wakes; ~120 fps frame cap; glyph-cache cap.
- Atomic ordering fix (no zombie panes on weak memory).
- Dropped unmaintained `serial` dependency (RUSTSEC-2017-0008) via portable-pty 0.9.
- WCAG-AA UI contrast; gamma-correct text blending.

### Foundations
- CPU rendering (winit + softbuffer), fontdue glyph cache.
- ConPTY via portable-pty; VT engine via alacritty_terminal.
- `main.rs` split into `app` / `input` / `paint` modules.
- 174 tests; quality gate (fmt + clippy `-D` + tests + size/dep budgets).

### Deferred (tracked)
- Per-cell damage-tracking repaint (perf; idle CPU already bounded).
- UI-Automation screen-reader provider (dedicated a11y effort).
