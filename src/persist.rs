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
    /// CA-32: persisted window width in physical pixels (None = use default).
    #[serde(default)]
    pub win_w: Option<u32>,
    /// CA-32: persisted window height in physical pixels (None = use default).
    #[serde(default)]
    pub win_h: Option<u32>,
}

impl SavedSession {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }
}

/// `%LOCALAPPDATA%\gritty\session.json` (then `%APPDATA%`, then the temp dir).
/// Never the current working directory — that would auto-load a planted session
/// when launched from an attacker-controlled folder (RT-13).
pub fn session_path() -> PathBuf {
    let mut dir = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
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

/// Largest session file we will parse. Guards against a crafted/corrupt file
/// causing a huge allocation or hang at startup (RT-1).
const MAX_SESSION_BYTES: u64 = 1_000_000;

pub fn load() -> Option<SavedSession> {
    load_from(&session_path())
}

pub fn load_from(path: &std::path::Path) -> Option<SavedSession> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_SESSION_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    SavedSession::from_json(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Axis, Node};

    fn sample() -> SavedSession {
        SavedSession {
            active: 1,
            win_w: None,
            win_h: None,
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
    fn valid_file_loads_and_oversize_file_rejected() {
        let dir = std::env::temp_dir();
        // Valid small file round-trips through load_from.
        let ok = dir.join(format!("gritty_test_ok_{}.json", std::process::id()));
        std::fs::write(&ok, sample().to_json()).unwrap();
        assert_eq!(load_from(&ok), Some(sample()));
        std::fs::remove_file(&ok).ok();

        // Oversize file is rejected before parsing.
        let big = dir.join(format!("gritty_test_big_{}.json", std::process::id()));
        std::fs::write(&big, vec![b'x'; (MAX_SESSION_BYTES + 1) as usize]).unwrap();
        assert!(load_from(&big).is_none());
        std::fs::remove_file(&big).ok();
    }

    #[test]
    fn path_ends_with_expected_file() {
        let p = session_path();
        assert!(p.ends_with("gritty/session.json") || p.ends_with("gritty\\session.json"));
    }

    // --- CA-32 window geometry persistence -----------------------------------

    #[test]
    fn win_geometry_roundtrip() {
        let mut s = sample();
        s.win_w = Some(1280);
        s.win_h = Some(800);
        let json = s.to_json();
        let back = SavedSession::from_json(&json).expect("parse");
        assert_eq!(back.win_w, Some(1280));
        assert_eq!(back.win_h, Some(800));
    }

    #[test]
    fn old_session_without_win_geometry_loads_as_none() {
        // Simulate a session file that predates CA-32 (no win_w/win_h fields).
        let old_json = r#"{"active":0,"tabs":[]}"#;
        let s = SavedSession::from_json(old_json).expect("parse old session");
        assert_eq!(s.win_w, None);
        assert_eq!(s.win_h, None);
    }
}
