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

impl Pane {
    pub fn new(name: String, cols: usize, rows: usize, proxy: EventLoopProxy<Wake>) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let term = Terminal::new(cols, rows);
        let waker = move || {
            let _ = proxy.send_event(Wake);
        };
        let pty = Pty::spawn("pwsh.exe", &["-NoLogo"], rows as u16, cols as u16, waker.clone())
            .or_else(|_| Pty::spawn("cmd.exe", &[], rows as u16, cols as u16, waker))
            .expect("spawn a native shell");
        Self { term, pty, name, proc_name: String::new() }
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
    pub fn new(name: String, color: u32, cols: usize, rows: usize, proxy: EventLoopProxy<Wake>) -> Self {
        let mut panes = HashMap::new();
        panes.insert(0, Pane::new("term 1".into(), cols, rows, proxy));
        Self { panes, tree: Node::Leaf(0), focus: 0, name, color, next_id: 1 }
    }

    /// Rebuild a tab from a saved snapshot, spawning a fresh shell per pane.
    pub fn from_saved(
        saved: &crate::persist::SavedTab,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
    ) -> Self {
        let mut panes = HashMap::new();
        for sp in &saved.panes {
            panes.insert(sp.id, Pane::new(sp.name.clone(), cols, rows, proxy.clone()));
        }
        Self {
            panes,
            tree: saved.tree.clone(),
            focus: saved.focus,
            name: saved.name.clone(),
            color: saved.color,
            next_id: saved.next_id,
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
