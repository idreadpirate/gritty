use std::num::NonZeroU32;

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use crate::app::Gritty;
use crate::color::{self, ACCENT, BG, FG, SELECTION_BG, UI_BAR_BG, UI_DIM, UI_TITLE_BG};
use crate::font::FontAtlas;
use crate::layout::Rect;
use crate::render::{self, draw_cell, draw_text, fill_rect, stroke_rect, Cell};
use crate::session;

impl Gritty {
    pub(crate) fn redraw(&mut self) {
        let (w, h) = self.win_size();
        let stride = w;
        let height = h;

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        // w/h are clamped to >=1 by win_size(), but construct defensively so a
        // future refactor can't turn this into a panic (CA-14).
        let nz_w = NonZeroU32::new(w as u32).unwrap_or(NonZeroU32::MIN);
        let nz_h = NonZeroU32::new(h as u32).unwrap_or(NonZeroU32::MIN);
        if surface.resize(nz_w, nz_h).is_err() {
            return; // skip this frame instead of crashing
        }
        let mut buffer = surface.buffer_mut().expect("buffer");

        self.background.resize(stride, height);
        buffer.copy_from_slice(&self.background.px);

        let (cw, ch) = (self.font.cell_w, self.font.cell_h);
        let active = self.active;
        let seamless = self.seamless;
        let th = if seamless { 0 } else { ch };

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
        for (i, tab) in self.tabs.iter().enumerate() {
            let label = format!(" {} ", tab.name);
            let tw = label.chars().count() * cw;
            let (fg, bg) = if i == active {
                (BG, tab.color)
            } else {
                (tab.color, UI_BAR_BG)
            };
            let r = Rect {
                x: tx,
                y: 0,
                w: tw,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, bg);
            draw_text(
                &mut buffer,
                stride,
                &mut self.font,
                tx,
                0,
                &label,
                fg,
                bg,
                true,
                r,
            );
            tx += tw + cw / 2;
        }

        let accent = self.tabs.get(active).map(|t| t.color).unwrap_or(ACCENT);

        // Broadcast indicator at the right of the tab bar.
        if self.broadcast {
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
                &mut self.font,
                r.x,
                0,
                label,
                BG,
                accent,
                true,
                r,
            );
        }

        // Panes.
        let area = Rect {
            x: 0,
            y: ch,
            w: stride,
            h: height.saturating_sub(ch),
        };
        let mut rects = Vec::new();
        let focus = self.tabs.get(active).map(|t| t.focus).unwrap_or(0);
        if let Some(tab) = self.tabs.get(active) {
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
                    (BG, accent)
                } else {
                    (UI_DIM, UI_TITLE_BG)
                };
                fill_rect(&mut buffer, stride, title_rect, tbg);
                let header = self
                    .tabs
                    .get(active)
                    .and_then(|t| t.panes.get(&id))
                    .map(|p| {
                        let proc = p.proc_name.as_str();
                        if proc.is_empty()
                            || proc == "pwsh"
                            || proc == "cmd"
                            || proc == "powershell"
                        {
                            p.name.clone()
                        } else {
                            format!("{}: {}", p.name, proc)
                        }
                    })
                    .unwrap_or_default();
                draw_text(
                    &mut buffer,
                    stride,
                    &mut self.font,
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
            if let Some(pane) = self.tabs.get(active).and_then(|t| t.panes.get(&id)) {
                draw_pane_grid(
                    &mut buffer,
                    stride,
                    &mut self.font,
                    pane,
                    grid,
                    is_focus,
                    accent,
                );
            }

            // Focused pane always gets the accent glow border.
            if is_focus {
                stroke_rect(&mut buffer, stride, rect, accent);
            }
        }

        // Command palette overlay.
        if let Some(p) = self.palette.as_ref() {
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
                &mut self.font,
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
                    (BG, accent)
                } else {
                    (FG, panel)
                };
                draw_text(
                    &mut buffer,
                    stride,
                    &mut self.font,
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

        // Rename overlay.
        if let Some(buf_str) = self.rename.clone() {
            let line = format!(" rename pane: {buf_str}_ ");
            let r = Rect {
                x: 0,
                y: height.saturating_sub(ch),
                w: stride,
                h: ch,
            };
            fill_rect(&mut buffer, stride, r, ACCENT);
            draw_text(
                &mut buffer,
                stride,
                &mut self.font,
                0,
                r.y,
                &line,
                BG,
                ACCENT,
                true,
                r,
            );
        }

        buffer.present().expect("present");
        self.last_render = std::time::Instant::now();
        self.redraw_pending = false;
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
    accent: u32,
) {
    let (cw, ch) = (font.cell_w, font.cell_h);
    let content = pane.term.term.renderable_content();
    let selection = content.selection;
    let at_bottom = content.display_offset == 0;
    let cursor_visible = is_focus && at_bottom && content.cursor.shape != CursorShape::Hidden;
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

        let base_fg = color::to_rgb(cell.fg, FG);
        let base_bg = color::to_rgb(cell.bg, BG);
        let is_default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));

        // SGR flags (inverse/bold/dim/hidden/underline) — CA-4.
        let (mut fg, mut bg, underline) = color::style_flags(base_fg, base_bg, cell.flags);
        let inverted = cell.flags.contains(Flags::INVERSE);
        let mut fill_bg = !is_default_bg || inverted;

        if selection.is_some_and(|r| r.contains(item.point)) {
            bg = SELECTION_BG;
            fill_bg = true;
        } else if cursor_visible && line == cur_row && col as i32 == cur_col {
            bg = accent;
            fg = BG;
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
    }
}
