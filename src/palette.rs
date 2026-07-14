// Command palette: a fuzzy-searchable list of actions (the "Control Center").

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    SplitRight,
    SearchScrollback,
    ShowHelp,
    SplitDown,
    ClosePane,
    RenamePane,
    RenameTab,
    NewTab,
    NextTab,
    PrevTab,
    ToggleBroadcast,
    BroadcastPasteAll,
    BroadcastEnterAll,
    ToggleSeamless,
    MoveTabToNewWindow,
    SaveSession,
    LoadSession,
    ToggleAgents,
}

/// `(label, shortcut, command)`. The shortcut column is rendered dim and
/// right-aligned in the palette, so every palette use teaches the direct
/// keybinding — the palette is the discovery surface, the keys are the fast
/// path. Empty string = no direct binding (palette-only action).
pub const COMMANDS: &[(&str, &str, Cmd)] = &[
    ("split right", "Ctrl+Shift+D", Cmd::SplitRight),
    ("split down", "Ctrl+Shift+E", Cmd::SplitDown),
    ("close pane", "Ctrl+Shift+W", Cmd::ClosePane),
    ("rename pane", "Ctrl+Shift+R", Cmd::RenamePane),
    ("rename tab", "", Cmd::RenameTab),
    ("new tab", "Ctrl+Shift+T", Cmd::NewTab),
    ("next tab", "Ctrl+Tab", Cmd::NextTab),
    ("previous tab", "Ctrl+Shift+Tab", Cmd::PrevTab),
    ("search scrollback", "Ctrl+Shift+F", Cmd::SearchScrollback),
    ("keybinding help", "F1", Cmd::ShowHelp),
    ("toggle broadcast input", "", Cmd::ToggleBroadcast),
    (
        "paste clipboard to all panes in the tab",
        "Ctrl+Shift+B",
        Cmd::BroadcastPasteAll,
    ),
    (
        "press Enter in all panes in the tab",
        "Ctrl+Shift+Enter",
        Cmd::BroadcastEnterAll,
    ),
    (
        "agent overview (jump to a pane)",
        "Ctrl+Shift+A",
        Cmd::ToggleAgents,
    ),
    ("toggle seamless mode", "", Cmd::ToggleSeamless),
    (
        "move tab to new window",
        "Ctrl+Shift+N",
        Cmd::MoveTabToNewWindow,
    ),
    ("save session", "", Cmd::SaveSession),
    ("load session", "", Cmd::LoadSession),
];

pub struct Palette {
    pub query: String,
    pub sel: usize,
}

impl Palette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            sel: 0,
        }
    }

    /// Commands matching the query, best score first: `(label, shortcut, cmd)`.
    pub fn matches(&self) -> Vec<(&'static str, &'static str, Cmd)> {
        let mut scored: Vec<(i32, &'static str, &'static str, Cmd)> = COMMANDS
            .iter()
            .filter_map(|(label, keys, cmd)| {
                crate::fuzzy::score(&self.query, label).map(|s| (s, *label, *keys, *cmd))
            })
            .collect();
        scored.sort_by_key(|t| core::cmp::Reverse(t.0));
        scored.into_iter().map(|(_, l, k, c)| (l, k, c)).collect()
    }

    pub fn selected(&self) -> Option<Cmd> {
        self.matches().get(self.sel).map(|(_, _, c)| *c)
    }

    pub fn clamp_selection(&mut self) {
        let n = self.matches().len();
        if n == 0 {
            self.sel = 0;
        } else if self.sel >= n {
            self.sel = n - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_to_matching_commands() {
        let mut p = Palette::new();
        p.query = "broad".into();
        let m = p.matches();
        assert!(m.iter().any(|(_, _, c)| *c == Cmd::ToggleBroadcast));
        assert!(!m.iter().any(|(_, _, c)| *c == Cmd::NewTab));
    }

    #[test]
    fn empty_query_lists_all() {
        let p = Palette::new();
        assert_eq!(p.matches().len(), COMMANDS.len());
    }

    #[test]
    fn selected_returns_command_at_index() {
        let mut p = Palette::new();
        // empty query: selection 0 is the first command in declaration order.
        assert!(p.selected() == Some(Cmd::SplitRight));
        p.sel = 2;
        assert!(p.selected() == p.matches().get(2).map(|(_, _, c)| *c));
    }

    #[test]
    fn selected_is_none_when_no_matches() {
        let mut p = Palette::new();
        p.query = "zzzzz-no-such-command".into();
        assert!(p.matches().is_empty());
        assert!(p.selected().is_none());
    }

    #[test]
    fn clamp_selection_pins_to_last_match() {
        let mut p = Palette::new();
        p.sel = 999;
        p.clamp_selection();
        assert_eq!(p.sel, COMMANDS.len() - 1);
    }

    #[test]
    fn clamp_selection_resets_when_no_matches() {
        let mut p = Palette::new();
        p.query = "zzzzz".into();
        p.sel = 5;
        p.clamp_selection();
        assert_eq!(p.sel, 0);
    }

    #[test]
    fn clamp_selection_leaves_valid_index_untouched() {
        let mut p = Palette::new();
        p.sel = 1;
        p.clamp_selection();
        assert_eq!(p.sel, 1);
    }
}
