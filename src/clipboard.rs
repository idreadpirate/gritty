// System clipboard access via arboard. Degrades gracefully if unavailable.

use arboard::Clipboard;

pub struct Clip {
    inner: Option<Clipboard>,
}

impl Clip {
    pub fn new() -> Self {
        Self {
            inner: Clipboard::new().ok(),
        }
    }

    pub fn copy(&mut self, text: &str) {
        if let Some(cb) = self.inner.as_mut() {
            // arboard's set_text opens the system clipboard, which can block under
            // contention — mark it so the watchdog can name it if the UI wedges.
            crate::watchdog::mark(crate::watchdog::CLIPBOARD_COPY);
            let _ = cb.set_text(text.to_owned());
        }
    }

    pub fn paste(&mut self) -> Option<String> {
        crate::watchdog::mark(crate::watchdog::CLIPBOARD_PASTE);
        self.inner.as_mut()?.get_text().ok()
    }
}
