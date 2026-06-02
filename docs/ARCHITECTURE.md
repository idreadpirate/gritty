# Architecture

gritty is a single-threaded, event-driven native application. The UI runs on the
main thread; each pane's PTY output is drained on a background thread and handed
back through a channel.

## Design principles

1. **Native & lightweight** — Win32 windowing (`winit`), CPU framebuffer
   (`softbuffer`). No GPU pipeline, no async runtime, no browser engine.
2. **Extract, don't reinvent** — ConPTY via `portable-pty`, VT parsing/grid via
   `alacritty_terminal`. We own the multiplexer, renderer, and UX.
3. **Pure logic is testable logic** — geometry, fuzzy matching, process-tree
   walking, and serialization are pure functions with unit tests; the thin GUI
   shell wires them together.

## Module map

| Module | Responsibility |
|---|---|
| `main.rs` | App state, event loop, input routing, render orchestration |
| `font.rs` | Monospace font loading + lazy glyph raster cache (`fontdue`) |
| `render.rs` | Cell/text/rect compositing into the framebuffer, with clipping |
| `background.rs` | Cached glow + dotted-grid base layer (recomputed on resize) |
| `color.rs` | ANSI/256 → RGB mapping and UI colors |
| `layout.rs` | Binary split-tree: rects, split, close/collapse, resize, dividers |
| `session.rs` | `Pane` (shell + grid + name) and `Tab` (tree of panes) |
| `term.rs` | Wraps `alacritty_terminal`: feed bytes, scroll, selection, modes |
| `pty.rs` | ConPTY spawn, threaded reader, write, resize, liveness, pid |
| `key.rs` | winit key events → PTY byte sequences |
| `clipboard.rs` | System clipboard (`arboard`, text-only) |
| `fuzzy.rs` | Subsequence scorer for the command palette |
| `palette.rs` | Command list + fuzzy filtering |
| `persist.rs` | Session snapshot ↔ JSON on disk (`serde`) |
| `proc.rs` | Foreground-process detection via the OS process tree |

## Data flow

```
key/mouse ─▶ main (route) ─▶ pty.write ─▶ shell
shell ─▶ reader thread ─▶ channel ──(EventLoopProxy wake)──▶ main.user_event
       ─▶ term.feed (alacritty grid) ─▶ request_redraw ─▶ render
```

- **Waking:** the PTY reader thread calls an `EventLoopProxy::send_event` waker
  on new output, so the UI sleeps (`ControlFlow::Wait`) until there's work — no
  busy-polling, near-zero idle CPU.
- **Rendering:** every frame copies the cached background, then composites the
  active tab's pane grids (clipped to each pane rect), title bars, focus glow,
  and any overlay (palette/rename). Default-background cells are left
  transparent so the glow shows through.

## Layout model

A tab owns a `HashMap<usize, Pane>` keyed by stable id and a `layout::Node`
binary tree referencing those ids. Splitting replaces a `Leaf` with a `Split`;
closing collapses the parent into its surviving child. Rectangles are computed
from the tree + window size; each pane's grid is sized to its rect.

## Concurrency

One reader thread per pane; everything else is on the UI thread. Panes whose
shell exits flip an `AtomicBool`, and the UI reaps them on the next wake.

## Testing

Run `cargo test`. Coverage focuses on the pure cores: layout geometry
(split/resize/divider/collapse), fuzzy scoring, palette filtering, color blend,
glyph rendering + clipping, paste normalization, process-tree resolution, and
session JSON roundtrip. ConPTY has an integration test that echoes through a real
`cmd.exe`.
