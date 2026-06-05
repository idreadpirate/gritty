# Architecture

gritty is a single-threaded, event-driven native app. The UI runs on the main
thread; each pane's PTY output is drained on a background thread and handed back
through a bounded channel.

## Design principles
1. **Native & lightweight** — Win32 windowing (`winit`), CPU framebuffer
   (`softbuffer`). No GPU pipeline, no async runtime, no browser engine.
2. **Extract, don't reinvent** — ConPTY via `portable-pty`, VT parsing/grid via
   `alacritty_terminal`. gritty owns the multiplexer, renderer, input, and UX.
3. **Pure logic is testable logic** — geometry, fuzzy matching, key/mouse
   encoding, color/flags, paste sanitization, process-tree walking, and
   serialization are pure functions with unit tests (300+ total); the GUI shell
   stays thin and wires them together.

## Module map
| Module | Responsibility |
|---|---|
| `main.rs` | Entry: module decls, constants, OS icon/caption helpers, `main()` |
| `app.rs` | `Gritty` state + lifecycle; tabs/panes; mouse selection & divider drag; drain/reap; session restore/snapshot; process polling; `ApplicationHandler` |
| `input.rs` | Keyboard routing, command-palette input, command dispatch |
| `paint.rs` | `redraw` (dirty-rect: backbuffer + render signature) + `draw_pane_grid`, tab bar, overlays, indicators |
| `render.rs` | Cell/text/rect compositing with clipping; gamma-correct blend |
| `font.rs` | Monospace load + lazy glyph cache; embedded fallback; cache cap |
| `background.rs` | Cached glow + dotted-grid base layer |
| `color.rs` | ANSI/256 → RGB; SGR flag application; theme + UI colors |
| `layout.rs` | Binary split-tree: rects, split, close/collapse, resize, dividers |
| `session.rs` | `Pane` (shell+grid+name) and `Tab` (tree of panes); cwd-on-split |
| `term.rs` | Wraps `alacritty_terminal`: feed, scroll, selection, OSC title/cwd, bell |
| `pty.rs` | ConPTY spawn, threaded reader, backpressure, liveness, pid |
| `key.rs` | winit key events → xterm byte sequences |
| `clipboard.rs` | System clipboard (`arboard`, text-only) |
| `fuzzy.rs` / `palette.rs` | Command-palette scoring + command list |
| `persist.rs` | Session snapshot ↔ JSON (size-capped, geometry, serde) |
| `proc.rs` | Foreground-process detection via the OS process tree |
| `agent.rs` | Pure agent detection: identify the agent from a process name, classify its state (working/blocked/idle) from the screen tail, shared status badge |
| `overview.rs` | Agent-overview overlay state + panel geometry/hit-testing (pure) |
| `config.rs` | Optional `config.toml` |

## Data flow
```
key/mouse ─▶ input/app (route) ─▶ pty.write ─▶ shell
shell ─▶ reader thread ─▶ bounded channel ──(EventLoopProxy wake)──▶ app.user_event
       ─▶ term.feed (alacritty grid) ─▶ schedule_redraw ─▶ paint
```
- **Wakes are coalesced**: the reader pings the UI only on idle→pending, re-armed
  on drain — a flooding pane can't wake-storm the loop. `ControlFlow::Wait` +
  ~60 fps cap ⇒ ~0% idle CPU, bounded busy CPU.
- **Repaints are damage-driven (dirty-rect)**: each window retains a backbuffer
  and a structural render signature (chrome/layout/focus/overlays/theme). A frame
  is a full repaint only on a structural change; otherwise it repaints only the
  VT-damaged grid rows (alacritty per-line damage), so a streaming pane costs
  ~one row, not the whole window.
- **Share-nothing concurrency**: the UI thread owns all terminal state; threads
  only push bytes through channels ⇒ no data races by construction.
- **Agent awareness rides the existing process poll** (no new thread/timer): each
  cycle, a pane running a recognized agent has its screen tail classified
  (`agent::detect_state`); a `working → idle/blocked` change in an unwatched pane
  latches attention and flashes the taskbar. The poll normally suspends when no
  window is visible (CA-54), but stays alive while a backgrounded agent is still
  `Working` so its completion still notifies — then idles back down.

## Layout model
A tab owns `HashMap<id, Pane>` plus a `layout::Node` binary tree referencing those
ids. Splitting replaces a `Leaf` with a `Split`; closing collapses the parent
into its surviving child. Rects derive from the tree + window size; each pane's
grid is sized to its rect.

## Testing
`cargo test` (300+). The quality gate (`scripts/gate.ps1`) enforces: rustfmt,
clippy `-D warnings`, all tests, release build, and **binary-size + dependency
budgets** — every change must keep it green.
