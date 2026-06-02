# gritty vs. other multiplexers

An honest, evidence-backed comparison. Measured on Windows 11; gritty figures are
from the release build in this repo.

## Measured footprint

| | Binary | Idle RAM | GPU driver | Runtime |
|---|---|---|---|---|
| **gritty** | **~1.1 MB** | **~22 MB** | none (CPU) | none |
| tmux | ~1 MB (C) | ~5–10 MB | n/a (TUI) | **needs a host terminal + POSIX layer on Windows** |
| wmux (ConPTY + Electron) | ~100–200 MB | ~150–300 MB | Chromium | Node + Chromium |
| Warp / Electron-class | 100 MB+ | hundreds of MB | GPU | Chromium |

gritty is roughly **100× smaller and ~10× lighter in RAM than Electron-based
native multiplexers** like wmux, while being a *self-contained GUI* (no host
terminal, no browser engine, no GPU pipeline).

## Where gritty decisively wins

1. **Native on Windows — tmux isn't.** tmux is Unix-native and requires WSL2,
   Cygwin, or MSYS2 on Windows because it depends on POSIX features like passing
   file descriptors over UNIX sockets. gritty speaks **ConPTY** directly: one
   `.exe`, no Linux layer, runs where WSL is banned (locked-down corporate PCs).
2. **It's the terminal *and* the multiplexer.** tmux runs *inside* a separate
   terminal emulator; you configure two programs. gritty is one integrated app.
3. **True color, zero config.** tmux's 24-bit color is a notorious
   `TERM`/`terminal-overrides` footgun; gritty renders full RGB out of the box.
4. **System clipboard that just works.** Native `Ctrl+Shift+C/V`, drag-to-copy,
   right-click paste — no `set -g @plugin` / OSC52 / `xclip` plumbing.
5. **GUI-native UX:** mouse drag-to-resize, click-to-switch tabs, per-tab colors,
   process-aware pane headers, a fuzzy command palette, and seamless mode.
6. **Engineered for many panes:** coalesced wakes, bounded backpressure, a frame
   cap, and visible-only repaints keep it responsive and near-0% idle CPU.

## Where tmux still wins (the honest part)

- **Remote & headless.** tmux shines over SSH and on servers; you can detach and
  reattach a *persistent server-side session* across disconnects. gritty is a
  local GUI — it restores layout, but does not host detachable remote sessions.
- **Scriptability & ecosystem.** Decades of plugins, `.tmux.conf`, automation.
- **Ubiquity.** It's everywhere, battle-tested over 15+ years.

## So is it "100× better"?

For a **local, native Windows GUI workflow**, the honest answer is: in the ways
that matter day-to-day there — installs as one tiny binary, runs without WSL,
true color and clipboard with zero config, GUI ergonomics, and provably bounded
resource use — gritty is in a different category, not a marginal improvement.

For **remote/server multiplexing**, tmux remains the right tool. We don't claim
to replace that, and saying otherwise would be dishonest. gritty's mission is to
be the best *local* multiplexer on Windows — and there, it earns the comparison.

## Sources

- [tmux on Windows requires WSL/Cygwin/MSYS2 — tmux.app](https://tmux.app/install/windows/)
- [Native ConPTY multiplexers: psmux](https://zenn.dev/sora_biz/articles/psmux-windows-native-tmux?locale=en),
  [wmux (ConPTY + Electron)](https://github.com/openwong2kim/wmux)
- [tmux true-color is config-dependent — issue #622](https://github.com/tmux/tmux/issues/622),
  [#4300 COLORTERM](https://github.com/tmux/tmux/issues/4300)
- [tmux native-Windows effort — PR #4086](https://github.com/tmux/tmux/pull/4086)
