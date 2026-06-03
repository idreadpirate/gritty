use std::num::NonZeroU32;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use crate::app::Gritty;
use crate::color::{self, PANE_SEP, SELECTION_BG, UI_BAR_BG, UI_DIM, UI_TITLE_BG};
use crate::font::FontAtlas;
use crate::layout::Rect;
use crate::render::{self, draw_cell, draw_text, fill_rect, stroke_rect, Cell};
use crate::session;

impl Gritty {
    pub(crate) fn redraw(&mut self, wi: usize) {
        let (w, h) = self.win_size(wi);
        let stride = w;
        let height = h;

        if wi >= self.windows.len() {
            return;
        }
        // Split borrows: the glyph atlas is shared across windows, the
        // surface/state belongs to this window. They are distinct fields of
        // `self`, so borrowing both at once is sound.
        let font = &mut self.font;
        let win = &mut self.windows[wi];

        // Account this frame up-front. The surface/buffer/present steps below may
        // bail out early ("skip this frame" on a minimized/occluded window or a
        // transient device-context loss). If we left `redraw_pending` set with a
        // stale `last_render`, `about_to_wait` would re-request a redraw every
        // tick — pegging a core at 100% (reads as a freeze). Clearing here means a
        // skipped frame simply waits for the next real trigger (PTY output,
        // resize, focus/occlude) to repaint.
        win.last_render = std::time::Instant::now();
        win.redraw_pending = false;

        // CA-54: a window the OS reports as occluded/minimized is not on screen,
        // so skip the whole paint (full-buffer blit + per-cell render). Bookkeeping
        // was cleared above, so this won't busy-spin `about_to_wait`; the next
        // `Occluded(false)` re-shows and requests a fresh frame.
        if !win.visible {
            return;
        }

        // w/h are clamped to >=1 by win_size(), but construct defensively so a
        // future refactor can't turn this into a panic (CA-14).
        let nz_w = NonZeroU32::new(w as u32).unwrap_or(NonZeroU32::MIN);
        let nz_h = NonZeroU32::new(h as u32).unwrap_or(NonZeroU32::MIN);
        if win.surface.resize(nz_w, nz_h).is_err() {
            return; // skip this frame instead of crashing
        }
        let Ok(mut buffer) = win.surface.buffer_mut() else {
            return; // transient device-context loss — skip this frame (CA-1)
        };

        win.background.resize(stride, height);
        buffer.copy_from_slice(&win.background.px);

        let (cw, ch) = (font.cell_w, font.cell_h);
        let active = win.active;
        let seamless = win.seamless;
        // CA-47: when the OS window is unfocused, the cursor is drawn hollow even
        // in the focused pane (convention), independent of intra-window focus.
        let os_focused = win.os_focused;
        let th = if seamless { 0 } else { ch };

        // CA-46: the active tab is being viewed now, so clear its background-
        // activity marker. (BELs in the active tab's visible panes flash in real
        // time below; only background tabs accumulate the marker, set in
        // `drain_pty`.)
        if let Some(tab) = win.tabs.get_mut(active) {
            tab.activity = false;
        }

        // Tab bar.
        fill_rect(
            &mut buffer,
            stride,
            Rect {
                x: 0,
                y: 0,
                w: stride,
                h: ch,
            },
            UI_BAR_BG,
        );
        let mut tx = 0usize;
        for (i, tab) in win.tabs.iter().enumerate() {
            // CA-25: prefix active tab with a glyph marker (not color alone).
            // CA-46: a background tab with output/bell activity since last viewed
            // gets a `•` marker in the same leading-pad cell, so it doesn't shift
            // the tab geometry the hit-tests depend on.
            let label = if i == active {
                format!(" ▸{} ", tab.name)
            } else if tab.activity {
                format!("•{} ", tab.name)
            } else {
                format!(" {} ", tab.name)
            };
            // CA-28: slot = label text + one cell for '×'.
            // CA-45: size the slot by the name's display width (CJK = 2 cells),
            // the same measure the hit-tests use, so render and click agree.
            let text_w = (crate::layout::name_cols(&tab.name) + 2) * cw;
            let slot_w = text_w + cw;
            if tx + slot_w > stride {
                break; // overflow: stop drawing tabs past the window edge
            }
            let (fg, bg) = if i == active {
                (color::bg(), tab.color)
            } else {
                (tab.color, UI_BAR_BG)
            };
            let r = Rect {
                x: tx,
                y: 0,
                w: slot_w,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, bg);
            let label_rect = Rect {
                x: tx,
                y: 0,
                w: text_w,
                h: ch,
            };
            draw_text(
                &mut buffer,
                stride,
                font,
                tx,
                0,
                &label,
                fg,
                bg,
                true,
                label_rect,
            );
            // CA-28: draw the '×' close button cell.
            let x_rect = Rect {
                x: tx + text_w,
                y: 0,
                w: cw,
                h: ch,
            };
            draw_text(
                &mut buffer,
                stride,
                font,
                tx + text_w,
                0,
                "×",
                UI_DIM,
                bg,
                true,
                x_rect,
            );
            tx += slot_w + cw / 2;
        }
        // CA-28: draw the '+' new-tab button after all tabs.
        if tx + cw <= stride {
            let plus_rect = Rect {
                x: tx,
                y: 0,
                w: cw,
                h: ch,
            };
            fill_rect(&mut buffer, stride, plus_rect, UI_BAR_BG);
            draw_text(
                &mut buffer,
                stride,
                font,
                tx,
                0,
                "+",
                UI_DIM,
                UI_BAR_BG,
                true,
                plus_rect,
            );
        }

        let accent = win
            .tabs
            .get(active)
            .map(|t| t.color)
            .unwrap_or(color::accent());

        // Broadcast indicator at the right of the tab bar.
        if win.broadcast {
            let label = " BROADCAST ";
            let lw = label.chars().count() * cw;
            let r = Rect {
                x: stride.saturating_sub(lw),
                y: 0,
                w: lw,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, accent);
            draw_text(
                &mut buffer,
                stride,
                font,
                r.x,
                0,
                label,
                color::bg(),
                accent,
                true,
                r,
            );
        }

        // Tab strip bottom separator — faint 1px line between tabs and content (CA-29).
        fill_rect(
            &mut buffer,
            stride,
            Rect {
                x: 0,
                y: ch.saturating_sub(1),
                w: stride,
                h: 1,
            },
            PANE_SEP,
        );

        // Panes.
        let area = Rect {
            x: 0,
            y: ch,
            w: stride,
            h: height.saturating_sub(ch),
        };
        let mut rects = Vec::new();
        let focus = win.tabs.get(active).map(|t| t.focus).unwrap_or(0);
        if let Some(tab) = win.tabs.get(active) {
            tab.tree.layout(area, &mut rects);
        }

        for (id, rect) in &rects {
            let id = *id;
            let rect = *rect;
            let is_focus = id == focus;

            // Pane title bar (hidden in seamless mode).
            if !seamless {
                let title_rect = Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: ch,
                };
                let (tfg, tbg) = if is_focus {
                    (color::bg(), accent)
                } else {
                    (UI_DIM, UI_TITLE_BG)
                };
                fill_rect(&mut buffer, stride, title_rect, tbg);
                let header = win
                    .tabs
                    .get(active)
                    .and_then(|t| t.panes.get(&id))
                    .map(|p| {
                        let proc = p.proc_name.as_str();
                        let base = if proc.is_empty()
                            || proc == "pwsh"
                            || proc == "cmd"
                            || proc == "powershell"
                        {
                            p.name.clone()
                        } else {
                            format!("{}: {}", p.name, proc)
                        };
                        // CA-25: add a non-color marker for the focused pane title.
                        if is_focus {
                            format!("▸ {base}")
                        } else {
                            base
                        }
                    })
                    .unwrap_or_default();
                draw_text(
                    &mut buffer,
                    stride,
                    font,
                    rect.x + cw / 2,
                    rect.y,
                    &header,
                    tfg,
                    tbg,
                    true,
                    title_rect,
                );
            }

            // Grid.
            let grid = Rect {
                x: rect.x,
                y: rect.y + th,
                w: rect.w,
                h: rect.h.saturating_sub(th),
            };
            if let Some(pane) = win.tabs.get(active).and_then(|t| t.panes.get(&id)) {
                draw_pane_grid(
                    &mut buffer,
                    stride,
                    font,
                    pane,
                    grid,
                    is_focus,
                    os_focused,
                    accent,
                );
            }

            // Focused pane gets the accent glow border; unfocused panes get a
            // subtle 1px separator so pane boundaries remain visible (CA-24).
            if is_focus {
                stroke_rect(&mut buffer, stride, rect, accent);
            } else {
                stroke_rect(&mut buffer, stride, rect, PANE_SEP);
            }
        }

        // Command palette overlay.
        if let Some(p) = win.palette.as_ref() {
            let (query, sel, matches) = (p.query.clone(), p.sel, p.matches());
            let shown = matches.len().min(8);
            let box_w = (stride * 2 / 3)
                .max(40 * cw.max(1))
                .min(stride.saturating_sub(cw));
            let box_h = (shown + 1) * ch + ch / 2;
            let bx = (stride.saturating_sub(box_w)) / 2;
            let by = ch * 2;
            let panel = 0x0020_2030u32;
            let rbox = Rect {
                x: bx,
                y: by,
                w: box_w,
                h: box_h,
            };
            fill_rect(&mut buffer, stride, rbox, panel);
            stroke_rect(&mut buffer, stride, rbox, accent);

            let qline = format!("> {query}_");
            let qrect = Rect {
                x: bx,
                y: by,
                w: box_w,
                h: ch,
            };
            draw_text(
                &mut buffer,
                stride,
                font,
                bx + cw,
                by + ch / 4,
                &qline,
                accent,
                panel,
                false,
                qrect,
            );

            for (i, (label, _)) in matches.iter().take(shown).enumerate() {
                let iy = by + ch + ch / 2 + i * ch;
                let irow = Rect {
                    x: bx,
                    y: iy,
                    w: box_w,
                    h: ch,
                };
                let (fg, bg) = if i == sel {
                    fill_rect(&mut buffer, stride, irow, accent);
                    (color::bg(), accent)
                } else {
                    (color::fg(), panel)
                };
                draw_text(
                    &mut buffer,
                    stride,
                    font,
                    bx + cw,
                    iy,
                    label,
                    fg,
                    bg,
                    false,
                    irow,
                );
            }
        }

        // RT-8: broadcast pending-signal confirmation prompt.
        if win.broadcast && win.broadcast_pending_signal.is_some() {
            let label = " [BROADCAST] press again to send signal to all panes ";
            let r = Rect {
                x: 0,
                y: height.saturating_sub(ch * 2),
                w: stride,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, accent);
            draw_text(
                &mut buffer,
                stride,
                font,
                0,
                r.y,
                label,
                color::bg(),
                accent,
                true,
                r,
            );
        }

        // CA-21: keybinding help overlay.
        if win.show_help {
            let entries: &[(&str, &str)] = &[
                ("F1 / Ctrl+Shift+/", "Toggle this help overlay"),
                ("Ctrl+Shift+T", "New tab"),
                ("Ctrl+Shift+W", "Close pane"),
                ("Ctrl+Shift+D", "Split pane right"),
                ("Ctrl+Shift+E", "Split pane down"),
                ("Ctrl+Shift+P", "Command palette"),
                ("Ctrl+Shift+R", "Rename pane"),
                ("Ctrl+Shift+C", "Copy selection"),
                ("Ctrl+Shift+V", "Paste"),
                ("Ctrl+Shift+B", "Broadcast-paste clipboard to ALL panes"),
                ("Ctrl+Tab", "Next tab"),
                ("Ctrl+1-9", "Switch to tab N"),
                ("Ctrl+0 / +/-", "Font zoom reset/in/out"),
                ("Ctrl+Alt+Arrows", "Resize pane"),
                ("Ctrl+Shift+Arrows", "Move focus"),
                ("Right-click", "Paste"),
            ];
            let shown = entries.len();
            let col_key_w = 24 * cw; // fixed-width key column
            let col_val_w = 32 * cw;
            let box_w = (col_key_w + col_val_w + cw * 2)
                .max(40 * cw.max(1))
                .min(stride.saturating_sub(cw));
            let box_h = (shown + 2) * ch;
            let bx = (stride.saturating_sub(box_w)) / 2;
            let by = ch * 2;
            let panel = 0x0020_2030u32;
            let rbox = Rect {
                x: bx,
                y: by,
                w: box_w,
                h: box_h,
            };
            fill_rect(&mut buffer, stride, rbox, panel);
            stroke_rect(&mut buffer, stride, rbox, accent);
            // Header row.
            let header_rect = Rect {
                x: bx,
                y: by,
                w: box_w,
                h: ch,
            };
            draw_text(
                &mut buffer,
                stride,
                font,
                bx + cw,
                by,
                "Keybindings  (Esc / F1 to close)",
                accent,
                panel,
                false,
                header_rect,
            );
            for (i, (binding, desc)) in entries.iter().enumerate() {
                let iy = by + ch + i * ch;
                let row_rect = Rect {
                    x: bx,
                    y: iy,
                    w: box_w,
                    h: ch,
                };
                // Key column.
                let key_rect = Rect {
                    x: bx + cw,
                    y: iy,
                    w: col_key_w,
                    h: ch,
                };
                draw_text(
                    &mut buffer,
                    stride,
                    font,
                    bx + cw,
                    iy,
                    binding,
                    accent,
                    panel,
                    false,
                    key_rect,
                );
                // Value column.
                let val_rect = Rect {
                    x: bx + cw + col_key_w,
                    y: iy,
                    w: row_rect.w.saturating_sub(cw + col_key_w),
                    h: ch,
                };
                draw_text(
                    &mut buffer,
                    stride,
                    font,
                    bx + cw + col_key_w,
                    iy,
                    desc,
                    color::fg(),
                    panel,
                    false,
                    val_rect,
                );
            }
        }

        // Rename overlay.
        if let Some(buf_str) = win.rename.clone() {
            let what = if win.rename_is_tab { "tab" } else { "pane" };
            let line = format!(" rename {what}: {buf_str}_ ");
            let r = Rect {
                x: 0,
                y: height.saturating_sub(ch),
                w: stride,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, color::accent());
            draw_text(
                &mut buffer,
                stride,
                font,
                0,
                r.y,
                &line,
                color::bg(),
                color::accent(),
                true,
                r,
            );
        }

        // CA-48: IME composition (preedit) overlay. While the user is composing
        // CJK / dead-key accents, winit delivers the in-progress string before the
        // final Commit. Show it on a status line so the user sees what they're
        // typing; it clears on commit or when composition ends.
        if !win.preedit.is_empty() {
            let line = format!(" compose: {} ", win.preedit);
            let r = Rect {
                x: 0,
                y: height.saturating_sub(ch),
                w: stride,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, accent);
            draw_text(
                &mut buffer,
                stride,
                font,
                0,
                r.y,
                &line,
                color::bg(),
                accent,
                true,
                r,
            );
        }

        // Ignore a transient present failure (device-context loss): skip this
        // frame rather than crash (CA-1). Frame bookkeeping was already cleared
        // at the top, so a skipped frame won't busy-spin `about_to_wait`.
        let _ = buffer.present();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_pane_grid(
    buffer: &mut [u32],
    stride: usize,
    font: &mut FontAtlas,
    pane: &session::Pane,
    grid: Rect,
    is_focus: bool,
    window_focused: bool,
    accent: u32,
) {
    let (cw, ch) = (font.cell_w, font.cell_h);
    let content = pane.term.term.renderable_content();
    let selection = content.selection;
    let at_bottom = content.display_offset == 0;
    let cursor_shape = content.cursor.shape;
    let cursor_hidden = cursor_shape == CursorShape::Hidden;
    // Focused pane in a focused window: block cursor inverts the cell. Otherwise
    // (unfocused pane, OR an unfocused OS window — CA-47) the cursor is drawn as a
    // hollow outline after the grid.
    let cursor_solid = cursor_is_solid(is_focus, window_focused);
    let cursor_active = at_bottom && !cursor_hidden;
    let cur_row = content.cursor.point.line.0;
    let cur_col = content.cursor.point.column.0 as i32;

    for item in content.display_iter {
        let line = item.point.line.0;
        if line < 0 {
            continue;
        }
        let cell = item.cell;
        // The spacer after a wide glyph is painted by the wide cell itself (CA-5).
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let row = line as usize;
        let col = item.point.column.0;

        let base_fg = color::to_rgb(cell.fg, color::fg());
        let base_bg = color::to_rgb(cell.bg, color::bg());
        let is_default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));

        // SGR flags (inverse/bold/dim/hidden/underline) — CA-4.
        let (mut fg, mut bg, underline) = color::style_flags(base_fg, base_bg, cell.flags);
        let inverted = cell.flags.contains(Flags::INVERSE);
        let mut fill_bg = !is_default_bg || inverted;

        if selection.is_some_and(|r| r.contains(item.point)) {
            bg = SELECTION_BG;
            fill_bg = true;
        } else if cursor_solid
            && cursor_active
            && line == cur_row
            && col as i32 == cur_col
            && cursor_shape == CursorShape::Block
        {
            // Focused block cursor: invert the cell (CA-17).
            // Beam and Underline draw overlays after the cell; no bg change here.
            bg = accent;
            fg = color::bg();
            fill_bg = true;
        }

        let px = grid.x + col * cw;
        let py = grid.y + row * ch;
        draw_cell(
            buffer,
            stride,
            font,
            px,
            py,
            Cell { ch: cell.c, fg, bg },
            fill_bg,
            grid,
        );

        if underline {
            let uy = py + ch.saturating_sub(2);
            render::fill_rect(
                buffer,
                stride,
                Rect {
                    x: px,
                    y: uy,
                    w: cw,
                    h: 1,
                },
                fg,
            );
        }

        // CA-33: underline cells that carry an OSC-8 hyperlink (1px, like SGR underline).
        if cell.hyperlink().is_some() {
            let uy = py + ch.saturating_sub(2);
            render::fill_rect(
                buffer,
                stride,
                Rect {
                    x: px,
                    y: uy,
                    w: cw,
                    h: 1,
                },
                fg,
            );
        }
    }

    // CA-27: visual bell — brief amber flash over the pane for this frame only.
    if pane.term.take_bell() {
        // Blend a translucent amber overlay (≈25% opacity) across the pane grid.
        // Alpha-blend: out = src * alpha + dst * (1 - alpha), alpha = 64/255 ≈ 0.25.
        const BELL_COLOR: u32 = 0x00ff_7b00; // molten orange (matches ACCENT)
        const ALPHA: u32 = 64; // ~25% opacity
        let sr = (BELL_COLOR >> 16) & 0xff;
        let sg = (BELL_COLOR >> 8) & 0xff;
        let sb = BELL_COLOR & 0xff;
        let h = buf_height(buffer, stride);
        let x1 = (grid.x + grid.w).min(stride);
        let y1 = (grid.y + grid.h).min(h);
        for y in grid.y..y1 {
            let base = y * stride;
            for x in grid.x..x1 {
                let dst = buffer[base + x];
                let dr = (dst >> 16) & 0xff;
                let dg = (dst >> 8) & 0xff;
                let db = dst & 0xff;
                let r = (sr * ALPHA + dr * (255 - ALPHA)) / 255;
                let g = (sg * ALPHA + dg * (255 - ALPHA)) / 255;
                let b = (sb * ALPHA + db * (255 - ALPHA)) / 255;
                buffer[base + x] = (r << 16) | (g << 8) | b;
            }
        }
    }

    // CA-22: scrollback position indicator — thin thumb on the right edge.
    let display_offset = pane.term.display_offset();
    if display_offset > 0 && grid.h > 0 && grid.w > 0 {
        let history = pane.term.term.grid().history_size();
        let rows = pane.term.size.rows;
        let (thumb_top, thumb_h) = scrollbar_thumb(grid.h, rows, history, display_offset);
        // 2px wide thumb drawn at the pane's right edge, using PANE_SEP as track
        // and a dimmed accent as thumb — visible without clashing.
        let thumb_x = grid.x + grid.w.saturating_sub(2);
        fill_rect(
            buffer,
            stride,
            Rect {
                x: thumb_x,
                y: grid.y,
                w: 2,
                h: grid.h,
            },
            PANE_SEP,
        );
        fill_rect(
            buffer,
            stride,
            Rect {
                x: thumb_x,
                y: grid.y + thumb_top,
                w: 2,
                h: thumb_h.max(2),
            },
            UI_DIM,
        );
    }

    // Post-grid cursor overlays (CA-17).
    if cursor_active && cur_row >= 0 && cur_col >= 0 {
        let cur_px = grid.x + cur_col as usize * cw;
        let cur_py = grid.y + cur_row as usize * ch;
        if cursor_solid {
            match cursor_shape {
                CursorShape::Underline => {
                    // 2px bar at the bottom of the cell.
                    render::fill_rect(
                        buffer,
                        stride,
                        Rect {
                            x: cur_px,
                            y: cur_py + ch.saturating_sub(2),
                            w: cw,
                            h: 2,
                        },
                        accent,
                    );
                }
                CursorShape::Beam => {
                    // 2px vertical bar at the left edge of the cell.
                    render::fill_rect(
                        buffer,
                        stride,
                        Rect {
                            x: cur_px,
                            y: cur_py,
                            w: 2,
                            h: ch,
                        },
                        accent,
                    );
                }
                // Block is handled inline above; Hidden is excluded by cursor_active.
                _ => {}
            }
        } else {
            // Unfocused pane: hollow dim outline at cursor position (CA-17).
            render::stroke_rect(
                buffer,
                stride,
                Rect {
                    x: cur_px,
                    y: cur_py,
                    w: cw,
                    h: ch,
                },
                UI_DIM,
            );
        }
    }
}

/// CA-17/CA-47: whether the text cursor is drawn solid (filled block / accent
/// beam / underline) rather than as a hollow outline. Solid only when this pane
/// is the focused pane AND its OS window currently has keyboard focus; an
/// unfocused pane or an unfocused window both hollow the cursor.
pub(crate) fn cursor_is_solid(pane_focused: bool, window_focused: bool) -> bool {
    pane_focused && window_focused
}

/// Buffer height in pixels (stride must be > 0).
fn buf_height(buf: &[u32], stride: usize) -> usize {
    buf.len().checked_div(stride).unwrap_or(0)
}

/// Compute the scrollbar thumb position within a `track_len`-pixel track.
///
/// Returns `(thumb_top, thumb_height)` in pixels.
///
/// * `track_len`      — height of the scrollable area in pixels
/// * `viewport_lines` — number of visible terminal rows
/// * `history_size`   — total scrollback lines available
/// * `display_offset` — lines currently scrolled above the bottom (0 = live)
///
/// The thumb is sized proportionally to the viewport vs total content and
/// positioned so that offset 0 (bottom) places the thumb at the bottom of the
/// track and the maximum offset places it at the top (CA-22).
pub(crate) fn scrollbar_thumb(
    track_len: usize,
    viewport_lines: usize,
    history_size: usize,
    display_offset: usize,
) -> (usize, usize) {
    // Total content is viewport + history.  Guard against zero total.
    let total = (viewport_lines + history_size).max(1);
    // Thumb height: proportion of content that is visible (at least 4px).
    let thumb_h = ((track_len * viewport_lines) / total).max(4).min(track_len);
    // Scrollable track length (pixels the thumb can travel within).
    let travel = track_len.saturating_sub(thumb_h);
    // display_offset == history_size → top; 0 → bottom.
    let offset_clamped = display_offset.min(history_size);
    // checked_div: history_size==0 -> None -> 0, so thumb_top stays at `travel`
    // (the old explicit zero-guard) and never divides by zero.
    let thumb_top = travel
        - (travel * offset_clamped)
            .checked_div(history_size)
            .unwrap_or(0);
    (thumb_top, thumb_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_at_bottom_when_offset_zero() {
        // offset=0 means we're at the live bottom; thumb should be at bottom of track.
        let (top, h) = scrollbar_thumb(100, 24, 100, 0);
        assert_eq!(top + h, 100, "thumb bottom should touch track end");
    }

    #[test]
    fn thumb_at_top_when_fully_scrolled() {
        // offset == history → scrolled to the very top; thumb top should be 0.
        let (top, _h) = scrollbar_thumb(100, 24, 100, 100);
        assert_eq!(
            top, 0,
            "thumb top should be at track start when fully scrolled"
        );
    }

    #[test]
    fn thumb_height_proportional_to_viewport() {
        // viewport == total → full-height thumb.
        let (_, h) = scrollbar_thumb(100, 50, 0, 0);
        assert_eq!(h, 100, "full viewport = full thumb");
    }

    #[test]
    fn thumb_minimum_height_enforced() {
        // Very long history → tiny ratio, but thumb must be at least 4px.
        let (_, h) = scrollbar_thumb(100, 1, 10_000, 500);
        assert!(h >= 4, "thumb height must be at least 4px, got {h}");
    }

    #[test]
    fn thumb_stays_within_track() {
        for offset in [0, 50, 100] {
            let (top, h) = scrollbar_thumb(100, 24, 100, offset);
            assert!(
                top + h <= 100,
                "thumb must not exceed track: top={top} h={h}"
            );
        }
    }

    // --- CA-47 cursor hollows when the OS window is unfocused -----------------

    #[test]
    fn cursor_solid_only_when_pane_and_window_focused() {
        // The filled block / accent beam only when this is the focused pane AND
        // the OS window has keyboard focus.
        assert!(cursor_is_solid(true, true));
        // CA-47: a focused pane in an UNFOCUSED window draws a hollow cursor.
        assert!(!cursor_is_solid(true, false));
        // An unfocused pane is always hollow (pre-existing CA-17 behaviour).
        assert!(!cursor_is_solid(false, true));
        assert!(!cursor_is_solid(false, false));
    }
}
