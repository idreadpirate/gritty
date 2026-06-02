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
    pub fn new(name: String, cols: usize, rows: usize, proxy: EventLoopProxy<Wake>) -> Self {
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
            if let Ok(p) = Pty::spawn(&path, &args, rows as u16, cols as u16, waker.clone()) {
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
            self.term.resize(cols, rows);
            self.pty.resize(rows as u16, cols as u16);
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
        panes.insert(0, Pane::new("term 1".into(), cols, rows, proxy));
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
            panes.insert(id, Pane::new(name, cols, rows, proxy.clone()));
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
    pub fn split(&mut self, axis: Axis, proxy: EventLoopProxy<Wake>) {
        let id = self.next_id;
        if self.tree.split_leaf(self.focus, id, axis) {
            self.next_id += 1;
            let name = format!("term {}", id + 1);
            // Sized properly on the next relayout.
            self.panes.insert(id, Pane::new(name, 80, 24, proxy));
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

    #[test]
    fn shell_candidates_are_absolute_trusted_paths() {
        let cands = shell_candidates();
        // pwsh 7, Windows PowerShell, then cmd — in that fallback order.
        assert_eq!(cands.len(), 3);
        assert!(cands[0].0.ends_with(r"\PowerShell\7\pwsh.exe"));
        assert!(cands[1].0.to_lowercase().ends_with("powershell.exe"));
        assert!(cands[2].0.to_lowercase().ends_with(r"\system32\cmd.exe"));

        // RT-2: every path must be drive-absolute, never a bare name a malicious
        // exe earlier in PATH could hijack.
        for (path, _) in &cands {
            assert!(
                path.len() > 3 && path.as_bytes()[1] == b':',
                "shell path not absolute: {path}"
            );
            assert!(!path.contains('/'), "expected Windows separators: {path}");
        }

        // PowerShell variants launch with -NoLogo; cmd takes no args.
        assert_eq!(cands[0].1, vec!["-NoLogo"]);
        assert_eq!(cands[1].1, vec!["-NoLogo"]);
        assert!(cands[2].1.is_empty());
    }

    #[test]
    fn shell_candidates_honor_env_roots() {
        // Paths must sit under the SystemRoot the function reads from the env.
        let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let cands = shell_candidates();
        assert!(cands[2].0.starts_with(&sysroot));
    }
}
