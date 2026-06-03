<div align="center">

<img src="grittyicon.png" alt="gritty" width="140" />

# gritty

**A lightweight, native Windows terminal multiplexer — in Rust.**

Tabs, split panes, a command palette, copy/paste that *actually works*, session
restore, process-aware panes, HiDPI, IME — in a single **sub-800 KB** executable
with **no GPU, no Electron, no runtime, no WSL.**

`290 tests` · `259 deps` · CPU-rendered · one `gritty.exe` **under 800 KB**

</div>

---

## Why it's bad ass

Most "modern" terminals are Chromium in a trench coat — hundreds of MB of RAM to
draw a grid of text. The classic multiplexer, tmux, can't even run natively on
Windows (it needs WSL/Cygwin). gritty refuses both compromises:

- **It's the terminal *and* the multiplexer, natively.** Speaks Windows **ConPTY**
  directly — one `.exe`, runs where WSL is banned (locked-down corporate boxes).
- **Brutally lightweight, by construction.** CPU software rendering (no GPU
  pipeline, no driver surface); a **sub-800 KB** binary (`opt-level=z` + `lto` +
  `strip` + `panic=abort`, hand-rolled config/session parsers instead of
  `toml`/`serde_json`, and a `build-std` `std` rebuilt for size); ~22 MB idle
  RAM; near-0% CPU when idle (event-driven repaint with a frame cap + wake
  coalescing). 20 busy panes can't peg a core.
- **Stands on giants, reinvents nothing risky.** It extracts the proven cores —
  WezTerm's `portable-pty` for ConPTY and Alacritty's `alacritty_terminal` VT
  engine — and wraps them in its own lean multiplexer, renderer, and UX.
- **Hardened like it's going to production.** Successive red-team + code-audit
  campaigns drove out paste-injection, PATH-hijack, session-restore DoS, OSC-8
  `file://` execution, a proc-tree-cycle UI hang, an unmaintained dependency
  (RUSTSEC-2017-0008), a memory-unsafety race, and silent panics (now a crash
  log) — ~50 findings, every fix shipped with a fail-on-revert test behind a
  quality gate (fmt + clippy `-D` + tests + binary/dependency budgets).
- **True color, zero config.** None of tmux's `TERM`/`terminal-overrides` dance.

## Features

**Multiplexing** — tabs; recursive split panes (binary layout tree); per-pane
names; per-tab neon accent colors; **multi-window tab tear-off** (drag a tab onto
another monitor — or `Ctrl+Shift+N` — and it becomes its own window, live panes
and all); **seamless mode** (hide all chrome, just a glow on the focused pane).

**Input & navigation** — full xterm key encoding (F-keys, modified arrows,
Alt-as-ESC, Ctrl-masking); mouse reporting to TUI apps (vim/htop/fzf get clicks &
wheel, Shift to bypass for local selection); double-click word / triple-click
line selection; **command palette** (`Ctrl+Shift+P`, fuzzy); **keybinding help
overlay** (`F1`); font zoom; **IME / dead-key composition** (CJK & accents);
**HiDPI-aware** (text scales correctly on 150 %/200 % displays).

**Copy/paste that always works** — drag to auto-copy, `Ctrl+Shift+C/V`,
right-click paste, **sanitized & bracketed-paste-safe** (strips control/escape
injection); **broadcast one paste to every pane** (`Ctrl+Shift+B` — fan a command
out to a whole fleet at once); **Ctrl-click OSC-8 hyperlinks** (http/https only).

**Pane intelligence** — **process-aware headers** (`editor: nvim`); **splits
inherit the focused pane's cwd** (OSC 7); window title capture (OSC 0/2);
scrollback with a position indicator; visual bell.

**Persistence & looks** — **session save/restore** (layout, names, colors, window
geometry survive restarts); optional `config.toml`; the "gunmetal & amber"
industrial theme; gamma-correct text; WCAG-AA UI contrast; embedded-fallback font
so it never fails to start.

**Resilience** — bounded PTY backpressure, graceful device-loss handling, a real
error dialog instead of a silent crash if no shell can spawn.

## Resize a pane three ways
Drag the border · `Ctrl+Alt+Arrows` · `Ctrl+Mouse-wheel`. The window's maximize
button fills the screen; restore-down snaps back to a centered, comfortably-sized
window (not the near-full-screen pre-maximize size).

## Install

**One line, no toolchain** (Windows 10/11, PowerShell):

```powershell
irm https://raw.githubusercontent.com/idreadpirate/gritty/master/scripts/install.ps1 | iex
```

This downloads the latest released `gritty.exe`, installs it under
`%LOCALAPPDATA%\Programs\gritty`, and adds Start Menu + Desktop shortcuts and a
PATH entry — no admin rights. Launch it from the Start Menu, the Desktop, or by
running `gritty` in any terminal. **Closing that terminal won't close gritty** —
it detaches from the launching shell on startup, so your panes outlive the
window you started them from. Uninstall any time:

```powershell
irm https://raw.githubusercontent.com/idreadpirate/gritty/master/scripts/uninstall.ps1 | iex
```

**Build from source** — `rustup` only; the pinned toolchain is automatic:

```sh
git clone https://github.com/idreadpirate/gritty
cd gritty
cargo build --release   # rust-toolchain.toml auto-selects nightly + rust-src;
                        # .cargo/config.toml rebuilds std at opt=z (-Z build-std)
./target/x86_64-pc-windows-msvc/release/gritty.exe
```

gritty pins a **nightly** toolchain (`rust-toolchain.toml`) and uses `-Z build-std`
(`.cargo/config.toml`) to rebuild `std` for size — that, with `opt-level=z` and
the hand-rolled parsers, keeps the self-contained `gritty.exe` **under 800 KB**.
`rustup` installs the pinned toolchain, `rust-src`, and the MSVC target
automatically on first build — no manual setup. Maintainers cut a release with
`./scripts/release.ps1` (gates, builds, and publishes the exe + checksum).

## Keybindings (essentials)

| Action | Keys |
|---|---|
| Command palette / help | `Ctrl+Shift+P` / `F1` |
| Split right / down | `Ctrl+Shift+D` / `Ctrl+Shift+E` |
| Move focus / resize pane | `Ctrl+Shift+Arrows` / `Ctrl+Alt+Arrows` |
| Rename pane / close pane | `Ctrl+Shift+R` / `Ctrl+Shift+W` |
| New tab / switch tab | `Ctrl+Shift+T` / `Ctrl+1…9` |
| Tear tab into new window | `Ctrl+Shift+N` / drag tab off the bar |
| Copy / paste | `Ctrl+Shift+C` / `Ctrl+Shift+V` |
| Font zoom | `Ctrl +` / `Ctrl -` / `Ctrl 0` |

Full reference: **[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)**.

## Honest scope

gritty is the best **local** terminal on Windows. It is **not** a replacement for
tmux on a server: it doesn't host detachable remote sessions over SSH. For
remote/headless multiplexing, use tmux. See **[docs/COMPARISON.md](docs/COMPARISON.md)**
for the measured, side-by-side honesty (including where tmux still wins).

Two known deferrals, tracked with rationale: per-cell damage-tracking (a perf
optimization — idle CPU is already bounded) and a UI-Automation screen-reader
provider (a large dedicated a11y effort).

## Documentation
- [Architecture](docs/ARCHITECTURE.md) · [Comparison](docs/COMPARISON.md) ·
  [Keybindings](docs/KEYBINDINGS.md) · [Contributing](CONTRIBUTING.md) ·
  [Changelog](CHANGELOG.md)

## License
MIT — see [LICENSE](LICENSE).
