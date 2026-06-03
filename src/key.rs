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
        Backspace => {
            // CA-121: route modifiers through Backspace so readline/zsh/pwsh
            // word-erase works. Alt+Backspace → ESC DEL (delete previous word);
            // Ctrl+Backspace → BS (0x08); plain Backspace → DEL (0x7f). `m` is the
            // xterm modifier code (1 + shift + 2·alt + 4·ctrl), so bit 1 = Alt and
            // bit 2 = Ctrl.
            let alt = (m.saturating_sub(1)) & 0b010 != 0;
            let ctrl = (m.saturating_sub(1)) & 0b100 != 0;
            if alt {
                vec![0x1b, 0x7f]
            } else if ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
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

    #[test]
    fn plain_tab_and_other_control_named_keys() {
        let none = ModifiersState::empty();
        assert_eq!(
            encode(&Key::Named(NamedKey::Tab), none),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            encode(&Key::Named(NamedKey::Backspace), none),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode(&Key::Named(NamedKey::Escape), none),
            Some(vec![0x1b])
        );
        assert_eq!(
            encode(&Key::Named(NamedKey::Space), none),
            Some(b" ".to_vec())
        );
    }

    #[test]
    fn alt_backspace_deletes_previous_word() {
        // CA-121: Alt+Backspace → ESC DEL (readline/zsh/pwsh word-erase).
        assert_eq!(
            encode(&Key::Named(NamedKey::Backspace), mods(false, true, false)),
            Some(vec![0x1b, 0x7f])
        );
    }

    #[test]
    fn ctrl_backspace_is_bs() {
        // CA-121: Ctrl+Backspace → BS (0x08).
        assert_eq!(
            encode(&Key::Named(NamedKey::Backspace), ModifiersState::CONTROL),
            Some(vec![0x08])
        );
    }

    #[test]
    fn plain_and_shift_backspace_stay_del() {
        // Plain Backspace and Shift+Backspace both emit DEL (0x7f) — only
        // Alt/Ctrl change the byte (CA-121).
        assert_eq!(
            encode(&Key::Named(NamedKey::Backspace), ModifiersState::empty()),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode(&Key::Named(NamedKey::Backspace), mods(true, false, false)),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn all_unmodified_arrows_and_motion_keys() {
        let none = ModifiersState::empty();
        let cases = [
            (NamedKey::ArrowUp, &b"\x1b[A"[..]),
            (NamedKey::ArrowDown, b"\x1b[B"),
            (NamedKey::ArrowLeft, b"\x1b[D"),
            (NamedKey::Home, b"\x1b[H"),
            (NamedKey::End, b"\x1b[F"),
            (NamedKey::PageUp, b"\x1b[5~"),
            (NamedKey::PageDown, b"\x1b[6~"),
            (NamedKey::Delete, b"\x1b[3~"),
            (NamedKey::Insert, b"\x1b[2~"),
        ];
        for (k, expect) in cases {
            assert_eq!(encode(&Key::Named(k), none), Some(expect.to_vec()), "{k:?}");
        }
    }

    #[test]
    fn function_keys_f1_through_f12() {
        let none = ModifiersState::empty();
        let cases = [
            (NamedKey::F1, &b"\x1bOP"[..]),
            (NamedKey::F2, b"\x1bOQ"),
            (NamedKey::F3, b"\x1bOR"),
            (NamedKey::F4, b"\x1bOS"),
            (NamedKey::F5, b"\x1b[15~"),
            (NamedKey::F6, b"\x1b[17~"),
            (NamedKey::F7, b"\x1b[18~"),
            (NamedKey::F8, b"\x1b[19~"),
            (NamedKey::F9, b"\x1b[20~"),
            (NamedKey::F10, b"\x1b[21~"),
            (NamedKey::F11, b"\x1b[23~"),
            (NamedKey::F12, b"\x1b[24~"),
        ];
        for (k, expect) in cases {
            assert_eq!(encode(&Key::Named(k), none), Some(expect.to_vec()), "{k:?}");
        }
    }

    #[test]
    fn modified_editing_key_gets_modifier_param() {
        // Shift+PageUp -> CSI 5 ; 2 ~  (shift => modifier code 2)
        assert_eq!(
            encode(&Key::Named(NamedKey::PageUp), mods(true, false, false)),
            Some(b"\x1b[5;2~".to_vec())
        );
        // Ctrl+F1 -> CSI 1 ; 5 P (modified F1 leaves SS3 for CSI)
        assert_eq!(
            encode(&Key::Named(NamedKey::F1), ModifiersState::CONTROL),
            Some(b"\x1b[1;5P".to_vec())
        );
    }

    #[test]
    fn unhandled_named_key_is_none() {
        assert_eq!(
            encode(&Key::Named(NamedKey::F13), ModifiersState::empty()),
            None
        );
    }

    #[test]
    fn ctrl_alt_letter_is_esc_then_control_byte() {
        // Alt+Ctrl+c => ESC, then ETX
        let key = Key::Character("c".into());
        assert_eq!(
            encode(&key, mods(false, true, true)),
            Some(vec![0x1b, 0x03])
        );
    }

    #[test]
    fn ctrl_punctuation_control_bytes() {
        let ctrl = ModifiersState::CONTROL;
        let cases = [
            (" ", 0x00u8),
            ("@", 0x00),
            ("\\", 0x1c),
            ("]", 0x1d),
            ("^", 0x1e),
            ("_", 0x1f),
            ("?", 0x7f),
        ];
        for (s, b) in cases {
            assert_eq!(
                encode(&Key::Character(s.into()), ctrl),
                Some(vec![b]),
                "{s:?}"
            );
        }
        // uppercase letters map like their lowercase counterpart
        assert_eq!(encode(&Key::Character("A".into()), ctrl), Some(vec![0x01]));
    }

    #[test]
    fn ctrl_other_ascii_passes_through() {
        // '1' is ASCII but not a control mapping -> the byte itself.
        assert_eq!(
            encode(&Key::Character("1".into()), ModifiersState::CONTROL),
            Some(vec![b'1'])
        );
    }

    #[test]
    fn ctrl_non_ascii_char_is_none() {
        assert_eq!(
            encode(&Key::Character("é".into()), ModifiersState::CONTROL),
            None
        );
    }

    #[test]
    fn unprintable_key_variant_is_none() {
        // A dead key (e.g. accent composition) is neither Named nor Character
        // and must produce no PTY bytes.
        assert_eq!(encode(&Key::Dead(None), ModifiersState::empty()), None);
    }
}
