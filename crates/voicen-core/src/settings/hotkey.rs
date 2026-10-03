//! Hotkey grammar (research R-6; spec 004 T030/T039; decision #23 N1).
//!
//! Canonical text: modifiers in the order `Ctrl`, `Alt`, `Shift`, `Win`, then one key
//! from the closed set [`HotkeyKey::all`], joined by `+` (`Ctrl+Alt+Space`). At least
//! one modifier, exactly one key, never `Esc` (reserved for cancel, FR-22). Whether a
//! valid hotkey can be registered is the registrar's question, not the grammar's.
//!
//! Key names (decision #25): letters `A`–`Z`, digits `0`–`9`, `F1`–`F24`, `Space`,
//! `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, the arrows `Up`, `Down`,
//! `Left`, `Right`, `Pause`, and numpad digits `Num0`–`Num9`.
//!
//! Parsing accepts the canonical text only (exact case, modifiers in canonical
//! order, each once, the key last), so every accepted string is its own canonical
//! form: `parse_hotkey(s)?.to_string() == s`.

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
        &ALL_KEYS
    }

    /// The key's text in the canonical hotkey string.
    fn name(self) -> &'static str {
        use HotkeyKey::*;
        match self {
            A => "A",
            B => "B",
            C => "C",
            D => "D",
            E => "E",
            F => "F",
            G => "G",
            H => "H",
            I => "I",
            J => "J",
            K => "K",
            L => "L",
            M => "M",
            N => "N",
            O => "O",
            P => "P",
            Q => "Q",
            R => "R",
            S => "S",
            T => "T",
            U => "U",
            V => "V",
            W => "W",
            X => "X",
            Y => "Y",
            Z => "Z",
            Digit0 => "0",
            Digit1 => "1",
            Digit2 => "2",
            Digit3 => "3",
            Digit4 => "4",
            Digit5 => "5",
            Digit6 => "6",
            Digit7 => "7",
            Digit8 => "8",
            Digit9 => "9",
            F1 => "F1",
            F2 => "F2",
            F3 => "F3",
            F4 => "F4",
            F5 => "F5",
            F6 => "F6",
            F7 => "F7",
            F8 => "F8",
            F9 => "F9",
            F10 => "F10",
            F11 => "F11",
            F12 => "F12",
            F13 => "F13",
            F14 => "F14",
            F15 => "F15",
            F16 => "F16",
            F17 => "F17",
            F18 => "F18",
            F19 => "F19",
            F20 => "F20",
            F21 => "F21",
            F22 => "F22",
            F23 => "F23",
            F24 => "F24",
            Space => "Space",
            Insert => "Insert",
            Delete => "Delete",
            Home => "Home",
            End => "End",
            PageUp => "PageUp",
            PageDown => "PageDown",
            ArrowUp => "Up",
            ArrowDown => "Down",
            ArrowLeft => "Left",
            ArrowRight => "Right",
            Pause => "Pause",
            Numpad0 => "Num0",
            Numpad1 => "Num1",
            Numpad2 => "Num2",
            Numpad3 => "Num3",
            Numpad4 => "Num4",
            Numpad5 => "Num5",
            Numpad6 => "Num6",
            Numpad7 => "Num7",
            Numpad8 => "Num8",
            Numpad9 => "Num9",
        }
    }

    fn from_name(name: &str) -> Option<HotkeyKey> {
        ALL_KEYS.iter().copied().find(|key| key.name() == name)
    }
}

const ALL_KEYS: [HotkeyKey; 82] = {
    use HotkeyKey::*;
    [
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z, Digit0,
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, F1, F2, F3, F4, F5,
        F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
        Space, Insert, Delete, Home, End, PageUp, PageDown, ArrowUp, ArrowDown, ArrowLeft,
        ArrowRight, Pause, Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7,
        Numpad8, Numpad9,
    ]
};

/// Modifier names in canonical order; the index is the rank.
const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

/// Spellings of the cancel key; never a hotkey key (FR-22).
const ESC_NAMES: [&str; 2] = ["Esc", "Escape"];

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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let flags = [self.ctrl, self.alt, self.shift, self.win];
        for (name, _) in MODIFIERS.iter().zip(flags).filter(|(_, on)| *on) {
            write!(f, "{name}+")?;
        }
        f.write_str(self.key.name())
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
    /// repeated or out-of-order modifier, an empty part (code `hotkey.invalid`).
    Invalid,
}

enum Token {
    Modifier(usize),
    Esc,
    Key(HotkeyKey),
}

fn token(part: &str) -> Option<Token> {
    if let Some(rank) = MODIFIERS.iter().position(|&m| m == part) {
        Some(Token::Modifier(rank))
    } else if ESC_NAMES.contains(&part) {
        Some(Token::Esc)
    } else {
        HotkeyKey::from_name(part).map(Token::Key)
    }
}

/// Parses the canonical text (see the module docs). Structural errors (unknown
/// token, empty part, a part after the key, a repeated or out-of-order modifier)
/// are [`HotkeyError::Invalid`]; otherwise a missing key, `Esc` as the key and a
/// missing modifier are reported in that order.
pub fn parse_hotkey(s: &str) -> Result<Hotkey, HotkeyError> {
    let mut modifiers = [false; 4];
    let mut last_rank: Option<usize> = None;
    // `Some(None)` is `Esc`: a key position taken by the reserved cancel key.
    let mut key: Option<Option<HotkeyKey>> = None;
    for part in s.split('+') {
        if key.is_some() {
            return Err(HotkeyError::Invalid);
        }
        match token(part).ok_or(HotkeyError::Invalid)? {
            Token::Modifier(rank) => {
                if last_rank.is_some_and(|last| last >= rank) {
                    return Err(HotkeyError::Invalid);
                }
                modifiers[rank] = true;
                last_rank = Some(rank);
            }
            Token::Esc => key = Some(None),
            Token::Key(k) => key = Some(Some(k)),
        }
    }
    let key = match key {
        None => return Err(HotkeyError::NoKey),
        Some(None) => return Err(HotkeyError::EscReserved),
        Some(Some(key)) => key,
    };
    if last_rank.is_none() {
        return Err(HotkeyError::NoModifier);
    }
    let [ctrl, alt, shift, win] = modifiers;
    Ok(Hotkey {
        ctrl,
        alt,
        shift,
        win,
        key,
    })
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
        // Bite: a lenient parser (unknown keys, two keys, empty parts, repeats), or a
        // structural error reported with another code than `Invalid` (#25(c)).
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
            assert_eq!(parse_hotkey(text), Err(HotkeyError::Invalid), "{text:?}");
        }
    }

    #[test]
    fn rejects_non_canonical_text() {
        // Canonical text only (decision #26; data-model › Hotkey): one spelling per
        // hotkey in the file, the UI and the logs (P-010). Each row below names a
        // valid hotkey in a non-canonical spelling, so it is `Invalid`, never another
        // code and never accepted.
        // Bites: (M1) `last >= rank` -> `last == rank` (out-of-order accepted);
        // (M2) case-insensitive modifier match; (M3) case-insensitive key match;
        // trimming parts or the whole string.
        for text in [
            // Modifiers out of the order Ctrl, Alt, Shift, Win.
            "Alt+Ctrl+A",
            "Shift+Ctrl+A",
            "Win+Alt+A",
            "Win+Ctrl+A",
            "Shift+Alt+A",
            "Win+Shift+Space",
            "Ctrl+Win+Alt+Space",
            "Ctrl+Alt+Win+Shift+F24",
            // Modifier case.
            "ctrl+A",
            "CTRL+A",
            "ctrl+Alt+Space",
            "Ctrl+ALT+Space",
            "Ctrl+alt+Space",
            "SHIFT+F9",
            "win+Z",
            // Key case.
            "Ctrl+a",
            "Ctrl+space",
            "Ctrl+SPACE",
            "Ctrl+f5",
            "Ctrl+num0",
            "Ctrl+NUM0",
            "Ctrl+pageup",
            "Ctrl+up",
            // Whitespace anywhere.
            "Ctrl + A",
            " Ctrl+A",
            "Ctrl+A ",
            "Ctrl+ A",
            "Ctrl +A",
            "Ctrl+Alt+Space\n",
            "\tCtrl+Alt+Space",
            "Ctrl+Page Up",
        ] {
            assert_eq!(parse_hotkey(text), Err(HotkeyError::Invalid), "{text:?}");
        }
    }

    /// Every ordered selection of distinct modifiers (none included), by name.
    fn modifier_sequences() -> Vec<Vec<&'static str>> {
        let mut out = vec![Vec::new()];
        let mut frontier: Vec<Vec<&'static str>> = vec![Vec::new()];
        for _ in 0..MODIFIERS_IN_TEST.len() {
            let mut next = Vec::new();
            for seq in &frontier {
                for m in MODIFIERS_IN_TEST {
                    if !seq.contains(&m) {
                        let mut longer = seq.clone();
                        longer.push(m);
                        next.push(longer);
                    }
                }
            }
            out.extend(next.iter().cloned());
            frontier = next;
        }
        out
    }

    /// The modifier names as the spec writes them (data-model › Hotkey), not taken
    /// from the module under test.
    const MODIFIERS_IN_TEST: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

    #[test]
    fn parse_then_format_is_identity_for_every_accepted_string() {
        // Property (data-model › Hotkey): for every string `s` the parser accepts,
        // `parse_hotkey(s)?.to_string() == s`. Candidates: every ordered modifier
        // selection (all 65 orders, empty included) x every key name of the closed
        // set plus `Esc`, each in seven spellings (as written, all lower, all upper,
        // modifiers lower, key lower, ` + ` separator, surrounding spaces).
        // Bites: M1 (out-of-order accepted), M2 (case-insensitive modifiers),
        // M3 (case-insensitive keys), any trim. Each makes a non-canonical candidate
        // parse, and its formatted text differs from it.
        let key_names: Vec<String> = HotkeyKey::all()
            .iter()
            .map(|&k| {
                let text = hk(true, false, false, false, k).to_string();
                text["Ctrl+".len()..].to_string()
            })
            .chain(["Esc".to_string()])
            .collect();
        let sequences = modifier_sequences();
        assert_eq!(sequences.len(), 1 + 4 + 12 + 24 + 24);

        let mut accepted = std::collections::BTreeSet::new();
        let mut candidates = 0usize;
        for seq in &sequences {
            for key in &key_names {
                let mods_lower: Vec<String> = seq.iter().map(|m| m.to_lowercase()).collect();
                let canonical_order: Vec<String> = seq.iter().map(|m| m.to_string()).collect();
                let join = |mods: &[String], key: &str, sep: &str| {
                    let mut parts: Vec<String> = mods.to_vec();
                    parts.push(key.to_string());
                    parts.join(sep)
                };
                let plain = join(&canonical_order, key, "+");
                for text in [
                    plain.clone(),
                    plain.to_lowercase(),
                    plain.to_uppercase(),
                    join(&mods_lower, key, "+"),
                    join(&canonical_order, &key.to_lowercase(), "+"),
                    join(&canonical_order, key, " + "),
                    format!(" {plain} "),
                ] {
                    candidates += 1;
                    if let Ok(h) = parse_hotkey(&text) {
                        assert_eq!(h.to_string(), text, "accepted {text:?} is not canonical");
                        accepted.insert(text);
                    }
                }
            }
        }
        assert!(candidates > 30_000, "{candidates} candidates");
        // Exactly one accepted spelling per hotkey: 82 keys x 15 modifier sets.
        assert_eq!(accepted.len(), HotkeyKey::all().len() * 15);
    }
}
