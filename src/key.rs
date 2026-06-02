// Encode winit key events into the byte sequences a PTY expects, including
// xterm modifier encoding (CA-6): function keys, modified arrows, Alt-as-ESC,
// and Ctrl-masking over the full ASCII range.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Returns the bytes to send to the PTY for a key press, if any.
pub fn encode(key: &Key, mods: ModifiersState) -> Option<Vec<u8>> {
    let ctrl = mods.control_key();
    let alt = mods.alt_key();
    let shift = mods.shift_key();
    let m = modifier_code(shift, alt, ctrl);

    match key {
        Key::Named(n) => named_bytes(*n, m, shift),
        Key::Character(s) => char_bytes(s, ctrl, alt),
        _ => None,
    }
}

/// xterm modifier parameter: 1 = none, then +shift/+alt(×2)/+ctrl(×4).
fn modifier_code(shift: bool, alt: bool, ctrl: bool) -> u8 {
    1 + (shift as u8) + ((alt as u8) << 1) + ((ctrl as u8) << 2)
}

/// CSI sequence for cursor-style keys (final byte A/B/C/D/H/F).
fn csi(m: u8, final_byte: u8) -> Vec<u8> {
    if m <= 1 {
        vec![0x1b, b'[', final_byte]
    } else {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    }
}

/// CSI `~` sequence for editing/function keys (PageUp = 5, F5 = 15, ...).
fn csi_tilde(m: u8, num: u8) -> Vec<u8> {
    if m <= 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{m}~").into_bytes()
    }
}

/// SS3 sequence for F1-F4 unmodified; CSI with modifier otherwise.
fn ss3(m: u8, final_byte: u8) -> Vec<u8> {
    if m <= 1 {
        vec![0x1b, b'O', final_byte]
    } else {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    }
}

fn named_bytes(n: NamedKey, m: u8, shift: bool) -> Option<Vec<u8>> {
    use NamedKey::*;
    let bytes = match n {
        Enter => b"\r".to_vec(),
        Backspace => vec![0x7f],
        Tab => {
            if shift {
                b"\x1b[Z".to_vec() // back-tab
            } else {
                b"\t".to_vec()
            }
        }
        Escape => vec![0x1b],
        Space => b" ".to_vec(),
        ArrowUp => csi(m, b'A'),
        ArrowDown => csi(m, b'B'),
        ArrowRight => csi(m, b'C'),
        ArrowLeft => csi(m, b'D'),
        Home => csi(m, b'H'),
        End => csi(m, b'F'),
        PageUp => csi_tilde(m, 5),
        PageDown => csi_tilde(m, 6),
        Delete => csi_tilde(m, 3),
        Insert => csi_tilde(m, 2),
        F1 => ss3(m, b'P'),
        F2 => ss3(m, b'Q'),
        F3 => ss3(m, b'R'),
        F4 => ss3(m, b'S'),
        F5 => csi_tilde(m, 15),
        F6 => csi_tilde(m, 17),
        F7 => csi_tilde(m, 18),
        F8 => csi_tilde(m, 19),
        F9 => csi_tilde(m, 20),
        F10 => csi_tilde(m, 21),
        F11 => csi_tilde(m, 23),
        F12 => csi_tilde(m, 24),
        _ => return None,
    };
    Some(bytes)
}

fn char_bytes(s: &str, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if alt {
        out.push(0x1b); // Meta = ESC prefix (Alt+b, Alt+. word motions)
    }
    if ctrl {
        out.push(ctrl_byte(s.chars().next()?)?);
    } else {
        out.extend_from_slice(s.as_bytes());
    }
    Some(out)
}

/// Map a character under Ctrl to its control byte (Ctrl+A=0x01 … Ctrl+_=0x1f).
fn ctrl_byte(c: char) -> Option<u8> {
    let b = match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        ' ' | '@' => 0x00,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        '?' => 0x7f,
        _ if (c as u32) < 128 => c as u8, // other ASCII: pass through
        _ => return None,
    };
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, ctrl: bool) -> ModifiersState {
        let mut m = ModifiersState::empty();
        if shift {
            m |= ModifiersState::SHIFT;
        }
        if alt {
            m |= ModifiersState::ALT;
        }
        if ctrl {
            m |= ModifiersState::CONTROL;
        }
        m
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(
            encode(&Key::Named(NamedKey::Enter), ModifiersState::empty()),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn ctrl_c_is_etx() {
        let key = Key::Character("c".into());
        assert_eq!(encode(&key, ModifiersState::CONTROL), Some(vec![0x03]));
    }

    #[test]
    fn plain_letter_passthrough() {
        let key = Key::Character("a".into());
        assert_eq!(encode(&key, ModifiersState::empty()), Some(b"a".to_vec()));
    }

    #[test]
    fn alt_letter_is_esc_prefixed() {
        let key = Key::Character("b".into());
        assert_eq!(
            encode(&key, mods(false, true, false)),
            Some(b"\x1bb".to_vec())
        );
    }

    #[test]
    fn ctrl_left_bracket_is_escape() {
        let key = Key::Character("[".into());
        assert_eq!(encode(&key, ModifiersState::CONTROL), Some(vec![0x1b]));
    }

    #[test]
    fn unmodified_arrow_is_legacy() {
        assert_eq!(
            encode(&Key::Named(NamedKey::ArrowRight), ModifiersState::empty()),
            Some(b"\x1b[C".to_vec())
        );
    }

    #[test]
    fn ctrl_arrow_is_word_jump() {
        // Ctrl => modifier code 5 => CSI 1;5C
        assert_eq!(
            encode(&Key::Named(NamedKey::ArrowRight), ModifiersState::CONTROL),
            Some(b"\x1b[1;5C".to_vec())
        );
    }

    #[test]
    fn f5_and_f1() {
        assert_eq!(
            encode(&Key::Named(NamedKey::F5), ModifiersState::empty()),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            encode(&Key::Named(NamedKey::F1), ModifiersState::empty()),
            Some(b"\x1bOP".to_vec())
        );
    }

    #[test]
    fn shift_tab_is_backtab() {
        assert_eq!(
            encode(&Key::Named(NamedKey::Tab), mods(true, false, false)),
            Some(b"\x1b[Z".to_vec())
        );
    }
}
