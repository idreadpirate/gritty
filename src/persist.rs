// Session persistence: save/restore the tab + pane layout to disk so a complex
// workspace survives restarts. Geometry, names, and colors are restored; each
// pane re-spawns a fresh shell (we don't resurrect running processes).

use std::path::PathBuf;

use crate::layout::{Axis, Node};

#[derive(Debug, Clone, PartialEq)]
pub struct SavedPane {
    pub id: usize,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
pub struct SavedWindow {
    pub active: usize,
    pub tabs: Vec<SavedTab>,
    /// Window size in physical pixels (None = use default).
    pub win_w: Option<u32>,
    pub win_h: Option<u32>,
    /// Top-left window position in physical pixels (None = let the OS place it).
    pub win_x: Option<i32>,
    pub win_y: Option<i32>,
    /// Seamless mode (no per-pane title bars). CA-57: previously not persisted, so
    /// a window saved in seamless mode came back with title bars on the next launch.
    /// A missing `seamless` key keeps pre-CA-57 session files loading (as `false`).
    pub seamless: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedSession {
    /// Multi-window workspace (one entry per OS window). Preferred form.
    pub windows: Vec<SavedWindow>,

    // --- Legacy single-window fields (pre-multi-window sessions) -------------
    // Kept so old `session.json` files still load. `windows()` folds these into
    // a single window when `windows` is empty.
    pub active: usize,
    pub tabs: Vec<SavedTab>,
    pub win_w: Option<u32>,
    pub win_h: Option<u32>,
}

impl SavedSession {
    pub fn to_json(&self) -> String {
        // Byte-compatible with the previous `serde_json::to_string_pretty`
        // output: 2-space indent, struct-declaration field order, every field
        // emitted (Options as their value or `null`). A non-finite ratio makes
        // serialization fail just as serde_json did, falling back to "{}".
        let mut w = JsonWriter::new();
        if self.write_to(&mut w).is_err() {
            return "{}".into();
        }
        w.finish()
    }

    fn write_to(&self, w: &mut JsonWriter) -> Result<(), ()> {
        w.begin_object();
        w.key("windows");
        w.array(&self.windows, |w, win| win.write_to(w))?;
        w.key("active");
        w.usize(self.active);
        w.key("tabs");
        w.array(&self.tabs, |w, t| t.write_to(w))?;
        w.key("win_w");
        w.opt_u32(self.win_w);
        w.key("win_h");
        w.opt_u32(self.win_h);
        w.end_object();
        Ok(())
    }

    pub fn from_json(s: &str) -> Option<Self> {
        let v = JsonValue::parse(s)?;
        let obj = v.as_object()?;
        Some(SavedSession {
            windows: obj.array_of("windows", SavedWindow::from_value)?,
            active: obj.usize_or("active", 0)?,
            tabs: obj.array_of("tabs", SavedTab::from_value)?,
            win_w: obj.opt_u32("win_w")?,
            win_h: obj.opt_u32("win_h")?,
        })
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

impl SavedWindow {
    fn write_to(&self, w: &mut JsonWriter) -> Result<(), ()> {
        w.begin_object();
        w.key("active");
        w.usize(self.active);
        w.key("tabs");
        w.array(&self.tabs, |w, t| t.write_to(w))?;
        w.key("win_w");
        w.opt_u32(self.win_w);
        w.key("win_h");
        w.opt_u32(self.win_h);
        w.key("win_x");
        w.opt_i32(self.win_x);
        w.key("win_y");
        w.opt_i32(self.win_y);
        w.key("seamless");
        w.bool(self.seamless);
        w.end_object();
        Ok(())
    }

    fn from_value(v: &JsonValue) -> Option<Self> {
        let obj = v.as_object()?;
        Some(SavedWindow {
            active: obj.usize_or("active", 0)?,
            tabs: obj.array_of("tabs", SavedTab::from_value)?,
            win_w: obj.opt_u32("win_w")?,
            win_h: obj.opt_u32("win_h")?,
            win_x: obj.opt_i32("win_x")?,
            win_y: obj.opt_i32("win_y")?,
            seamless: obj.bool_or("seamless", false)?,
        })
    }
}

impl SavedTab {
    fn write_to(&self, w: &mut JsonWriter) -> Result<(), ()> {
        w.begin_object();
        w.key("name");
        w.string(&self.name);
        w.key("color");
        w.u32(self.color);
        w.key("focus");
        w.usize(self.focus);
        w.key("next_id");
        w.usize(self.next_id);
        w.key("tree");
        write_node(w, &self.tree)?;
        w.key("panes");
        w.array(&self.panes, |w, p| {
            p.write_to(w);
            Ok(())
        })?;
        w.end_object();
        Ok(())
    }

    fn from_value(v: &JsonValue) -> Option<Self> {
        let obj = v.as_object()?;
        Some(SavedTab {
            name: obj.string_req("name")?,
            color: obj.u32_req("color")?,
            focus: obj.usize_req("focus")?,
            next_id: obj.usize_req("next_id")?,
            tree: node_from_value(obj.get("tree")?)?,
            panes: obj.array_of("panes", SavedPane::from_value)?,
        })
    }
}

impl SavedPane {
    fn write_to(&self, w: &mut JsonWriter) {
        w.begin_object();
        w.key("id");
        w.usize(self.id);
        w.key("name");
        w.string(&self.name);
        w.end_object();
    }

    fn from_value(v: &JsonValue) -> Option<Self> {
        let obj = v.as_object()?;
        Some(SavedPane {
            id: obj.usize_req("id")?,
            name: obj.string_req("name")?,
        })
    }
}

// --- layout::Node (de)serialization --------------------------------------
// Mirrors serde's externally-tagged enum form: `Node::Leaf(0)` -> `{"Leaf": 0}`,
// `Node::Split { .. }` -> `{"Split": { "axis": .., "ratio": .., "a": .., "b": .. }}`.
// `Axis` is a unit-variant enum: `Axis::LeftRight` -> `"LeftRight"`.

fn write_node(w: &mut JsonWriter, n: &Node) -> Result<(), ()> {
    w.begin_object();
    match n {
        Node::Leaf(id) => {
            w.key("Leaf");
            w.usize(*id);
        }
        Node::Split { axis, ratio, a, b } => {
            w.key("Split");
            w.begin_object();
            w.key("axis");
            w.string_raw(axis_str(*axis));
            w.key("ratio");
            w.f32(*ratio)?;
            w.key("a");
            write_node(w, a)?;
            w.key("b");
            write_node(w, b)?;
            w.end_object();
        }
    }
    w.end_object();
    Ok(())
}

fn axis_str(a: Axis) -> &'static str {
    match a {
        Axis::LeftRight => "LeftRight",
        Axis::TopBottom => "TopBottom",
    }
}

fn axis_from_str(s: &str) -> Option<Axis> {
    match s {
        "LeftRight" => Some(Axis::LeftRight),
        "TopBottom" => Some(Axis::TopBottom),
        _ => None,
    }
}

fn node_from_value(v: &JsonValue) -> Option<Node> {
    let obj = v.as_object()?;
    if let Some(leaf) = obj.get("Leaf") {
        return Some(Node::Leaf(leaf.as_usize()?));
    }
    let split = obj.get("Split")?.as_object()?;
    Some(Node::Split {
        axis: axis_from_str(split.get("axis")?.as_str()?)?,
        ratio: split.get("ratio")?.as_f32()?,
        a: Box::new(node_from_value(split.get("a")?)?),
        b: Box::new(node_from_value(split.get("b")?)?),
    })
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

// =========================================================================
// Minimal pretty-JSON writer
// =========================================================================
// Emits exactly what `serde_json::to_string_pretty` produced for these types:
// 2-space indentation, one field/element per line, `": "` after keys, a
// trailing newline-free document, empty collections as `[]`. The only failure
// is a non-finite float (`f32`), which serde_json also refused to serialize.

struct JsonWriter {
    out: String,
    depth: usize,
    /// True until the first member of the current container is written, so we
    /// know whether to prefix the next member with a comma.
    first: Vec<bool>,
}

impl JsonWriter {
    fn new() -> Self {
        JsonWriter {
            out: String::new(),
            depth: 0,
            first: Vec::new(),
        }
    }

    fn finish(self) -> String {
        self.out
    }

    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    /// Open a new line for the next member, emitting a comma after the previous
    /// one. serde_json's pretty printer puts each member on its own indented
    /// line; the opening `{`/`[` is followed by a newline only when non-empty.
    fn member_sep(&mut self) {
        if let Some(first) = self.first.last_mut() {
            if *first {
                *first = false;
            } else {
                self.out.push(',');
            }
            self.out.push('\n');
            self.indent();
        }
    }

    fn begin_object(&mut self) {
        self.out.push('{');
        self.depth += 1;
        self.first.push(true);
    }

    fn end_object(&mut self) {
        let was_empty = self.first.pop().unwrap_or(true);
        self.depth -= 1;
        if !was_empty {
            self.out.push('\n');
            self.indent();
        }
        self.out.push('}');
    }

    fn key(&mut self, k: &str) {
        self.member_sep();
        escape_into(&mut self.out, k);
        self.out.push_str(": ");
    }

    fn array<T>(
        &mut self,
        items: &[T],
        mut each: impl FnMut(&mut JsonWriter, &T) -> Result<(), ()>,
    ) -> Result<(), ()> {
        self.out.push('[');
        if items.is_empty() {
            self.out.push(']');
            return Ok(());
        }
        self.depth += 1;
        self.first.push(true);
        for it in items {
            self.member_sep();
            each(self, it)?;
        }
        self.first.pop();
        self.depth -= 1;
        self.out.push('\n');
        self.indent();
        self.out.push(']');
        Ok(())
    }

    fn string(&mut self, s: &str) {
        escape_into(&mut self.out, s);
    }

    /// A value already known to need no escaping (enum variant tags).
    fn string_raw(&mut self, s: &str) {
        self.out.push('"');
        self.out.push_str(s);
        self.out.push('"');
    }

    fn usize(&mut self, v: usize) {
        self.out.push_str(itoa_usize(v).as_str());
    }

    fn u32(&mut self, v: u32) {
        self.out.push_str(itoa_usize(v as usize).as_str());
    }

    fn opt_u32(&mut self, v: Option<u32>) {
        match v {
            Some(v) => self.u32(v),
            None => self.out.push_str("null"),
        }
    }

    fn opt_i32(&mut self, v: Option<i32>) {
        match v {
            Some(v) => self.out.push_str(&format!("{v}")),
            None => self.out.push_str("null"),
        }
    }

    fn bool(&mut self, v: bool) {
        self.out.push_str(if v { "true" } else { "false" });
    }

    /// Format an `f32` exactly as serde_json did: the shortest decimal that
    /// round-trips (Rust's `{}`), with a `.0` appended when the value is
    /// integer-valued. Non-finite values cannot be represented and fail the
    /// whole serialization (matching serde_json's `Err`).
    fn f32(&mut self, v: f32) -> Result<(), ()> {
        if !v.is_finite() {
            return Err(());
        }
        let s = format!("{v}");
        self.out.push_str(&s);
        // serde_json renders integral floats with a trailing `.0` (e.g. `0.0`),
        // whereas `{}` prints `0`; add it back when no fraction/exponent shows.
        if !s.contains(['.', 'e', 'E']) {
            self.out.push_str(".0");
        }
        Ok(())
    }
}

/// Stack-buffer integer formatting without pulling in a formatting allocation
/// per call; `usize` decimal is at most 20 digits.
struct ItoaBuf {
    buf: [u8; 20],
    start: usize,
}
impl ItoaBuf {
    fn as_str(&self) -> &str {
        // SAFETY substitute: buffer holds only ASCII digits we wrote.
        std::str::from_utf8(&self.buf[self.start..]).unwrap_or("0")
    }
}
fn itoa_usize(mut v: usize) -> ItoaBuf {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    ItoaBuf { buf, start: i }
}

/// Escape a string into `out` exactly as serde_json does: wrap in quotes,
/// backslash-escape `"` and `\`, map `\b \f \n \r \t`, and `\u00XX` any other
/// control char (< 0x20). All other bytes (incl. non-ASCII UTF-8 and `/`) pass
/// through unchanged.
fn escape_into(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// =========================================================================
// Minimal JSON value parser
// =========================================================================
// A tolerant recursive-descent parser sufficient for session files: objects,
// arrays, strings (with the standard escapes incl. `\uXXXX`), numbers, the
// literals `true`/`false`/`null`. It accepts both the pretty form we emit and
// the compact legacy form, and any key order — matching what serde_json read.

#[derive(Debug, Clone, PartialEq)]
enum JsonValue {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsonValue>),
    Obj(Vec<(String, JsonValue)>),
}

impl JsonValue {
    fn parse(s: &str) -> Option<JsonValue> {
        let bytes = s.as_bytes();
        let mut p = Parser { b: bytes, i: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        // Trailing non-whitespace means malformed input (serde_json rejected it).
        if p.i != bytes.len() {
            return None;
        }
        Some(v)
    }

    fn as_object(&self) -> Option<Obj<'_>> {
        match self {
            JsonValue::Obj(m) => Some(Obj { entries: m }),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_usize(&self) -> Option<usize> {
        match self {
            JsonValue::Num(n) => num_to_usize(*n),
            _ => None,
        }
    }

    fn as_u32(&self) -> Option<u32> {
        match self {
            JsonValue::Num(n) => num_to_u32(*n),
            _ => None,
        }
    }

    fn as_i32(&self) -> Option<i32> {
        match self {
            JsonValue::Num(n) => num_to_i32(*n),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_f32(&self) -> Option<f32> {
        match self {
            JsonValue::Num(n) => Some(*n as f32),
            _ => None,
        }
    }
}

fn num_to_usize(n: f64) -> Option<usize> {
    if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= usize::MAX as f64 {
        Some(n as usize)
    } else {
        None
    }
}
fn num_to_u32(n: f64) -> Option<u32> {
    if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= u32::MAX as f64 {
        Some(n as u32)
    } else {
        None
    }
}
fn num_to_i32(n: f64) -> Option<i32> {
    if n.is_finite() && n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        Some(n as i32)
    } else {
        None
    }
}

/// Borrowed view over a parsed JSON object's entries, with the typed accessors
/// the (de)serializers need. `_req` requires a present, well-typed key; the
/// `_or` / `opt_` accessors model serde's `#[serde(default)]` (a missing key
/// takes the default, a present-but-wrong-typed key is an error -> None).
struct Obj<'a> {
    entries: &'a [(String, JsonValue)],
}

impl<'a> Obj<'a> {
    fn get(&self, key: &str) -> Option<&'a JsonValue> {
        // JSON (RFC 8259) leaves duplicate keys implementation-defined; the de
        // facto convention (serde_json, JS) is last-value-wins, so scan in
        // reverse to honor the final occurrence of a duplicated key.
        self.entries.iter().rfind(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn array_of<T>(&self, key: &str, each: impl Fn(&JsonValue) -> Option<T>) -> Option<Vec<T>> {
        match self.get(key) {
            None => Some(Vec::new()),
            Some(JsonValue::Arr(items)) => items.iter().map(each).collect(),
            Some(_) => None,
        }
    }

    fn usize_or(&self, key: &str, default: usize) -> Option<usize> {
        match self.get(key) {
            None => Some(default),
            Some(v) => v.as_usize(),
        }
    }

    fn usize_req(&self, key: &str) -> Option<usize> {
        self.get(key)?.as_usize()
    }

    fn u32_req(&self, key: &str) -> Option<u32> {
        self.get(key)?.as_u32()
    }

    fn string_req(&self, key: &str) -> Option<String> {
        Some(self.get(key)?.as_str()?.to_string())
    }

    fn bool_or(&self, key: &str, default: bool) -> Option<bool> {
        match self.get(key) {
            None => Some(default),
            Some(v) => v.as_bool(),
        }
    }

    fn opt_u32(&self, key: &str) -> Option<Option<u32>> {
        match self.get(key) {
            None | Some(JsonValue::Null) => Some(None),
            Some(v) => Some(Some(v.as_u32()?)),
        }
    }

    fn opt_i32(&self, key: &str) -> Option<Option<i32>> {
        match self.get(key) {
            None | Some(JsonValue::Null) => Some(None),
            Some(v) => Some(Some(v.as_i32()?)),
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn value(&mut self) -> Option<JsonValue> {
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(JsonValue::Str),
            b't' | b'f' => self.boolean(),
            b'n' => self.null(),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn object(&mut self) -> Option<JsonValue> {
        self.i += 1; // '{'
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek()? == b'}' {
            self.i += 1;
            return Some(JsonValue::Obj(entries));
        }
        loop {
            self.skip_ws();
            if self.peek()? != b'"' {
                return None;
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek()? != b':' {
                return None;
            }
            self.i += 1;
            self.skip_ws();
            let val = self.value()?;
            entries.push((key, val));
            self.skip_ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                }
                b'}' => {
                    self.i += 1;
                    return Some(JsonValue::Obj(entries));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<JsonValue> {
        self.i += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek()? == b']' {
            self.i += 1;
            return Some(JsonValue::Arr(items));
        }
        loop {
            self.skip_ws();
            let v = self.value()?;
            items.push(v);
            self.skip_ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                }
                b']' => {
                    self.i += 1;
                    return Some(JsonValue::Arr(items));
                }
                _ => return None,
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // opening '"'
        let mut s = String::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(s),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{08}'),
                        b'f' => s.push('\u{0c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            // Surrogate pair handling for code points > U+FFFF.
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.b.get(self.i) != Some(&b'\\')
                                    || self.b.get(self.i + 1) != Some(&b'u')
                                {
                                    return None;
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&lo) {
                                    return None;
                                }
                                let c =
                                    0x10000 + (((cp - 0xD800) as u32) << 10) + (lo - 0xDC00) as u32;
                                s.push(char::from_u32(c)?);
                            } else {
                                s.push(char::from_u32(cp as u32)?);
                            }
                        }
                        _ => return None,
                    }
                }
                // A raw control byte inside a string is invalid JSON.
                0x00..=0x1F => return None,
                // Continue a multi-byte UTF-8 sequence by collecting its bytes.
                _ => {
                    let start = self.i - 1;
                    let len = utf8_len(c);
                    let end = start + len;
                    if end > self.b.len() {
                        return None;
                    }
                    // The leading byte plus its continuation bytes form one char.
                    let chunk = std::str::from_utf8(&self.b[start..end]).ok()?;
                    s.push_str(chunk);
                    self.i = end;
                }
            }
        }
    }

    fn hex4(&mut self) -> Option<u16> {
        let mut v: u16 = 0;
        for _ in 0..4 {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u16,
                b'a'..=b'f' => (c - b'a' + 10) as u16,
                b'A'..=b'F' => (c - b'A' + 10) as u16,
                _ => return None,
            };
            v = v * 16 + d;
        }
        Some(v)
    }

    fn boolean(&mut self) -> Option<JsonValue> {
        if self.b[self.i..].starts_with(b"true") {
            self.i += 4;
            Some(JsonValue::Bool(true))
        } else if self.b[self.i..].starts_with(b"false") {
            self.i += 5;
            Some(JsonValue::Bool(false))
        } else {
            None
        }
    }

    fn null(&mut self) -> Option<JsonValue> {
        if self.b[self.i..].starts_with(b"null") {
            self.i += 4;
            Some(JsonValue::Null)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<JsonValue> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => self.i += 1,
                _ => break,
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).ok()?;
        text.parse::<f64>().ok().map(JsonValue::Num)
    }
}

/// UTF-8 sequence length from a leading byte.
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn duplicate_object_keys_use_last_value() {
        // RFC 8259 leaves duplicates implementation-defined; the de facto
        // convention (serde_json, JS) is last-value-wins. A hand-edited or
        // duplicated session must restore the final `active`, not the first.
        let tampered = r#"{"active":99,"active":0,"tabs":[]}"#;
        let s = SavedSession::from_json(tampered).expect("parse tampered session");
        assert_eq!(s.active, 0, "last duplicate key must win");
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
