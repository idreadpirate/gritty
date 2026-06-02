<div align="center">

# gritty

**A lightweight, native Windows terminal multiplexer — written in Rust.**

Tabs, split panes, per-pane names, copy/paste that just works, and a command
palette — in a single ~1 MB executable with **no GPU, no runtime, no WSL.**

</div>

---

## Why gritty

Most "modern" terminals are Electron apps that idle at hundreds of megabytes.
gritty is the opposite: a CPU-rendered native Win32 app that starts instantly,
stays light, and never depends on a graphics driver or a browser engine.

It extracts the proven cores of two great projects — [WezTerm's
`portable-pty`](https://github.com/wez/wezterm) for ConPTY and
[Alacritty's `alacritty_terminal`](https://github.com/alacritty/alacritty) VT
engine — and wraps them in a lean, original multiplexer and renderer.

## Features

- **Tabs & split panes** — recursive splits via a binary layout tree; drag a
  border or use the keyboard to resize.
- **Named, process-aware panes** — name any pane; the header also shows the
  foreground process (e.g. `editor: nvim`).
- **Copy / paste that always works** — drag to select (auto-copy),
  `Ctrl+Shift+C/V`, right-click paste, bracketed-paste safe.
- **Command palette** (`Ctrl+Shift+P`) — fuzzy-searchable actions.
- **Broadcast input** — type into every pane in a tab at once.
- **Seamless mode** — hide chrome; just a glow on the focused pane.
- **Session save/restore** — your tab/pane layout survives restarts.
- **Scrollback** — wheel scroll; typing snaps back to the live view.
- **Per-tab neon accents** and a subtle glow background.

## Install / build

Requires the Rust toolchain (MSVC target) on Windows.

```sh
git clone <repo> gritty
cd gritty
cargo build --release
./target/release/gritty.exe
```

The release binary is a single self-contained `gritty.exe` (~1 MB).

## Usage

Run `gritty.exe`. It opens a tab with one pane running PowerShell (falling back
to `cmd`). Split, name, and arrange panes; close a pane with `Ctrl+Shift+W` or
by typing `exit`. Your layout is saved on exit and restored next launch.

See **[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)** for the full reference. The
essentials:

| Action | Keys |
|---|---|
| Command palette | `Ctrl+Shift+P` |
| Split right / down | `Ctrl+Shift+D` / `Ctrl+Shift+E` |
| Move focus / resize pane | `Ctrl+Shift+Arrows` / `Ctrl+Alt+Arrows` |
| Rename pane | `Ctrl+Shift+R` |
| New tab / switch tab | `Ctrl+Shift+T` / `Ctrl+1..9` |
| Copy / paste | `Ctrl+Shift+C` / `Ctrl+Shift+V` |

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — module map and design decisions
- [Comparison](docs/COMPARISON.md) — gritty vs tmux and others (measured, honest)
- [Keybindings](docs/KEYBINDINGS.md) — full reference
- [Contributing](CONTRIBUTING.md) — build, test, and style guide
- [Changelog](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).
