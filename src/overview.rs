// Agent overview overlay: a selectable list of every agent pane in the window,
// so you can jump straight to the one that just finished or needs you — the
// payoff of the per-pane agent detection in `agent.rs`.
//
// The item list is rebuilt from the live panes on every frame/keystroke (see
// `Gritty::agent_items`), so this struct holds only the current selection —
// nothing here can go stale against the real panes.

use crate::agent::AgentState;

/// One row in the overview: enough to render it and to jump to its pane.
pub struct Item {
    /// Index of the tab owning the pane (to switch the active tab on jump).
    pub tab: usize,
    /// Pane id within that tab (to set the tab's focus on jump).
    pub pane: usize,
    /// Pre-rendered `tab / pane: agent` label.
    pub label: String,
    pub state: AgentState,
    pub attention: bool,
}

/// Overlay state — just the highlighted row. Built by `Gritty::toggle_agents`,
/// which pre-selects the first pane wanting attention.
pub struct Overview {
    pub sel: usize,
}

/// Most rows the overview draws (and the keyboard selection is clamped to). A
/// dozen agent panes is already a very busy fleet; capping keeps the panel from
/// overflowing the window and the selection from reaching a never-drawn row.
pub const VISIBLE_ROWS: usize = 12;

/// Clamp a selection to the visible window of `len` items. Empty ⇒ 0.
pub fn clamp_sel(sel: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    sel.min(len.min(VISIBLE_ROWS) - 1)
}

/// Panel rectangle, shared by the renderer and the click hit-test so they agree
/// on geometry. Returns `(bx, by, box_w, box_h, shown_rows)`. Mirrors the
/// command palette's centered box. `count` is the number of agent items.
pub fn geom(
    stride: usize,
    cw: usize,
    ch: usize,
    count: usize,
) -> (usize, usize, usize, usize, usize) {
    let shown = count.min(VISIBLE_ROWS);
    let box_w = (stride * 2 / 3)
        .max(40 * cw.max(1))
        .min(stride.saturating_sub(cw));
    let box_h = (shown + 2) * ch; // title row + items + a row of padding
    let bx = stride.saturating_sub(box_w) / 2;
    let by = ch * 2;
    (bx, by, box_w, box_h, shown)
}

/// Y of the first item row inside the panel (the title sits on `by`).
pub fn first_row_y(by: usize, ch: usize) -> usize {
    by + ch + ch / 2
}

/// Where a click landed on the overview panel.
#[derive(Debug, PartialEq, Eq)]
pub enum Hit {
    /// An item row (0-based, within the shown window).
    Row(usize),
    /// Inside the panel but not on an item (title/padding) — keep it open.
    Chrome,
    /// Outside the panel — dismiss it.
    Outside,
}

#[allow(clippy::too_many_arguments)]
pub fn hit(
    px: usize,
    py: usize,
    bx: usize,
    by: usize,
    box_w: usize,
    box_h: usize,
    ch: usize,
    shown: usize,
) -> Hit {
    if px < bx || px >= bx + box_w || py < by || py >= by + box_h {
        return Hit::Outside;
    }
    let first = first_row_y(by, ch);
    if py < first {
        return Hit::Chrome; // title row
    }
    let i = (py - first) / ch.max(1);
    if i < shown {
        Hit::Row(i)
    } else {
        Hit::Chrome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_pins_within_visible_window() {
        assert_eq!(clamp_sel(0, 0), 0); // empty list
        assert_eq!(clamp_sel(5, 3), 2); // past the end → last item
        assert_eq!(clamp_sel(1, 3), 1); // valid index untouched
        assert_eq!(clamp_sel(99, 50), VISIBLE_ROWS - 1); // past the drawn window
    }

    #[test]
    fn hit_maps_clicks_to_rows() {
        // 10px cells, 5 items. Compute the panel and probe each region.
        let (bx, by, bw, bh, shown) = geom(800, 10, 10, 5);
        assert_eq!(shown, 5);
        let first = first_row_y(by, 10);
        // First item row.
        assert_eq!(
            hit(bx + 5, first + 1, bx, by, bw, bh, 10, shown),
            Hit::Row(0)
        );
        // Third item row.
        assert_eq!(
            hit(bx + 5, first + 2 * 10 + 1, bx, by, bw, bh, 10, shown),
            Hit::Row(2)
        );
        // Title row is chrome.
        assert_eq!(hit(bx + 5, by + 1, bx, by, bw, bh, 10, shown), Hit::Chrome);
        // Left of the panel is outside.
        assert_eq!(
            hit(bx.saturating_sub(1), first, bx, by, bw, bh, 10, shown),
            Hit::Outside
        );
    }
}
