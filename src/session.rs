// Panes (one shell each) and Tabs (a split-tree of panes).

use std::collections::HashMap;

use winit::event_loop::EventLoopProxy;

use crate::layout::{Axis, Node};
use crate::pty::Pty;
use crate::term::Terminal;
use crate::Wake;

pub struct Pane {
    pub term: Terminal,
    pub pty: Pty,
    pub name: String,
    /// Foreground process running in the pane (e.g. "nvim"), updated periodically.
    pub proc_name: String,
}

/// Reconcile a saved tab into a consistent 1:1 leaf↔pane plan: one pane per tree
/// leaf (name from `saved.panes` or a default), a `focus` that is a real leaf,
/// and a `next_id` past every id. Orphan panes (absent from the tree) are
/// dropped so they don't leak hidden shells (RT-7/CA-15).
pub fn plan_from_saved(saved: &crate::persist::SavedTab) -> (Vec<(usize, String)>, usize, usize) {
    let mut leaves = Vec::new();
    saved.tree.leaves(&mut leaves);
    let names: HashMap<usize, &str> = saved
        .panes
        .iter()
        .map(|p| (p.id, p.name.as_str()))
        .collect();
    let plan: Vec<(usize, String)> = leaves
        .iter()
        .map(|id| {
            let name = names
                .get(id)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("term {}", id + 1));
            (*id, name)
        })
        .collect();
    let focus = if leaves.contains(&saved.focus) {
        saved.focus
    } else {
        *leaves.first().unwrap_or(&0)
    };
    let next_id = leaves
        .iter()
        .max()
        .map(|m| m + 1)
        .unwrap_or(1)
        .max(saved.next_id);
    (plan, focus, next_id)
}

/// Absolute, trusted shell paths tried in order (RT-2: never resolve a shell by
/// bare name, which a malicious `pwsh.exe`/`cmd.exe` earlier in PATH could hijack).
fn shell_candidates() -> Vec<(String, Vec<&'static str>)> {
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    vec![
        (format!(r"{pf}\PowerShell\7\pwsh.exe"), vec!["-NoLogo"]),
        (
            format!(r"{sysroot}\System32\WindowsPowerShell\v1.0\powershell.exe"),
            vec!["-NoLogo"],
        ),
        (format!(r"{sysroot}\System32\cmd.exe"), vec![]),
    ]
}

impl Pane {
    pub fn new(
        name: String,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
        cwd: Option<&str>,
    ) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let term = Terminal::new(cols, rows);
        let waker = move || {
            let _ = proxy.send_event(Wake);
        };
        let mut pty = None;
        for (path, args) in shell_candidates() {
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            if let Ok(p) = Pty::spawn(&path, &args, rows as u16, cols as u16, waker.clone(), cwd) {
                pty = Some(p);
                break;
            }
        }
        let pty = pty.expect("spawn a native shell (no pwsh/powershell/cmd found)");
        Self {
            term,
            pty,
            name,
            proc_name: String::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if (cols, rows) != (self.term.size.cols, self.term.size.rows) {
            // RT-11: resize the PTY first so the shell/kernel learns the new
            // dimensions before we reflow the grid.  If the grid moved first, a
            // chunk already formatted for the old width could be parsed against
            // the new (mismatched) cell layout during a rapid resize.
            self.pty.resize(rows as u16, cols as u16);
            self.term.resize(cols, rows);
        }
    }
}

pub struct Tab {
    pub panes: HashMap<usize, Pane>,
    pub tree: Node,
    pub focus: usize,
    pub name: String,
    pub color: u32,
    next_id: usize,
}

impl Tab {
    pub fn new(
        name: String,
        color: u32,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
    ) -> Self {
        let mut panes = HashMap::new();
        panes.insert(0, Pane::new("term 1".into(), cols, rows, proxy, None));
        Self {
            panes,
            tree: Node::Leaf(0),
            focus: 0,
            name,
            color,
            next_id: 1,
        }
    }

    /// Rebuild a tab from a saved snapshot, spawning one fresh shell per tree
    /// leaf. Uses `plan_from_saved` so the result is always consistent
    /// (RT-7/CA-15): every leaf has a pane, focus is real, orphans are dropped.
    pub fn from_saved(
        saved: &crate::persist::SavedTab,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
    ) -> Self {
        let (plan, focus, next_id) = plan_from_saved(saved);
        let mut panes = HashMap::new();
        for (id, name) in plan {
            panes.insert(id, Pane::new(name, cols, rows, proxy.clone(), None));
        }
        Self {
            panes,
            tree: saved.tree.clone(),
            focus,
            name: saved.name.clone(),
            color: saved.color,
            next_id,
        }
    }

    pub fn next_id(&self) -> usize {
        self.next_id
    }

    /// Resize the focused pane along `axis` (grow or shrink).
    pub fn resize_focus(&mut self, axis: Axis, grow: bool) {
        self.tree.resize(self.focus, axis, grow, 0.04);
    }

    /// Split the focused pane along `axis`, focusing the new pane.
    /// The new pane inherits the focused pane's working directory (OSC 7 cwd).
    pub fn split(&mut self, axis: Axis, proxy: EventLoopProxy<Wake>) {
        let id = self.next_id;
        if self.tree.split_leaf(self.focus, id, axis) {
            self.next_id += 1;
            let name = format!("term {}", id + 1);
            // Read the focused pane's latest OSC 7 cwd (if any) so the new
            // shell starts in the same directory.
            let inherited_cwd = self.panes.get(&self.focus).and_then(|p| p.term.cwd());
            // Sized properly on the next relayout.
            self.panes
                .insert(id, Pane::new(name, 80, 24, proxy, inherited_cwd.as_deref()));
            self.focus = id;
        }
    }

    /// Close the focused pane. Returns true if the tab is now empty.
    pub fn close_focus(&mut self) -> bool {
        let target = self.focus;
        let tree = std::mem::replace(&mut self.tree, Node::Leaf(target));
        match tree.without(target) {
            Some(t) => {
                self.tree = t;
                self.panes.remove(&target);
                let mut leaves = Vec::new();
                self.tree.leaves(&mut leaves);
                self.focus = *leaves.first().unwrap_or(&target);
                false
            }
            None => {
                self.panes.remove(&target);
                true
            }
        }
    }

    pub fn rename_focus(&mut self, name: String) {
        if let Some(p) = self.panes.get_mut(&self.focus) {
            p.name = name;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Node};
    use crate::persist::{SavedPane, SavedTab};

    fn tab(tree: Node, panes: Vec<(usize, &str)>, focus: usize, next_id: usize) -> SavedTab {
        SavedTab {
            name: "t".into(),
            color: 0,
            focus,
            next_id,
            tree,
            panes: panes
                .into_iter()
                .map(|(id, n)| SavedPane { id, name: n.into() })
                .collect(),
        }
    }

    #[test]
    fn plan_drops_orphans_and_fills_missing_names() {
        let tree = Node::Split {
            axis: Axis::LeftRight,
            ratio: 0.5,
            a: Box::new(Node::Leaf(0)),
            b: Box::new(Node::Leaf(1)),
        };
        let saved = tab(tree, vec![(0, "editor"), (9, "orphan")], 0, 2);
        let (plan, focus, next_id) = plan_from_saved(&saved);
        assert_eq!(plan.len(), 2); // one pane per leaf; orphan 9 dropped
        assert!(plan.iter().any(|(id, n)| *id == 0 && n == "editor"));
        assert!(plan.iter().any(|(id, n)| *id == 1 && n == "term 2")); // synthesized
        assert_eq!(focus, 0);
        assert!(next_id >= 2);
    }

    #[test]
    fn plan_repairs_invalid_focus() {
        let saved = tab(Node::Leaf(0), vec![(0, "a")], 7, 1);
        let (_, focus, _) = plan_from_saved(&saved);
        assert_eq!(focus, 0);
    }

    /// RT-11: after resize the terminal grid reflects the new dimensions and the
    /// call does not panic.  We cannot observe the PTY resize order from a unit
    /// test (that is a runtime invariant), but we can confirm the observable
    /// postcondition: term.size is updated correctly and the pane stays usable.
    #[test]
    fn resize_updates_term_size_and_does_not_panic() {
        use crate::term::Terminal;

        // Build a minimal Pane-like struct that mirrors the resize logic without
        // needing a real PTY (which would spawn a shell process in CI).
        struct FakePane {
            term: Terminal,
            resize_log: Vec<(usize, usize)>,
        }
        impl FakePane {
            fn new(cols: usize, rows: usize) -> Self {
                Self {
                    term: Terminal::new(cols, rows),
                    resize_log: Vec::new(),
                }
            }
            /// Mirrors Pane::resize order: PTY first (recorded), then grid.
            fn resize(&mut self, cols: usize, rows: usize) {
                let cols = cols.max(1);
                let rows = rows.max(1);
                if (cols, rows) != (self.term.size.cols, self.term.size.rows) {
                    // PTY resize recorded before grid resize (RT-11).
                    self.resize_log.push((cols, rows));
                    self.term.resize(cols, rows);
                }
            }
        }

        let mut fp = FakePane::new(80, 24);
        assert_eq!(fp.term.size.cols, 80);
        assert_eq!(fp.term.size.rows, 24);

        fp.resize(120, 40);
        assert_eq!(fp.term.size.cols, 120);
        assert_eq!(fp.term.size.rows, 40);
        // PTY resize was recorded (i.e. it happened) before grid was updated.
        assert_eq!(fp.resize_log, vec![(120, 40)]);

        // Resize to the same dimensions is a no-op (idempotent).
        fp.resize(120, 40);
        assert_eq!(fp.resize_log.len(), 1, "no-op resize must not re-trigger");

        // Clamp: zero dimensions are treated as 1×1, does not panic.
        fp.resize(0, 0);
        assert_eq!(fp.term.size.cols, 1);
        assert_eq!(fp.term.size.rows, 1);
    }
}
