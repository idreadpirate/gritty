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

/// One OS window's worth of workspace: its tabs, focused tab, and on-screen
/// geometry. A session is a list of these (tab tear-off creates multiple).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub active: usize,
    pub tabs: Vec<SavedTab>,
    /// Window size in physical pixels (None = use default).
    #[serde(default)]
    pub win_w: Option<u32>,
    #[serde(default)]
    pub win_h: Option<u32>,
    /// Top-left window position in physical pixels (None = let the OS place it).
    #[serde(default)]
    pub win_x: Option<i32>,
    #[serde(default)]
    pub win_y: Option<i32>,
    /// Seamless mode (no per-pane title bars). CA-57: previously not persisted, so
    /// a window saved in seamless mode came back with title bars on the next launch.
    /// `#[serde(default)]` keeps pre-CA-57 session files loading (as `false`).
    #[serde(default)]
    pub seamless: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    /// Multi-window workspace (one entry per OS window). Preferred form.
    #[serde(default)]
    pub windows: Vec<SavedWindow>,

    // --- Legacy single-window fields (pre-multi-window sessions) -------------
    // Kept so old `session.json` files still load. `windows()` folds these into
    // a single window when `windows` is empty.
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub win_w: Option<u32>,
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

    /// Build a multi-window session directly from a window list.
    pub fn from_windows(windows: Vec<SavedWindow>) -> Self {
        Self {
            windows,
            active: 0,
            tabs: Vec::new(),
            win_w: None,
            win_h: None,
        }
    }

    /// The windows to restore: the multi-window list when present, otherwise a
    /// single window synthesized from the legacy single-window fields. Returns
    /// empty when there is nothing to restore.
    pub fn windows(&self) -> Vec<SavedWindow> {
        if !self.windows.is_empty() {
            self.windows.clone()
        } else if !self.tabs.is_empty() {
            vec![SavedWindow {
                active: self.active,
                tabs: self.tabs.clone(),
                win_w: self.win_w,
                win_h: self.win_h,
                win_x: None,
                win_y: None,
                seamless: false,
            }]
        } else {
            Vec::new()
        }
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
    save_to(&session_path(), session)
}

/// Write `session` to `path` atomically: serialize to a sibling `.tmp` file then
/// rename it over the target. RT-18: `std::fs::write` straight onto session.json
/// leaves a truncated file if the process is killed / loses power mid-write, and
/// the next launch silently loses the whole workspace. A same-volume rename is
/// atomic on NTFS, so a partial write can never clobber the last good session.
pub fn save_to(path: &std::path::Path, session: &SavedSession) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, session.to_json())?;
    std::fs::rename(&tmp, path)
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
            windows: Vec::new(),
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
    fn save_to_is_atomic_and_roundtrips() {
        // RT-18: save_to writes via a temp file + rename. The target loads back
        // identically and no `.tmp` file is left behind after the rename.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("gritty_test_save_{}.json", std::process::id()));
        let tmp = path.with_extension("json.tmp");
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&tmp).ok();

        save_to(&path, &sample()).expect("save");
        assert_eq!(load_from(&path), Some(sample()));
        assert!(
            !tmp.exists(),
            "temp file must be renamed away, not left behind"
        );

        std::fs::remove_file(&path).ok();
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

    // --- Multi-window persistence --------------------------------------------

    fn win_sample(name: &str, x: i32, y: i32) -> SavedWindow {
        SavedWindow {
            active: 0,
            tabs: vec![SavedTab {
                name: name.into(),
                color: 0x00ff_7b00,
                focus: 0,
                next_id: 1,
                tree: Node::Leaf(0),
                panes: vec![SavedPane {
                    id: 0,
                    name: "term 1".into(),
                }],
            }],
            win_w: Some(960),
            win_h: Some(600),
            win_x: Some(x),
            win_y: Some(y),
            seamless: false,
        }
    }

    #[test]
    fn multi_window_roundtrip_preserves_position() {
        let s =
            SavedSession::from_windows(vec![win_sample("a", 10, 20), win_sample("b", 1930, 40)]);
        let back = SavedSession::from_json(&s.to_json()).expect("parse");
        assert_eq!(s, back);
        assert_eq!(back.windows.len(), 2);
        assert_eq!(back.windows[1].win_x, Some(1930));
        assert_eq!(back.windows[1].win_y, Some(40));
    }

    #[test]
    fn windows_prefers_multi_window_list() {
        let s = SavedSession::from_windows(vec![win_sample("a", 0, 0), win_sample("b", 100, 0)]);
        let ws = s.windows();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].tabs[0].name, "a");
    }

    #[test]
    fn windows_folds_legacy_single_window() {
        // A pre-multi-window session (only legacy `tabs`/`active`) becomes one window.
        let legacy = sample(); // has 2 tabs, active 1, no `windows`
        assert!(legacy.windows.is_empty());
        let ws = legacy.windows();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].active, 1);
        assert_eq!(ws[0].tabs.len(), 2);
        assert_eq!(ws[0].win_x, None); // legacy files carried no position
    }

    #[test]
    fn windows_empty_when_nothing_saved() {
        let empty = SavedSession::from_windows(Vec::new());
        assert!(empty.windows().is_empty());
    }

    #[test]
    fn legacy_json_without_windows_field_loads() {
        // Real old file shape: top-level active/tabs, no `windows` key.
        let old = r#"{"active":0,"tabs":[{"name":"t","color":0,"focus":0,"next_id":1,"tree":{"Leaf":0},"panes":[{"id":0,"name":"term 1"}]}]}"#;
        let s = SavedSession::from_json(old).expect("parse legacy");
        assert!(s.windows.is_empty());
        let ws = s.windows();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].tabs[0].name, "t");
    }

    #[test]
    fn seamless_persists_and_defaults_false() {
        // CA-57: seamless is per-window state; it must survive a save/restore.
        let mut w = win_sample("seamless", 0, 0);
        w.seamless = true;
        let s = SavedSession::from_windows(vec![w]);
        let back = SavedSession::from_json(&s.to_json()).expect("parse");
        assert!(back.windows[0].seamless, "seamless flag must round-trip");
        // A pre-CA-57 session.json has no `seamless` key → must default to false.
        let old = r#"{"windows":[{"active":0,"tabs":[]}]}"#;
        let s2 = SavedSession::from_json(old).expect("parse old");
        assert!(
            !s2.windows[0].seamless,
            "missing seamless must default to false"
        );
    }
}
