# gritty vs. the field

An honest, evidence-backed comparison. gritty's numbers are **measured** from the
release build in this repo; competitor facts are **cited** — and where the data
couldn't be independently verified, we say so rather than invent it.

## gritty's measured footprint

| | Binary | RAM | Rendering | Runtime |
|---|---|---|---|---|
| **gritty** | **~1.1 MB** — one self-contained `.exe` | **~10 MB per pane** (2000-line scrollback grid each; tunable via `config.toml`) — tens of MB for a typical session | CPU / software (no GPU) | none |

A single ~1.1 MB exe: no GPU pipeline, no Electron/Chromium, no language
runtime, no WSL. RAM scales with panes because each pane keeps its own scrollback
grid — lower `scrollback` in `config.toml` to trade history for memory.

> **Why no head-to-head RAM/size table for other terminals?** A 2026 research
> pass found that competitors' RAM and binary-size figures are widely *reported*
> but rarely *reproducible* — so quoting them as fact would be dishonest. The
> comparisons below are limited to **independently verifiable, cited** facts.

## vs. the native-Windows terminal field

| Terminal | Native Windows (no WSL) | Built-in multiplexer | Render | Strongest edge over gritty |
|---|:--:|:--:|:--:|---|
| **gritty** | ✅ | ✅ tabs · recursive splits · tab tear-off · session restore | CPU | — |
| Windows Terminal | ✅ | ✅ (richest pane ops) | GPU | swap / move-pane-between-tabs, per-pane profiles, MS-backed ecosystem |
| WezTerm | ✅ | ✅ (+ detach/attach, no WSL) | GPU | cross-platform, Lua scripting, **remote/SSH multiplexing** |
| ConEmu | ✅ | ✅ (recursive grids) | CPU | mature & configurable (but legacy injection, not a single tiny exe) |
| Alacritty | ✅ | ❌ (needs tmux/zellij) | GPU | raw throughput — but *no* built-in multiplexing |
| Kitty | ❌ (macOS/Linux) | ✅ | GPU | — (not on Windows) |
| Ghostty (official) | ❌ (WSL only, needs a GPU) | ✅ | GPU | — (not native on Windows) |

**Verified facts behind the table:**

- **The two most-hyped GPU terminals don't run natively on Windows.** Ghostty's
  maintainer: *"I'm not yet committed on Windows working for Ghostty 1.0"*; it
  requires a real GPU and runs only via WSL. Kitty is macOS/Linux only. Neither
  can contest gritty's core pitch.
- **GPU rendering gives little-to-no input-latency advantage on Windows — and CPU
  terminals win.** A 240 Hz high-speed-camera benchmark measured CPU-rendered
  conhost at **45.8 ms** and MinTTY at **52.4 ms**, *beating* WezTerm and Windows
  Terminal (75 ms) and Alacritty (87.5 ms, slowest). So gritty's CPU renderer is
  not a latency liability on Windows. *(Windows-specific; on Linux, GPU wins.)*
- **Ghostty needs a GPU and degrades badly on software rendering** (~200% CPU in a
  VM), whereas gritty's CPU renderer runs fine in **VMs / RDP / locked-down boxes**.
- **Alacritty has no tabs or splits by design** — it delegates multiplexing to
  tmux/zellij, so it can't match gritty's multiplexer axis on its own.

## Where rivals genuinely win (the honest part)

- **Windows Terminal** — broader in-window pane manipulation (swap panes, move a
  pane between tabs, per-pane shell profiles) and a Microsoft-backed ecosystem.
- **WezTerm** — cross-platform, a Lua scripting/config API, and **remote/SSH
  multiplexing** with detach/attach (works natively on Windows, no WSL).
- **tmux** — the king of remote/headless server sessions over SSH (detach and
  reattach a persistent server-side session across disconnects).
- **GPU terminals** — higher raw throughput on extreme scroll/output.

## So where does gritty win?

Not on any single axis in isolation — on the **intersection**: a sub-800 KB
single native-Windows `.exe`, **CPU-rendered** (so it works where GPU terminals
degrade or aren't available), that is *also* a full **multiplexer** (tabs,
recursive splits, tab tear-off, session restore). No other terminal matches all
of those at once. gritty's mission is to be the best **local, native-Windows**
multiplexer — and there it's in a category of its own.

For **remote/server** multiplexing, use tmux or WezTerm. gritty doesn't host
detachable remote sessions, and we don't pretend it does.

## A newer axis: agent awareness

gritty also detects which AI coding agent runs in each pane and reads its live
state (working/blocked/idle), badging the header, flashing the taskbar when an
unwatched agent finishes or blocks, and offering a jump-list across all panes.
The idea comes from **herdr**, a Unix agent-multiplexer — which doesn't run on
Windows (it's a client-server design over Unix sockets). gritty reimplements the
useful core in its CPU-rendered, single-`.exe` native-Windows model: screen-read
detection only, no integration hooks, no server, no extra dependencies. Among
*native-Windows* terminals, this agent-state awareness is, as far as we can tell,
unique to gritty.

## Sources

- GPU-vs-CPU input latency on Windows (240 Hz camera): https://chadaustin.me/2024/02/windows-terminal-latency/
- Ghostty: no native Windows support, needs a GPU (maintainer discussion): https://github.com/ghostty-org/ghostty/discussions/2563
- Windows Terminal pane model (splits / swap / move-pane): https://learn.microsoft.com/en-us/windows/terminal/panes
- WezTerm multiplexing (unix domains supported on Windows): https://wezterm.org/multiplexing.html
- Native-Windows ConPTY multiplexer (psmux): https://github.com/psmux/psmux
- ConEmu recursive split panes: https://conemu.github.io/en/SplitScreen.html
- tmux on Windows requires WSL/Cygwin/MSYS2: https://tmux.app/install/windows/
