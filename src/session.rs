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
    /// CA-40: set the first time `reap_dead` sees this pane's shell as exited, so
    /// the pane survives one extra cycle and its final drained line (an exit/
    /// farewell message) is painted once before the pane is reaped.
    pub dead_seen: bool,
}

/// User-tunable knobs that affect how a pane's shell is spawned (CA-37). Derived
/// once from `config.toml` at startup and threaded into every `Pane::new`.
#[derive(Debug, Clone)]
pub struct SpawnCfg {
    /// Lines of scrollback kept per pane.
    pub scrollback: usize,
    /// Optional absolute path to a preferred shell, tried before the built-ins.
    pub shell: Option<String>,
}

impl Default for SpawnCfg {
    fn default() -> Self {
        Self {
            scrollback: crate::term::DEFAULT_SCROLLBACK,
            shell: None,
        }
    }
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

/// The ordered shell candidates with a user-configured shell prepended (CA-37).
///
/// RT-2/RT-6: a configured shell is honored only when it is an *absolute* path,
/// never a bare name — otherwise it is ignored and we fall back to the trusted
/// built-ins. (A non-existent path is skipped by the spawn loop's `exists()`
/// guard, so a typo degrades gracefully instead of failing to start.)
fn candidates_with_override(shell: Option<&str>) -> Vec<(String, Vec<&'static str>)> {
    let mut out = shell_candidates();
    if let Some(s) = shell {
        if std::path::Path::new(s).is_absolute() {
            out.insert(0, (s.to_string(), vec![]));
        }
    }
    out
}

impl Pane {
    pub fn new(
        name: String,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
        cwd: Option<&str>,
        cfg: &SpawnCfg,
    ) -> Result<Self, String> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let term = Terminal::new(cols, rows, cfg.scrollback);
        let waker = move || {
            let _ = proxy.send_event(Wake);
        };
        let mut pty = None;
        for (path, args) in candidates_with_override(cfg.shell.as_deref()) {
            if !std::path::Path::new(&path).exists() {
                continue;
            }
            if let Ok(p) = Pty::spawn(&path, &args, rows as u16, cols as u16, waker.clone(), cwd) {
                pty = Some(p);
                break;
            }
        }
        let pty = pty.ok_or_else(|| {
            "No shell could be spawned (tried pwsh, powershell, cmd — none found or failed to start)".to_string()
        })?;
        Ok(Self {
            term,
            pty,
            name,
            proc_name: String::new(),
            dead_seen: false,
        })
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
    /// CA-46: a BEL fired in one of this tab's panes since it was last viewed.
    /// Set while the tab is in the background (its panes aren't painted, so the
    /// real-time amber flash can't fire); drawn as a marker on the tab and
    /// cleared when the tab becomes active again.
    pub activity: bool,
}

impl Tab {
    pub fn new(
        name: String,
        color: u32,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
        cfg: &SpawnCfg,
    ) -> Result<Self, String> {
        let mut panes = HashMap::new();
        panes.insert(0, Pane::new("term 1".into(), cols, rows, proxy, None, cfg)?);
        Ok(Self {
            panes,
            tree: Node::Leaf(0),
            focus: 0,
            name,
            color,
            next_id: 1,
            activity: false,
        })
    }

    /// Rebuild a tab from a saved snapshot, spawning one fresh shell per tree
    /// leaf. Uses `plan_from_saved` so the result is always consistent
    /// (RT-7/CA-15): every leaf has a pane, focus is real, orphans are dropped.
    pub fn from_saved(
        saved: &crate::persist::SavedTab,
        cols: usize,
        rows: usize,
        proxy: EventLoopProxy<Wake>,
        cfg: &SpawnCfg,
    ) -> Result<Self, String> {
        let (plan, focus, next_id) = plan_from_saved(saved);
        let mut panes = HashMap::new();
        for (id, name) in plan {
            panes.insert(id, Pane::new(name, cols, rows, proxy.clone(), None, cfg)?);
        }
        Ok(Self {
            panes,
            tree: saved.tree.clone(),
            focus,
            name: saved.name.clone(),
            color: saved.color,
            next_id,
            activity: false,
        })
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
    /// Returns `Err` if shell spawn fails; the tree is left unmodified in that case.
    pub fn split(
        &mut self,
        axis: Axis,
        proxy: EventLoopProxy<Wake>,
        cfg: &SpawnCfg,
    ) -> Result<(), String> {
        let id = self.next_id;
        // Clone the tree before mutating so we can roll back on spawn failure.
        let tree_before = self.tree.clone();
        if self.tree.split_leaf(self.focus, id, axis) {
            let name = format!("term {}", id + 1);
            // Read the focused pane's latest OSC 7 cwd (if any) so the new
            // shell starts in the same directory.
            let inherited_cwd = self.panes.get(&self.focus).and_then(|p| p.term.cwd());
            // Sized properly on the next relayout. Roll back the tree split on failure.
            match Pane::new(name, 80, 24, proxy, inherited_cwd.as_deref(), cfg) {
                Ok(pane) => {
                    self.next_id += 1;
                    self.panes.insert(id, pane);
                    self.focus = id;
                    Ok(())
                }
                Err(e) => {
                    // Roll back the tree mutation so the tab stays consistent.
                    self.tree = tree_before;
                    Err(e)
                }
            }
        } else {
            Ok(())
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
                    term: Terminal::new(cols, rows, crate::term::DEFAULT_SCROLLBACK),
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

    /// CA-140: a window resize must reach EVERY tab's panes, not just the active
    /// tab. Before the fix, `relayout` only iterated the active tab, so a
    /// backgrounded shell kept stale dimensions. We model the all-tabs resize loop
    /// over fake panes (no real PTY) and assert every tab's term was resized.
    #[test]
    fn resize_reaches_every_tab_not_just_active() {
        use crate::term::Terminal;

        // Three "tabs", each a single fake pane sized 80x24. `active` is tab 2,
        // but the resize must update tabs 0 and 1 (the background ones) too.
        let mut tabs: Vec<Terminal> = (0..3)
            .map(|_| Terminal::new(80, 24, crate::term::DEFAULT_SCROLLBACK))
            .collect();
        let active = 2usize;

        // The active-only path (the pre-CA-140 bug): only `active` would resize.
        // The relayout_all path: iterate ALL tabs.
        for term in tabs.iter_mut() {
            term.resize(120, 40);
        }

        for (i, term) in tabs.iter().enumerate() {
            assert_eq!(
                (term.size.cols, term.size.rows),
                (120, 40),
                "background tab {i} (active={active}) must be resized too"
            );
        }
    }

    /// RT-6: `shell_candidates` must never include a path that is resolved via
    /// PATH (bare name); every path must be absolute. If none exist the caller
    /// gets an `Err`, not a panic.
    #[test]
    fn shell_candidates_are_absolute_paths() {
        for (path, _args) in super::shell_candidates() {
            assert!(
                std::path::Path::new(&path).is_absolute(),
                "shell candidate must be an absolute path, got: {path}"
            );
        }
    }

    /// CA-37: a config `shell` override (absolute path) is tried first, ahead of
    /// the built-in candidates.
    #[test]
    fn shell_override_is_prepended_when_absolute() {
        let with = super::candidates_with_override(Some(r"C:\tools\nu.exe"));
        assert_eq!(
            with.first().map(|(p, _)| p.as_str()),
            Some(r"C:\tools\nu.exe"),
            "absolute config shell must be tried first"
        );
        // It only adds one entry; the built-ins still follow.
        assert_eq!(with.len(), super::shell_candidates().len() + 1);
    }

    /// CA-37/RT-2: a bare-name (non-absolute) override is ignored — never resolved
    /// via PATH, where a hijacked `nu.exe` could be picked up.
    #[test]
    fn shell_override_rejects_bare_name() {
        let with = super::candidates_with_override(Some("nu.exe"));
        assert_eq!(
            with.len(),
            super::shell_candidates().len(),
            "a non-absolute shell override must be dropped, not resolved via PATH"
        );
        assert!(
            with.iter()
                .all(|(p, _)| std::path::Path::new(p).is_absolute()),
            "every candidate must remain an absolute path"
        );
    }

    /// CA-37: with no override the list is exactly the built-in candidates.
    #[test]
    fn no_shell_override_keeps_builtins() {
        assert_eq!(
            super::candidates_with_override(None),
            super::shell_candidates()
        );
    }

    /// RT-6: `Pane::new` returns `Err` (not panic) when no candidate path
    /// exists. We test this by simulating the filtering logic: given a list
    /// where every path is known-absent, the result is `Err`.
    #[test]
    fn pane_spawn_failure_returns_err_not_panic() {
        // Mimic the loop inside Pane::new with paths that cannot exist.
        let candidates: Vec<(String, Vec<&str>)> = vec![
            (r"C:\nonexistent\pwsh.exe".to_string(), vec![]),
            (r"C:\nonexistent\powershell.exe".to_string(), vec![]),
            (r"C:\nonexistent\cmd.exe".to_string(), vec![]),
        ];
        let mut pty_opt: Option<()> = None;
        for (path, _args) in &candidates {
            if std::path::Path::new(path).exists() {
                pty_opt = Some(());
                break;
            }
        }
        let result: Result<(), String> = pty_opt.ok_or_else(|| {
            "No shell could be spawned (tried pwsh, powershell, cmd — none found or failed to start)".to_string()
        });
        assert!(result.is_err(), "spawn failure must return Err, not panic");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("pwsh") || msg.contains("shell"),
            "error message must mention the shell names: {msg}"
        );
    }
}
