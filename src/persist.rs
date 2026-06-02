// Session persistence: save/restore the tab + pane layout to disk so a complex
// workspace survives restarts. Geometry, names, and colors are restored; each
// pane re-spawns a fresh shell (we don't resurrect running processes).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::layout::Node;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPane {
    pub id: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub name: String,
    pub color: u32,
    pub focus: usize,
    pub next_id: usize,
    pub tree: Node,
    pub panes: Vec<SavedPane>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

impl SavedSession {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

/// `%APPDATA%\gritty\session.json` (falls back to the working dir).
pub fn session_path() -> PathBuf {
    let mut dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.push("gritty");
    dir.push("session.json");
    dir
}

pub fn save(session: &SavedSession) -> std::io::Result<()> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, session.to_json())
}

pub fn load() -> Option<SavedSession> {
    let text = std::fs::read_to_string(session_path()).ok()?;
    SavedSession::from_json(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Node};

    fn sample() -> SavedSession {
        SavedSession {
            active: 1,
            tabs: vec![
                SavedTab {
                    name: "tab 1".into(),
                    color: 0x00ff_3d9a,
                    focus: 1,
                    next_id: 2,
                    tree: Node::Split {
                        axis: Axis::LeftRight,
                        ratio: 0.4,
                        a: Box::new(Node::Leaf(0)),
                        b: Box::new(Node::Leaf(1)),
                    },
                    panes: vec![
                        SavedPane {
                            id: 0,
                            name: "editor".into(),
                        },
                        SavedPane {
                            id: 1,
                            name: "logs".into(),
                        },
                    ],
                },
                SavedTab {
                    name: "tab 2".into(),
                    color: 0x003d_f0ff,
                    focus: 0,
                    next_id: 1,
                    tree: Node::Leaf(0),
                    panes: vec![SavedPane {
                        id: 0,
                        name: "term 1".into(),
                    }],
                },
            ],
        }
    }

    #[test]
    fn json_roundtrip_is_identity() {
        let s = sample();
        let json = s.to_json();
        let back = SavedSession::from_json(&json).expect("parse");
        assert_eq!(s, back);
    }

    #[test]
    fn garbage_json_is_none() {
        assert!(SavedSession::from_json("not json").is_none());
    }

    #[test]
    fn path_ends_with_expected_file() {
        let p = session_path();
        assert!(p.ends_with("gritty/session.json") || p.ends_with("gritty\\session.json"));
    }
}
