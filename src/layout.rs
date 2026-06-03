// A binary split-tree describing how panes tile a tab. Pure geometry — no
// rendering, no terminal state — so it's exhaustively testable.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side (a = left, b = right); divider is vertical.
    LeftRight,
    /// Children stack (a = top, b = bottom); divider is horizontal.
    TopBottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn contains(&self, px: usize, py: usize) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn center(&self) -> (usize, usize) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// Split `area` into the two child rectangles for a given axis and ratio.
pub fn child_areas(axis: Axis, ratio: f32, area: Rect) -> (Rect, Rect) {
    // RT-80: `ratio` can arrive unvalidated from a restored/crafted session.json
    // (SavedTab.tree -> Node::Split.ratio is read raw). A NaN rounds to 0 (a pane
    // silently vanishes); a huge value makes `area.x + wa` overflow. Sanitize at
    // the boundary so a corrupt file can never produce an out-of-bounds rect.
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.5
    };
    match axis {
        Axis::LeftRight => {
            let wa = ((area.w as f32) * ratio).round() as usize;
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    w: wa,
                    h: area.h,
                },
                Rect {
                    x: area.x + wa,
                    y: area.y,
                    w: area.w.saturating_sub(wa),
                    h: area.h,
                },
            )
        }
        Axis::TopBottom => {
            let ha = ((area.h as f32) * ratio).round() as usize;
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    w: area.w,
                    h: ha,
                },
                Rect {
                    x: area.x,
                    y: area.y + ha,
                    w: area.w,
                    h: area.h.saturating_sub(ha),
                },
            )
        }
    }
}

/// Content area below the tab bar (height `bar`), spanning the full window.
pub fn content_rect(w: usize, h: usize, bar: usize) -> Rect {
    Rect {
        x: 0,
        y: bar,
        w,
        h: h.saturating_sub(bar),
    }
}

/// A pane's grid area = its full rect minus a title bar of height `title`.
pub fn grid_rect(rect: Rect, title: usize) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y + title,
        w: rect.w,
        h: rect.h.saturating_sub(title),
    }
}

/// Display width (in terminal cells) of a tab/pane name. CA-45: wide East-Asian
/// glyphs occupy two cells and combining/zero-width marks occupy none, so a raw
/// `chars().count()` mis-measures CJK names. The renderer advances the tab strip
/// by this width and the hit-tests (`tab_at`, `app::tab_button_at`) must use the
/// *same* measure, or a click lands on the wrong tab / misses the `×`.
pub fn name_cols(name: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    name.width()
}

/// Index of the tab whose slot sits under pixel `x` on the tab bar.
/// `name_lens` yields each tab name's display width (see `name_cols`). The
/// renderer (`paint::redraw`) draws each tab as a slot of `(len + 2)` label
/// cells plus one close-button (`×`) cell, then advances by a half-cell gap —
/// i.e. the stride is `(len + 2) * cw + cw + cw / 2`. Mirror that exactly
/// (CA-111) so a plain tab-switch click lands on the tab it appears over from
/// the 2nd tab on. (The close-button cell is hit-tested first by
/// `app::tab_button_at`, so including it in the slot here is correct: a
/// tab-switch click anywhere in the drawn slot resolves to that tab.)
///
/// CA-43: `w` caps the strip at the window width exactly as the renderer and
/// `app::tab_button_at` do (they stop drawing/hit-testing once `tx + slot_w`
/// exceeds the window), so `tab_at` never returns an off-screen tab the user
/// can't see or click.
pub fn tab_at(
    name_lens: impl IntoIterator<Item = usize>,
    cw: usize,
    x: usize,
    w: usize,
) -> Option<usize> {
    let mut tx = 0usize;
    for (i, len) in name_lens.into_iter().enumerate() {
        let slot_w = (len + 2) * cw + cw;
        if tx + slot_w > w {
            break; // overflow: matches the renderer's edge cap (CA-43)
        }
        if x >= tx && x < tx + slot_w {
            return Some(i);
        }
        tx += slot_w + cw / 2;
    }
    None
}

/// Map a pixel inside a pane's `grid` to `(column, row, right_half)`. `off` is
/// the scrollback display offset, so `row` can be negative when scrolled up.
/// `right_half` is true when the pixel falls on the right side of the cell
/// (selection caret placement).
pub fn grid_cell(
    grid: Rect,
    x: f64,
    y: f64,
    cols: usize,
    off: usize,
    cw: usize,
    ch: usize,
) -> (usize, i32, bool) {
    let cwf = cw as f64;
    let chf = ch as f64;
    let rel_x = (x - grid.x as f64).max(0.0);
    let rel_y = (y - grid.y as f64).max(0.0);
    let col = ((rel_x / cwf).floor() as usize).min(cols.saturating_sub(1));
    let row = (rel_y / chf).floor() as i32 - off as i32;
    let right_half = (rel_x % cwf) >= cwf / 2.0;
    (col, row, right_half)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(usize),
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    /// Resolve every leaf to a pixel rectangle inside `area`.
    pub fn layout(&self, area: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            Node::Leaf(id) => out.push((*id, area)),
            Node::Split { axis, ratio, a, b } => {
                let (ra, rb) = child_areas(*axis, *ratio, area);
                a.layout(ra, out);
                b.layout(rb, out);
            }
        }
    }

    /// Replace leaf `target` with a split of itself and `new_id`.
    pub fn split_leaf(&mut self, target: usize, new_id: usize, axis: Axis) -> bool {
        match self {
            Node::Leaf(id) => {
                if *id == target {
                    let old = *id;
                    *self = Node::Split {
                        axis,
                        ratio: 0.5,
                        a: Box::new(Node::Leaf(old)),
                        b: Box::new(Node::Leaf(new_id)),
                    };
                    true
                } else {
                    false
                }
            }
            Node::Split { a, b, .. } => {
                a.split_leaf(target, new_id, axis) || b.split_leaf(target, new_id, axis)
            }
        }
    }

    /// Remove leaf `target`, collapsing its parent split. Returns the new tree,
    /// or None if the tree consisted solely of that leaf.
    pub fn without(self, target: usize) -> Option<Node> {
        match self {
            Node::Leaf(id) => {
                if id == target {
                    None
                } else {
                    Some(Node::Leaf(id))
                }
            }
            Node::Split { axis, ratio, a, b } => match (a.without(target), b.without(target)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    axis,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(n), None) | (None, Some(n)) => Some(n),
                (None, None) => None,
            },
        }
    }

    pub fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    /// Path (0 = first child, 1 = second) to the split whose divider is within
    /// `tol` pixels of (x, y). Used for mouse drag-to-resize.
    pub fn divider_at(&self, area: Rect, x: usize, y: usize, tol: usize) -> Option<Vec<u8>> {
        if let Node::Split { axis, ratio, a, b } = self {
            let (ra, rb) = child_areas(*axis, *ratio, area);
            if let Some(mut p) = a.divider_at(ra, x, y, tol) {
                p.insert(0, 0);
                return Some(p);
            }
            if let Some(mut p) = b.divider_at(rb, x, y, tol) {
                p.insert(0, 1);
                return Some(p);
            }
            let hit = match axis {
                Axis::LeftRight => {
                    let dx = ra.x + ra.w;
                    x.abs_diff(dx) <= tol && y >= area.y && y < area.y + area.h
                }
                Axis::TopBottom => {
                    let dy = ra.y + ra.h;
                    y.abs_diff(dy) <= tol && x >= area.x && x < area.x + area.w
                }
            };
            if hit {
                return Some(Vec::new());
            }
        }
        None
    }

    /// Axis + pixel area of the split at `path` (for translating a drag to a ratio).
    pub fn split_area(&self, path: &[u8], area: Rect) -> Option<(Axis, Rect)> {
        match self {
            Node::Split { axis, ratio, a, b } => {
                if path.is_empty() {
                    return Some((*axis, area));
                }
                let (ra, rb) = child_areas(*axis, *ratio, area);
                match path[0] {
                    0 => a.split_area(&path[1..], ra),
                    _ => b.split_area(&path[1..], rb),
                }
            }
            Node::Leaf(_) => None,
        }
    }

    /// Set the ratio of the split at `path`.
    pub fn set_ratio(&mut self, path: &[u8], value: f32) {
        if let Node::Split { ratio, a, b, .. } = self {
            if path.is_empty() {
                *ratio = value.clamp(0.05, 0.95);
            } else if path[0] == 0 {
                a.set_ratio(&path[1..], value);
            } else {
                b.set_ratio(&path[1..], value);
            }
        }
    }

    pub fn contains(&self, id: usize) -> bool {
        match self {
            Node::Leaf(i) => *i == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Grow (or shrink) the pane `target` along `axis` by adjusting the ratio of
    /// the nearest enclosing split with that axis. Returns true if anything moved.
    pub fn resize(&mut self, target: usize, axis: Axis, grow: bool, step: f32) -> bool {
        self.resize_inner(target, axis, if grow { step } else { -step }) == 2
    }

    fn resize_inner(&mut self, target: usize, want_axis: Axis, delta_a: f32) -> u8 {
        match self {
            Node::Leaf(id) => {
                if *id == target {
                    1
                } else {
                    0
                }
            }
            Node::Split { axis, ratio, a, b } => {
                let in_a = a.contains(target);
                let res = if in_a {
                    a.resize_inner(target, want_axis, delta_a)
                } else if b.contains(target) {
                    b.resize_inner(target, want_axis, delta_a)
                } else {
                    return 0;
                };
                match res {
                    0 => 0,
                    2 => 2, // already handled deeper
                    _ => {
                        if *axis == want_axis {
                            let d = if in_a { delta_a } else { -delta_a };
                            *ratio = (*ratio + d).clamp(0.1, 0.9);
                            2
                        } else {
                            1 // keep looking for a matching-axis ancestor
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 60,
    };

    fn rects(node: &Node) -> Vec<(usize, Rect)> {
        let mut v = Vec::new();
        node.layout(AREA, &mut v);
        v
    }

    #[test]
    fn single_leaf_fills_area() {
        let n = Node::Leaf(0);
        assert_eq!(rects(&n), vec![(0, AREA)]);
    }

    #[test]
    fn left_right_split_halves_width() {
        let mut n = Node::Leaf(0);
        assert!(n.split_leaf(0, 1, Axis::LeftRight));
        let r = rects(&n);
        assert_eq!(
            r[0],
            (
                0,
                Rect {
                    x: 0,
                    y: 0,
                    w: 50,
                    h: 60
                }
            )
        );
        assert_eq!(
            r[1],
            (
                1,
                Rect {
                    x: 50,
                    y: 0,
                    w: 50,
                    h: 60
                }
            )
        );
    }

    #[test]
    fn top_bottom_split_halves_height() {
        let mut n = Node::Leaf(0);
        assert!(n.split_leaf(0, 1, Axis::TopBottom));
        let r = rects(&n);
        assert_eq!(
            r[0],
            (
                0,
                Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30
                }
            )
        );
        assert_eq!(
            r[1],
            (
                1,
                Rect {
                    x: 0,
                    y: 30,
                    w: 100,
                    h: 30
                }
            )
        );
    }

    #[test]
    fn nested_split_three_panes() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        n.split_leaf(1, 2, Axis::TopBottom);
        let mut leaves = Vec::new();
        n.leaves(&mut leaves);
        assert_eq!(leaves, vec![0, 1, 2]);
        assert_eq!(rects(&n).len(), 3);
    }

    #[test]
    fn without_collapses_parent() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        let n = n.without(1).expect("tree not empty");
        assert_eq!(rects(&n), vec![(0, AREA)]);
    }

    #[test]
    fn without_last_leaf_is_none() {
        assert!(Node::Leaf(0).without(0).is_none());
    }

    #[test]
    fn split_missing_target_is_noop() {
        let mut n = Node::Leaf(0);
        assert!(!n.split_leaf(99, 1, Axis::LeftRight));
    }

    #[test]
    fn resize_grows_focused_pane() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight); // 0 = left, 1 = right
        assert!(n.resize(0, Axis::LeftRight, true, 0.1)); // grow left pane
        let r = rects(&n);
        assert_eq!(r[0].1.w, 60); // 0.6 * 100
        assert_eq!(r[1].1.w, 40);
    }

    #[test]
    fn resize_right_pane_grows_left() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        assert!(n.resize(1, Axis::LeftRight, true, 0.1)); // grow right pane
        let r = rects(&n);
        assert_eq!(r[1].1.w, 60);
    }

    #[test]
    fn resize_wrong_axis_does_nothing() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        assert!(!n.resize(0, Axis::TopBottom, true, 0.1));
    }

    #[test]
    fn divider_found_near_boundary() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight); // boundary at x=50
        assert_eq!(n.divider_at(AREA, 51, 30, 4), Some(vec![]));
        assert!(n.divider_at(AREA, 10, 30, 4).is_none());
    }

    #[test]
    fn set_ratio_then_layout_reflects_drag() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        let path = n.divider_at(AREA, 50, 30, 4).unwrap();
        n.set_ratio(&path, 0.25);
        let r = rects(&n);
        assert_eq!(r[0].1.w, 25);
        assert_eq!(r[1].1.w, 75);
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert!(r.contains(10, 10)); // top-left inclusive
        assert!(r.contains(29, 29)); // last interior pixel
        assert!(!r.contains(30, 20)); // right edge exclusive
        assert!(!r.contains(20, 30)); // bottom edge exclusive
        assert!(!r.contains(9, 20)); // left of rect
    }

    #[test]
    fn rect_center_is_midpoint() {
        let r = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 60,
        };
        assert_eq!(r.center(), (50, 30));
    }

    #[test]
    fn divider_found_on_horizontal_split() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::TopBottom); // boundary at y=30
        assert_eq!(n.divider_at(AREA, 50, 31, 4), Some(vec![]));
        assert!(n.divider_at(AREA, 50, 10, 4).is_none());
    }

    #[test]
    fn divider_descends_into_nested_split() {
        // left pane is itself split top/bottom; the inner divider has path [0].
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight); // x boundary at 50
        n.split_leaf(0, 2, Axis::TopBottom); // inside left half: y boundary at 30
        let path = n.divider_at(AREA, 25, 31, 4).expect("hit inner divider");
        assert_eq!(path, vec![0]);
        // and the right subtree path begins with 1
        let mut m = Node::Leaf(0);
        m.split_leaf(0, 1, Axis::LeftRight);
        m.split_leaf(1, 2, Axis::TopBottom); // inside right half
        let p2 = m
            .divider_at(AREA, 75, 31, 4)
            .expect("hit right inner divider");
        assert_eq!(p2, vec![1]);
    }

    #[test]
    fn split_area_resolves_axis_and_rect() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        let (axis, area) = n.split_area(&[], AREA).expect("root split");
        assert_eq!(axis, Axis::LeftRight);
        assert_eq!(area, AREA);
        // a leaf has no split area
        assert!(Node::Leaf(0).split_area(&[], AREA).is_none());
    }

    #[test]
    fn split_area_follows_path_into_children() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        n.split_leaf(1, 2, Axis::TopBottom); // right child becomes a split
        let (axis, area) = n.split_area(&[1], AREA).expect("right child split");
        assert_eq!(axis, Axis::TopBottom);
        assert_eq!(area.x, 50); // right half starts at x=50
    }

    #[test]
    fn set_ratio_on_nested_path() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        n.split_leaf(0, 2, Axis::TopBottom); // left child is now a top/bottom split
        n.set_ratio(&[0], 0.25); // adjust the inner (left) split
        let r = rects(&n);
        // left column is 50 wide; its top pane should be 0.25 * 60 = 15 tall
        let top_left = r.iter().find(|(id, _)| *id == 0).unwrap().1;
        assert_eq!(top_left.h, 15);
    }

    #[test]
    fn set_ratio_clamps_to_bounds() {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        n.set_ratio(&[], 5.0); // absurd value clamps to 0.95
        let r = rects(&n);
        assert_eq!(r[0].1.w, 95);
    }

    #[test]
    fn child_areas_sanitizes_hostile_ratio() {
        // RT-80: a corrupt session.json can carry NaN / Infinity / out-of-range
        // ratios. Every case must yield children that partition the parent area
        // exactly and stay in bounds — no vanish-to-zero overflow, no huge rect.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -5.0, 2.0] {
            let (a, b) = child_areas(Axis::LeftRight, bad, AREA);
            assert!(a.w <= AREA.w, "ratio {bad}: left width {} > area", a.w);
            assert_eq!(a.x + a.w, b.x, "ratio {bad}: children must be contiguous");
            assert_eq!(a.w + b.w, AREA.w, "ratio {bad}: must partition width");
            assert!(
                b.x + b.w <= AREA.x + AREA.w,
                "ratio {bad}: right out of bounds"
            );
        }
        // NaN falls back to the 0.5 midpoint rather than collapsing a pane to 0.
        let (a, _) = child_areas(Axis::LeftRight, f32::NAN, AREA);
        assert_eq!(a.w, 50);
    }

    #[test]
    fn content_rect_subtracts_bar() {
        assert_eq!(
            content_rect(800, 600, 20),
            Rect {
                x: 0,
                y: 20,
                w: 800,
                h: 580
            }
        );
        // bar taller than window saturates to zero height, not underflow
        assert_eq!(content_rect(800, 10, 20).h, 0);
    }

    #[test]
    fn grid_rect_subtracts_title() {
        let pane = Rect {
            x: 5,
            y: 10,
            w: 100,
            h: 50,
        };
        assert_eq!(
            grid_rect(pane, 12),
            Rect {
                x: 5,
                y: 22,
                w: 100,
                h: 38
            }
        );
        assert_eq!(grid_rect(pane, 0), pane); // seamless: no title bar
    }

    #[test]
    fn tab_at_maps_pixels_to_tabs() {
        // CA-111: the slot stride must include the close-button (`×`) cell, so it
        // matches the renderer (paint::redraw) and the close hit-test
        // (app::tab_button_at): slot = (len + 2) * cw + cw, then a cw/2 gap.
        let cw = 10;
        // "ab"(2) -> slot (2+2)*10 + 10 = 50, gap 5; "cde"(3) -> slot (3+2)*10 + 10 = 60.
        let lens = [2usize, 3];
        let w = 1000; // wide enough that nothing overflows
        assert_eq!(tab_at(lens, cw, 0, w), Some(0));
        assert_eq!(tab_at(lens, cw, 49, w), Some(0)); // last pixel of tab 0's slot
        assert_eq!(tab_at(lens, cw, 52, w), None); // in the gap between tabs [50, 55)
        assert_eq!(tab_at(lens, cw, 55, w), Some(1)); // first tab slot 50 + gap 5
        assert_eq!(tab_at(lens, cw, 114, w), Some(1)); // 55 + 60 - 1, last pixel of tab 1
        assert_eq!(tab_at(lens, cw, 115, w), None); // first pixel past tab 1's slot
        assert_eq!(tab_at(lens, cw, 1000, w), None); // past the last tab
    }

    #[test]
    fn tab_at_never_returns_an_overflowed_tab() {
        // CA-43: a tab whose slot would extend past the window width `w` is not
        // drawn by the renderer, so `tab_at` must not hit-test it either — a
        // click in that off-screen region returns None, matching the renderer
        // and `tab_button_at`'s edge cap (so far tabs aren't "clickable" ghosts).
        let cw = 10;
        // tab 0 slot = 50 (gap 5), tab 1 slot = 50. With w=70, tab 1 would start
        // at tx=55 and end at 105 > 70, so it must be skipped.
        let lens = [2usize, 2];
        let w = 70;
        assert_eq!(tab_at(lens, cw, 10, w), Some(0)); // tab 0 fits and is hit
        assert_eq!(tab_at(lens, cw, 60, w), None); // tab 1 overflows → not hit
    }

    #[test]
    fn name_cols_measures_display_width_not_char_count() {
        // CA-45: ASCII is one cell per char; East-Asian wide glyphs are two; a
        // combining mark adds zero. A raw chars().count() would mis-size every
        // CJK tab, desyncing the renderer from the hit-tests.
        assert_eq!(name_cols("abc"), 3);
        assert_eq!(name_cols("世界"), 4); // two wide CJK glyphs = 4 cells
        assert_eq!(name_cols("a\u{0301}"), 1); // 'a' + combining acute = 1 cell
    }

    #[test]
    fn tab_at_uses_display_width_for_wide_names() {
        // CA-45: a CJK tab name must hit-test by the cells it actually occupies.
        // "世"(width 2) -> slot (2+2)*10 + 10 = 50; next tab starts at 55.
        let cw = 10;
        let w = 1000;
        let lens = [name_cols("世"), 1]; // 2, 1
        assert_eq!(tab_at(lens, cw, 25, w), Some(0)); // inside the wide tab
        assert_eq!(tab_at(lens, cw, 56, w), Some(1)); // second tab after the gap
    }

    /// A three-level tree: root LeftRight( Split(0,1) , Leaf(2) ).
    fn nested() -> Node {
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 2, Axis::LeftRight); // 0 | 2
        n.split_leaf(0, 1, Axis::TopBottom); // left half becomes 0 over 1
        n
    }

    #[test]
    fn without_keeps_both_branches_when_sibling_survives() {
        // Remove a deeply-nested leaf so the top-level split still has two sides.
        let n = nested().without(1).expect("tree not empty");
        let mut leaves = Vec::new();
        n.leaves(&mut leaves);
        leaves.sort();
        assert_eq!(leaves, vec![0, 2]); // 1 gone, both top children remain
                                        // still a split (Some, Some) — not collapsed to a single leaf
        assert!(matches!(n, Node::Split { .. }));
    }

    #[test]
    fn contains_walks_into_split_children() {
        let n = nested();
        assert!(n.contains(1)); // buried in the left subtree's split arm
        assert!(n.contains(2));
        assert!(!n.contains(99));
    }

    #[test]
    fn split_area_descends_first_child_path() {
        let n = nested();
        // path [0] selects the left subtree, which is itself a top/bottom split.
        let (axis, area) = n.split_area(&[0], AREA).expect("left inner split");
        assert_eq!(axis, Axis::TopBottom);
        assert_eq!(area.x, 0); // left half starts at x=0
    }

    #[test]
    fn set_ratio_follows_second_child_path() {
        // root LeftRight( Leaf(0) , Split(1,2) ) — adjust the right subtree via [1].
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 1, Axis::LeftRight);
        n.split_leaf(1, 2, Axis::TopBottom);
        n.set_ratio(&[1], 0.25);
        let r = rects(&n);
        // right column top pane (id 1) should be 0.25 * 60 = 15 tall
        let top_right = r.iter().find(|(id, _)| *id == 1).unwrap().1;
        assert_eq!(top_right.h, 15);
    }

    #[test]
    fn resize_absent_target_is_noop() {
        let mut n = nested();
        assert!(!n.resize(99, Axis::LeftRight, true, 0.1)); // target not in tree
    }

    #[test]
    fn resize_finds_matching_axis_ancestor_through_nested_split() {
        // Growing pane 1 (in the inner top/bottom split) along LeftRight must
        // walk up past the non-matching inner split to the matching root split.
        let mut n = nested();
        assert!(n.resize(1, Axis::LeftRight, true, 0.1));
    }

    #[test]
    fn resize_stops_at_innermost_matching_axis() {
        // Two nested LeftRight splits: resizing pane 0 must be handled by the
        // inner split and the outer split must propagate the "done" (2) result
        // upward without adjusting itself again.
        let mut n = Node::Leaf(0);
        n.split_leaf(0, 2, Axis::LeftRight); // outer: (0) | 2
        n.split_leaf(0, 1, Axis::LeftRight); // inner: (0 | 1) | 2
        let before = rects(&n);
        assert!(n.resize(0, Axis::LeftRight, true, 0.1));
        let after = rects(&n);
        // pane 0 grew; pane 2 (under the outer split) is unchanged.
        let w0 = |rs: &[(usize, Rect)]| rs.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        let w2 = |rs: &[(usize, Rect)]| rs.iter().find(|(id, _)| *id == 2).unwrap().1.w;
        assert!(w0(&after) > w0(&before), "inner pane should have grown");
        assert_eq!(w2(&before), w2(&after), "outer split must not move");
    }

    #[test]
    fn set_ratio_on_a_leaf_is_a_noop() {
        // set_ratio targets splits; a leaf has no ratio, so it must do nothing
        // (and not panic) when handed any path.
        let mut leaf = Node::Leaf(0);
        leaf.set_ratio(&[], 0.5);
        leaf.set_ratio(&[0, 1], 0.5);
        assert_eq!(leaf, Node::Leaf(0));
    }

    #[test]
    fn grid_cell_maps_pixel_to_column_row_side() {
        let grid = Rect {
            x: 100,
            y: 50,
            w: 200,
            h: 100,
        };
        // pixel 5px into first cell of a 10x20 grid: col 0, row 0, left half
        let (col, row, right) = grid_cell(grid, 104.0, 52.0, 80, 0, 10, 20);
        assert_eq!((col, row, right), (0, 0, false));
        // 6px into the cell is the right half
        let (_, _, right2) = grid_cell(grid, 106.0, 52.0, 80, 0, 10, 20);
        assert!(right2);
        // scrollback offset shifts the row up (can go negative)
        let (_, row3, _) = grid_cell(grid, 104.0, 52.0, 80, 5, 10, 20);
        assert_eq!(row3, -5);
        // a pixel above/left of the grid clamps to col 0, not underflow
        let (col4, _, _) = grid_cell(grid, 0.0, 0.0, 80, 0, 10, 20);
        assert_eq!(col4, 0);
        // column clamps to the last column
        let (col5, _, _) = grid_cell(grid, 100_000.0, 52.0, 80, 0, 10, 20);
        assert_eq!(col5, 79);
    }
}
