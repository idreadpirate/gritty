// Command palette: a fuzzy-searchable list of actions (the "Control Center").

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    SplitRight,
    SplitDown,
    ClosePane,
    RenamePane,
    NewTab,
    NextTab,
    PrevTab,
    ToggleBroadcast,
    ToggleSeamless,
    SaveSession,
    LoadSession,
}

pub const COMMANDS: &[(&str, Cmd)] = &[
    ("split right", Cmd::SplitRight),
    ("split down", Cmd::SplitDown),
    ("close pane", Cmd::ClosePane),
    ("rename pane", Cmd::RenamePane),
    ("new tab", Cmd::NewTab),
    ("next tab", Cmd::NextTab),
    ("previous tab", Cmd::PrevTab),
    ("toggle broadcast input", Cmd::ToggleBroadcast),
    ("toggle seamless mode", Cmd::ToggleSeamless),
    ("save session", Cmd::SaveSession),
    ("load session", Cmd::LoadSession),
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

    /// Commands matching the query, best score first.
    pub fn matches(&self) -> Vec<(&'static str, Cmd)> {
        let mut scored: Vec<(i32, &'static str, Cmd)> = COMMANDS
            .iter()
            .filter_map(|(label, cmd)| {
                crate::fuzzy::score(&self.query, label).map(|s| (s, *label, *cmd))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, l, c)| (l, c)).collect()
    }

    pub fn selected(&self) -> Option<Cmd> {
        self.matches().get(self.sel).map(|(_, c)| *c)
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
        assert!(m.iter().any(|(_, c)| *c == Cmd::ToggleBroadcast));
        assert!(!m.iter().any(|(_, c)| *c == Cmd::NewTab));
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
        assert!(p.selected() == p.matches().get(2).map(|(_, c)| *c));
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
