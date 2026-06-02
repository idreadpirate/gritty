// Encode winit key events into the byte sequences a PTY expects.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Returns the bytes to send to the PTY for a key press, if any.
pub fn encode(key: &Key, mods: ModifiersState) -> Option<Vec<u8>> {
    let ctrl = mods.control_key();
    match key {
        Key::Named(named) => named_bytes(*named),
        Key::Character(s) => {
            if ctrl {
                let c = s.chars().next()?;
                if c.is_ascii_alphabetic() {
                    // Ctrl-A..Ctrl-Z -> 0x01..0x1a
                    Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f])
                } else {
                    Some(s.as_bytes().to_vec())
                }
            } else {
                Some(s.as_bytes().to_vec())
            }
        }
        _ => None,
    }
}

fn named_bytes(named: NamedKey) -> Option<Vec<u8>> {
    let b: &[u8] = match named {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => &[0x7f],
        NamedKey::Tab => b"\t",
        NamedKey::Escape => &[0x1b],
        NamedKey::Space => b" ",
        NamedKey::ArrowUp => b"\x1b[A",
        NamedKey::ArrowDown => b"\x1b[B",
        NamedKey::ArrowRight => b"\x1b[C",
        NamedKey::ArrowLeft => b"\x1b[D",
        NamedKey::Home => b"\x1b[H",
        NamedKey::End => b"\x1b[F",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        NamedKey::Delete => b"\x1b[3~",
        NamedKey::Insert => b"\x1b[2~",
        _ => return None,
    };
    Some(b.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
