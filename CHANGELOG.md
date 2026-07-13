# Changelog

All notable changes to gritty.

## [Unreleased]

### Fixed (stability / correctness)
- **Multi-pane memory leak + long-session lag** — with several tabs/panes
  streaming at once, winit's unbounded user-event queue accumulated PTY wakes
  faster than they were consumed (per-pane coalescing still let N panes enqueue
  N wakes per consumed event): linear RSS growth (~+21%/2 min at 16 flooding
  panes) plus, past the 10k Win32 message cap, silently dropped posts. A new
  app-wide `WakeCoalescer` keeps at most **one** Wake in flight. Proved by A/B
  stress: same 16-pane flood, before +21.3% RSS (linear, no plateau) → after
  +1.3% (flat); 10-minute mixed multi-tab run +0.8%, PASS.
- **Hard freeze under sustained floods** — `drain_pty` raced the reader threads
  unboundedly, so a flood could pin the UI thread inside one drain and starve
  input/redraw. Draining is now budgeted (~2 MB/pane/cycle) with a self-wake
  for the backlog; throughput unchanged (5.6 vs 5.5 MB/s A/B).
- **In-band queries were never answered** — the engine's replies to CPR
  (`ESC[6n`), DA1, DECRQM, `CSI 18 t` size reports, and OSC 4/10/11 color
  probes were dropped; programs querying their terminal (vim background
  detection, prompt reflow) hung or misrendered. Replies are now written back
  to the pane's PTY.
- **Synchronized-update freeze** — a program dying mid-`ESC[?2026h` left its
  pane frozen on stale content (vte buffers everything until ESU or a timeout
  gritty never enforced). Expired updates are now force-flushed after vte's
  150 ms deadline.
- **UTF-8 BOM rejection** — a BOM (PowerShell 5.1 `Set-Content`, Notepad) made
  gritty silently discard the whole `session.json` (startup fell back to a
  fresh single tab) and ignore the first `config.toml` key. Both loaders now
  strip it.

### Performance (throughput / memory)
- **Speed-first build** — the binary-size budget was deliberately traded for
  speed. Release profile `opt-level=z → 3`; `build-std` no longer uses
  `optimize_for_size` (std is rebuilt for speed); and `target-cpu=x86-64-v3`
  enables AVX2/FMA/BMI2 so the compiler autovectorizes the software-raster hot
  loops and the VT parser. Binary grows ~800 KB → ~1.1 MB. **CPU floor: Haswell
  (2013+)** — the build will not run on older CPUs.
- **Lower default scrollback** (`5000 → 2000` lines/pane) — scrollback is the
  dominant per-pane RAM consumer (~7.6 MB → ~3 MB at 80 cols) and the memory that
  grows as a pane streams. 2000 keeps generous history at ~40% of the cost; raise
  it with the `scrollback` config key for deeper history.

### Agent awareness
- **Per-pane agent detection** — gritty recognizes ~12 AI coding agents
  (`claude`, `codex`, `cursor`, `copilot`, `gemini`, `opencode`, `droid`,
  `aider`, …) from the pane's foreground process, then classifies each one's
  live state — **working · blocked · idle** — by matching the agent's on-screen
  UI chrome (spinner, "esc to interrupt", a permission/question prompt). The
  pane header shows a color-neutral badge: `●` busy · `◆` needs input · `○` idle.
  Pure detector, fully unit-tested; reuses the existing process poll, so it adds
  no new threads or timers. Ported in spirit from herdr (which doesn't run on
  Windows), reimplemented in gritty's CPU/native model.
- **Done / blocked notifications** — when an agent finishes (`working → idle`) or
  stops for input (`working → blocked`) in a pane you *aren't* watching, gritty
  latches a `★` header badge and flashes the taskbar button (`FLASHW_TIMERNOFG`
  — never steals focus; stops when you look). Focusing the pane clears it. The
  process poll stays alive while a *backgrounded* agent is still working, so the
  flash reaches you even when gritty is minimized or occluded — yet still
  suspends (CA-54) when nothing is working, preserving ~0% idle CPU.
- **Agent overview** (`Ctrl+Shift+A`, command palette, or `F1` help) — a
  centered jump-list of every agent pane across all tabs with its status badge,
  pre-selected on the first pane needing attention. `↑/↓` select, `Enter`/click
  jumps to that pane (switching tab + focus via the same broadcast-disarming path
  as a keyboard tab switch), `Esc`/outside-click closes. Implemented as an
  overlay (like the palette) — it never touches the grid/PTY-geometry/resize
  paths; a `… +N more` footer is shown rather than silently capping the list.

### Rendering & performance
- **Dirty-rect rendering** — fixed a CPU spin (~87 % of a core, which read as a
  freeze/"can't close") under a continuously updating pane (agent spinner,
  streaming log). Each window keeps a persistent backbuffer and a structural
  render signature; a frame is a *full* repaint only on the first frame, a
  resize, or a structural change (chrome, layout, focus, titles, overlays,
  theme, live selection, scrollback view, bell) — otherwise only the
  VT-damaged grid rows are repainted (via alacritty's per-line damage). A
  one-line spinner now repaints ~one line instead of the whole grid.
- `scripts/stress.ps1` — many-pane (default 100) load/leak harness: writes a
  session, launches gritty, samples RSS / threads / handles / CPU over time and
  flags a leak (RSS climbing), a thread leak, or render spin; `-Broadcast`
  streams a spinner into every pane at once.

### Footprint & build
- **Self-contained `gritty.exe` is now under 800 KB** (was ~1.25 MB): release
  profile `opt-level=z` + `codegen-units=1`; hand-rolled `config.toml` and
  `session.json` parsers (drop `toml`/`serde_json` from the runtime); a 32px
  embedded icon; and a `build-std`-rebuilt `std` compiled for size. Pinned
  nightly toolchain (`rust-toolchain.toml`) + `-Z build-std` (`.cargo/config.toml`),
  using `std,panic_abort` so the crash-log panic hook still fires.
- **CI**: GitHub Actions runs the full `gate.ps1` (fmt + clippy `-D` + tests +
  release build + binary/dependency budgets) on Windows for every push/PR.

### Hardening & correctness (2026-06 red-team campaign)
- Closed ~50 audit findings, each with a fail-on-revert regression test —
  e.g. OSC-8 `file://` execution blocked (http/https only); proc-tree-cycle UI
  hang guarded; aggregate session-restore pane budget + runtime tab/pane/window
  caps; atomic session writes; crash-log panic hook; keyboard/active-tab index
  desync on reap; mouse-protocol fidelity (legacy form, motion gating,
  right/middle buttons, Shift-to-bypass); HiDPI; IME; `config.toml` actually
  applied; window title from OSC 0/2; CJK-width tabs.

### Window & input
- **HiDPI / `ScaleFactorChanged` aware** — text scales correctly on 150 %/200 %.
- **IME / dead-key composition** (CJK & accents).
- **Broadcast paste** to every pane at once (`Ctrl+Shift+B`).
- Default font size is now **14 px** (was 18) — tune live with `Ctrl +/-/0` or
  set `font_size` in `config.toml`.
- Maximize→restore-down snaps to a centered, comfortably-sized window instead of
  the near-full-screen pre-maximize size.

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
  `Ctrl+Space` emits NUL (`0x00`) like xterm/readline/Emacs (not a space).
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
- Restored window size is clamped to sane bounds (≤ 16384 per dimension), so a
  crafted `session.json` can't request a degenerate `u32::MAX` window.
- `Pane::new -> Result` with a native error dialog instead of a silent abort.
- Embedded fallback font + no-panic glyph path; graceful surface device-loss.
- Bounded PTY backpressure; coalesced wakes; ~60 fps frame cap; glyph-cache cap.
- Atomic ordering fix (no zombie panes on weak memory).
- Dropped unmaintained `serial` dependency (RUSTSEC-2017-0008) via portable-pty 0.9.
- WCAG-AA UI contrast; gamma-correct text blending.

### Foundations
- CPU rendering (winit + softbuffer), fontdue glyph cache.
- ConPTY via portable-pty; VT engine via alacritty_terminal.
- `main.rs` split into `app` / `input` / `paint` modules.
- 300+ tests; quality gate (fmt + clippy `-D` + tests + size/dep budgets).

### Deferred (tracked)
- UI-Automation screen-reader provider (dedicated a11y effort).
