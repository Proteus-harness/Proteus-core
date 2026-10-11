//! Keystroke to terminal input bytes (xterm conventions).

/// A key press as the UI reports it: `key` is the gpui key name, `text` the
/// character it types, if any.
#[derive(Debug, Clone, Copy, Default)]
pub struct Key<'a> {
    pub key: &'a str,
    pub text: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Bytes for `key`, or `None` when the terminal has no input for it.
pub fn encode(key: Key<'_>, app_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = 1 + key.shift as u8 + 2 * key.alt as u8 + 4 * key.ctrl as u8;
    let modified = modifiers > 1;
    let csi = |code: &str, end: char| -> Vec<u8> {
        if modified {
            format!("\x1b[{code};{modifiers}{end}").into_bytes()
        } else {
            format!("\x1b[{code}{end}").into_bytes()
        }
    };
    let cursor = |end: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{modifiers}{end}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let bytes = match key.key {
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => csi("2", '~'),
        "delete" => csi("3", '~'),
        "pageup" => csi("5", '~'),
        "pagedown" => csi("6", '~'),
        "f1" => function('P', modifiers),
        "f2" => function('Q', modifiers),
        "f3" => function('R', modifiers),
        "f4" => function('S', modifiers),
        "f5" => csi("15", '~'),
        "f6" => csi("17", '~'),
        "f7" => csi("18", '~'),
        "f8" => csi("19", '~'),
        "f9" => csi("20", '~'),
        "f10" => csi("21", '~'),
        "f11" => csi("23", '~'),
        "f12" => csi("24", '~'),
        "enter" => alt(key.alt, b"\r"),
        "escape" => alt(key.alt, b"\x1b"),
        "tab" if key.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(key.alt, b"\t"),
        "backspace" if key.ctrl => alt(key.alt, b"\x08"),
        "backspace" => alt(key.alt, b"\x7f"),
        _ if key.ctrl => alt(key.alt, &[control(key.key)?]),
        "space" => alt(key.alt, b" "),
        _ => alt(key.alt, key.text?.as_bytes()),
    };
    Some(bytes)
}

fn function(end: char, modifiers: u8) -> Vec<u8> {
    if modifiers > 1 {
        format!("\x1b[1;{modifiers}{end}").into_bytes()
    } else {
        format!("\x1bO{end}").into_bytes()
    }
}

fn alt(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn control(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return match key {
            "space" => Some(0),
            _ => None,
        };
    };
    match ch.to_ascii_lowercase() {
        ch @ 'a'..='z' => Some(ch as u8 - b'a' + 1),
        '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Key<'_> {
        Key {
            key: name,
            ..Key::default()
        }
    }

    #[test]
    fn text_and_controls() {
        let typed = Key {
            text: Some("ы"),
            ..key("ы")
        };
        assert_eq!(encode(typed, false).unwrap(), "ы".as_bytes());
        let ctrl_c = Key {
            ctrl: true,
            ..key("c")
        };
        assert_eq!(encode(ctrl_c, false).unwrap(), b"\x03");
        let alt_b = Key {
            alt: true,
            text: Some("b"),
            ..key("b")
        };
        assert_eq!(encode(alt_b, false).unwrap(), b"\x1bb");
        assert_eq!(encode(key("enter"), false).unwrap(), b"\r");
        assert_eq!(encode(key("backspace"), false).unwrap(), b"\x7f");
        assert!(encode(key("shift"), false).is_none());
    }

    #[test]
    fn cursor_keys_follow_mode_and_modifiers() {
        assert_eq!(encode(key("up"), false).unwrap(), b"\x1b[A");
        assert_eq!(encode(key("up"), true).unwrap(), b"\x1bOA");
        let ctrl_left = Key {
            ctrl: true,
            ..key("left")
        };
        assert_eq!(encode(ctrl_left, true).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode(key("pageup"), false).unwrap(), b"\x1b[5~");
        assert_eq!(encode(key("f5"), false).unwrap(), b"\x1b[15~");
        let shift_tab = Key {
            shift: true,
            ..key("tab")
        };
        assert_eq!(encode(shift_tab, false).unwrap(), b"\x1b[Z");
    }
}
