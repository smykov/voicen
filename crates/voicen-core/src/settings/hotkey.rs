//! Hotkey grammar (research R-6; spec 004 T030/T039; decision #23 N1).
//!
//! Canonical text: modifiers in the order `Ctrl`, `Alt`, `Shift`, `Win`, then one key
//! from the closed set [`HotkeyKey::all`], joined by `+` (`Ctrl+Alt+Space`). At least
//! one modifier, exactly one key, never `Esc` (reserved for cancel, FR-22). Whether a
//! valid hotkey can be registered is the registrar's question, not the grammar's.
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

use std::fmt;

/// The closed key set of research R-6 (82 keys). `Esc` is not in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Pause,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
}

impl HotkeyKey {
    /// Every key of the closed set, once.
    pub fn all() -> &'static [HotkeyKey] {
        todo!("T-003: HotkeyKey::all")
    }
}

/// A hotkey: modifiers + one key. `Display` writes the canonical text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: HotkeyKey,
}

impl fmt::Display for Hotkey {
    #[allow(unused_variables)] // STUB: body is todo!()
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("T-003: Hotkey Display (canonical format)")
    }
}

/// Why a hotkey string was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyError {
    /// No `Ctrl`, `Alt`, `Shift` or `Win` (code `hotkey.no_modifier`).
    NoModifier,
    /// Modifiers only (code `hotkey.no_key`).
    NoKey,
    /// `Esc` used as the key (code `hotkey.esc_reserved`).
    EscReserved,
    /// Anything else the grammar does not accept: unknown token, a second key, a
    /// repeated modifier, an empty part.
    Invalid,
}

#[allow(unused_variables)] // STUB: body is todo!()
pub fn parse_hotkey(s: &str) -> Result<Hotkey, HotkeyError> {
    todo!("T-003: parse_hotkey")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hk(ctrl: bool, alt: bool, shift: bool, win: bool, key: HotkeyKey) -> Hotkey {
        Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key,
        }
    }

    #[test]
    fn canonical_default_round_trips() {
        // Bite: parse or format of the FR-21 default hotkey broken.
        let parsed = parse_hotkey("Ctrl+Alt+Space").expect("default hotkey parses");
        assert_eq!(parsed, hk(true, true, false, false, HotkeyKey::Space));
        assert_eq!(parsed.to_string(), "Ctrl+Alt+Space");
    }

    #[test]
    fn format_orders_modifiers_ctrl_alt_shift_win() {
        // Bite: modifiers written in another order (two texts for one hotkey).
        let table = [
            (
                hk(true, true, true, true, HotkeyKey::F24),
                "Ctrl+Alt+Shift+Win+F24",
            ),
            (hk(false, false, true, true, HotkeyKey::A), "Shift+Win+A"),
            (hk(true, false, true, false, HotkeyKey::F9), "Ctrl+Shift+F9"),
            (hk(false, true, false, false, HotkeyKey::Digit5), "Alt+5"),
            (hk(false, false, false, true, HotkeyKey::Z), "Win+Z"),
            (
                hk(true, false, false, false, HotkeyKey::PageDown),
                "Ctrl+PageDown",
            ),
        ];
        for (hotkey, text) in table {
            assert_eq!(hotkey.to_string(), text);
            assert_eq!(parse_hotkey(text), Ok(hotkey), "{text}");
        }
    }

    #[test]
    fn every_key_and_modifier_combination_round_trips() {
        // Property: for every key of the closed set and every non-empty modifier
        // set, parse(format(h)) == h. Bite: a key with no name, or two keys sharing one.
        let keys = HotkeyKey::all();
        assert!(!keys.is_empty());
        for &key in keys {
            for mask in 1u8..16 {
                let h = hk(
                    mask & 1 != 0,
                    mask & 2 != 0,
                    mask & 4 != 0,
                    mask & 8 != 0,
                    key,
                );
                let text = h.to_string();
                assert_eq!(parse_hotkey(&text), Ok(h), "{text}");
            }
        }
    }

    #[test]
    fn closed_key_set_matches_r6() {
        // Bite: a key missing from or added to the R-6 set, or Esc in it.
        let keys = HotkeyKey::all();
        assert_eq!(keys.len(), 26 + 10 + 24 + 1 + 6 + 4 + 1 + 10);
        let mut names: Vec<String> = keys
            .iter()
            .map(|&key| {
                let text = hk(true, false, false, false, key).to_string();
                text.strip_prefix("Ctrl+")
                    .unwrap_or_else(|| panic!("{key:?}: {text}"))
                    .to_string()
            })
            .collect();
        for name in &names {
            assert!(
                !name.is_empty() && !name.contains('+'),
                "bad key name {name:?}"
            );
            for reserved in ["esc", "escape", "ctrl", "alt", "shift", "win"] {
                assert!(
                    !name.eq_ignore_ascii_case(reserved),
                    "{name} in the key set"
                );
            }
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), keys.len(), "two keys share a name");

        // Keys whose text R-6 fixes.
        let named = [
            ("A", HotkeyKey::A),
            ("Z", HotkeyKey::Z),
            ("0", HotkeyKey::Digit0),
            ("9", HotkeyKey::Digit9),
            ("F1", HotkeyKey::F1),
            ("F24", HotkeyKey::F24),
            ("Space", HotkeyKey::Space),
            ("Insert", HotkeyKey::Insert),
            ("Delete", HotkeyKey::Delete),
            ("Home", HotkeyKey::Home),
            ("End", HotkeyKey::End),
            ("PageUp", HotkeyKey::PageUp),
            ("PageDown", HotkeyKey::PageDown),
            ("Pause", HotkeyKey::Pause),
        ];
        for (name, key) in named {
            let text = format!("Ctrl+{name}");
            assert_eq!(parse_hotkey(&text).map(|h| h.key), Ok(key), "{text}");
        }
    }

    #[test]
    fn rejects_no_modifier() {
        for text in ["Space", "A", "F5", "0"] {
            assert_eq!(parse_hotkey(text), Err(HotkeyError::NoModifier), "{text}");
        }
    }

    #[test]
    fn rejects_no_key() {
        for text in ["Ctrl", "Ctrl+Alt", "Alt+Shift", "Ctrl+Alt+Shift+Win"] {
            assert_eq!(parse_hotkey(text), Err(HotkeyError::NoKey), "{text}");
        }
    }

    #[test]
    fn rejects_esc() {
        for text in ["Ctrl+Esc", "Ctrl+Alt+Esc", "Win+Esc"] {
            assert_eq!(parse_hotkey(text), Err(HotkeyError::EscReserved), "{text}");
        }
        // Whatever the spelling, Esc never parses.
        assert!(parse_hotkey("Ctrl+Escape").is_err());
    }

    #[test]
    fn rejects_invalid_strings() {
        // Bite: a lenient parser (unknown keys, two keys, empty parts, repeats).
        for text in [
            "",
            "+",
            "Ctrl+",
            "+A",
            "Ctrl++A",
            "Ctrl+A+B",
            "Ctrl+Shift+A+Space",
            "Ctrl+Ctrl+A",
            "Ctrl+F0",
            "Ctrl+F25",
            "Ctrl+Tab",
            "Ctrl+Enter",
            "Ctrl+AA",
            "Ctrl+Hyper+A",
        ] {
            assert!(parse_hotkey(text).is_err(), "{text:?} must be rejected");
        }
    }
}
