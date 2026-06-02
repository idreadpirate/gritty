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

#[derive(Debug)]
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
            Node::Split { axis, ratio, a, b } => match axis {
                Axis::LeftRight => {
                    let wa = ((area.w as f32) * ratio).round() as usize;
                    a.layout(Rect { x: area.x, y: area.y, w: wa, h: area.h }, out);
                    b.layout(
                        Rect { x: area.x + wa, y: area.y, w: area.w.saturating_sub(wa), h: area.h },
                        out,
                    );
                }
                Axis::TopBottom => {
                    let ha = ((area.h as f32) * ratio).round() as usize;
                    a.layout(Rect { x: area.x, y: area.y, w: area.w, h: ha }, out);
                    b.layout(
                        Rect { x: area.x, y: area.y + ha, w: area.w, h: area.h.saturating_sub(ha) },
                        out,
                    );
                }
            },
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
            Node::Split { axis, ratio, a, b } => {
                match (a.without(target), b.without(target)) {
                    (Some(a), Some(b)) => Some(Node::Split {
                        axis,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (Some(n), None) | (None, Some(n)) => Some(n),
                    (None, None) => None,
                }
            }
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

    const AREA: Rect = Rect { x: 0, y: 0, w: 100, h: 60 };

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
        assert_eq!(r[0], (0, Rect { x: 0, y: 0, w: 50, h: 60 }));
        assert_eq!(r[1], (1, Rect { x: 50, y: 0, w: 50, h: 60 }));
    }

    #[test]
    fn top_bottom_split_halves_height() {
        let mut n = Node::Leaf(0);
        assert!(n.split_leaf(0, 1, Axis::TopBottom));
        let r = rects(&n);
        assert_eq!(r[0], (0, Rect { x: 0, y: 0, w: 100, h: 30 }));
        assert_eq!(r[1], (1, Rect { x: 0, y: 30, w: 100, h: 30 }));
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
}
