//! Win32 facts as pure data (T-051; research R-1, R-12): the numbers the shell
//! passes to `RegisterHotKey` and polls with `GetAsyncKeyState`, the hold-mode
//! release rule, and the "target is elevated" rule. No Win32 call and no `windows`
//! crate here, so the Linux gate proves the tables; T-006 cross-checks them
//! against the `windows::Win32::…` constants on Windows CI.
//!
//! The values were read from the pinned `windows` 0.62.2 source
//! (`Win32/UI/Input/KeyboardAndMouse/mod.rs`: `MOD_*`, `VK_*`;
//! `Win32/System/SystemServices/mod.rs`: `SECURITY_MANDATORY_*_RID`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::settings::hotkey::{Hotkey, HotkeyKey};

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
/// Auto-repeat of a held hotkey sends no further `WM_HOTKEY` (R-1).
const MOD_NOREPEAT: u32 = 0x4000;

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
/// Alt.
const VK_MENU: u16 = 0x12;
/// There is no generic Win virtual key: the left and the right one.
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;

/// The virtual-key code of a key of the closed set.
fn vk(key: HotkeyKey) -> u16 {
    use HotkeyKey::*;
    match key {
        A => 0x41,
        B => 0x42,
        C => 0x43,
        D => 0x44,
        E => 0x45,
        F => 0x46,
        G => 0x47,
        H => 0x48,
        I => 0x49,
        J => 0x4A,
        K => 0x4B,
        L => 0x4C,
        M => 0x4D,
        N => 0x4E,
        O => 0x4F,
        P => 0x50,
        Q => 0x51,
        R => 0x52,
        S => 0x53,
        T => 0x54,
        U => 0x55,
        V => 0x56,
        W => 0x57,
        X => 0x58,
        Y => 0x59,
        Z => 0x5A,
        Digit0 => 0x30,
        Digit1 => 0x31,
        Digit2 => 0x32,
        Digit3 => 0x33,
        Digit4 => 0x34,
        Digit5 => 0x35,
        Digit6 => 0x36,
        Digit7 => 0x37,
        Digit8 => 0x38,
        Digit9 => 0x39,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        F13 => 0x7C,
        F14 => 0x7D,
        F15 => 0x7E,
        F16 => 0x7F,
        F17 => 0x80,
        F18 => 0x81,
        F19 => 0x82,
        F20 => 0x83,
        F21 => 0x84,
        F22 => 0x85,
        F23 => 0x86,
        F24 => 0x87,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        Pause => 0x13,
        Numpad0 => 0x60,
        Numpad1 => 0x61,
        Numpad2 => 0x62,
        Numpad3 => 0x63,
        Numpad4 => 0x64,
        Numpad5 => 0x65,
        Numpad6 => 0x66,
        Numpad7 => 0x67,
        Numpad8 => 0x68,
        Numpad9 => 0x69,
    }
}

/// What `RegisterHotKey` and the release poll need for one hotkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyCodes {
    /// `MOD_*` bits of the pressed modifiers, always with `MOD_NOREPEAT` (R-1).
    pub modifiers: u32,
    /// The key's virtual-key code.
    pub vk: u16,
    /// One group per key of the combination (each modifier, then the key); a
    /// group is held while any of its virtual keys is down (Win: left or right).
    pub poll: Vec<Vec<u16>>,
}

/// The `RegisterHotKey` modifiers, the key's VK and the poll groups of `h`.
pub fn hotkey_codes(h: &Hotkey) -> HotkeyCodes {
    let key = vk(h.key);
    let mut modifiers = MOD_NOREPEAT;
    let mut poll = Vec::with_capacity(5);
    for (pressed, bit, group) in [
        (h.ctrl, MOD_CONTROL, &[VK_CONTROL][..]),
        (h.alt, MOD_ALT, &[VK_MENU][..]),
        (h.shift, MOD_SHIFT, &[VK_SHIFT][..]),
        (h.win, MOD_WIN, &[VK_LWIN, VK_RWIN][..]),
    ] {
        if pressed {
            modifiers |= bit;
            poll.push(group.to_vec());
        }
    }
    poll.push(vec![key]);
    HotkeyCodes {
        modifiers,
        vk: key,
        poll,
    }
}

/// Hold-mode release (R-1: the first key found up counts as the release): true
/// when any group of `poll` has no virtual key down. [`hotkey_codes`] always
/// gives at least the key's group; an empty `poll` is never released.
pub fn released(poll: &[Vec<u16>], is_down: impl Fn(u16) -> bool) -> bool {
    poll.iter()
        .any(|group| !group.iter().any(|&vk| is_down(vk)))
}

/// The unassigned virtual key the hotkey thread sends (down, then up) on each
/// `WM_HOTKEY` before calling the session, so the user's later Alt release opens no
/// menu in the focused window (research R-2; T-006). Not a key of the closed set
/// and not a modifier.
pub fn menu_mask_vk() -> u16 {
    todo!("T-006: the menu-mask virtual key (R-2)")
}

/// The virtual keys `Paster::wait_modifiers_released` polls (data-model
/// "DeliveryDecision": Shift, Ctrl, Alt and both Win keys; T-006).
pub fn modifier_wait_keys() -> &'static [u16] {
    todo!("T-006: the modifier-wait key set")
}

/// A mandatory integrity level RID (`SECURITY_MANDATORY_*_RID`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntegrityLevel(pub u32);

/// R-12 / data-model "StartWindow.elevated": the target runs above our integrity
/// level, or either level is unknown (then paste is unsafe: copy only).
///
/// UIPI compares the RIDs, so `MEDIUM_PLUS` (0x2100) is above `MEDIUM` (0x2000).
/// An unknown own level is treated like an unknown target (T-051 analysis Q4).
pub fn target_elevated(own: Option<IntegrityLevel>, target: Option<IntegrityLevel>) -> bool {
    match (own, target) {
        (Some(own), Some(target)) => target > own,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::hotkey::HotkeyKey;

    // Independent literals, read from the pinned windows 0.62.2 source
    // (Win32/UI/Input/KeyboardAndMouse/mod.rs:672-676 and :834-1243,
    // Win32/System/SystemServices/mod.rs:4262-4269), not from the code under test.
    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_WIN: u32 = 0x0008;
    const MOD_NOREPEAT: u32 = 0x4000;
    const VK_SHIFT: u16 = 0x10;
    const VK_CONTROL: u16 = 0x11;
    const VK_MENU: u16 = 0x12;
    const VK_LWIN: u16 = 0x5B;
    const VK_RWIN: u16 = 0x5C;

    const LOW: IntegrityLevel = IntegrityLevel(0x1000);
    const MEDIUM: IntegrityLevel = IntegrityLevel(0x2000);
    const MEDIUM_PLUS: IntegrityLevel = IntegrityLevel(0x2100);
    const HIGH: IntegrityLevel = IntegrityLevel(0x3000);
    const SYSTEM: IntegrityLevel = IntegrityLevel(0x4000);

    /// Every key of research R-6 with its VK, written out by hand.
    const VK_TABLE: [(HotkeyKey, u16); 82] = {
        use HotkeyKey::*;
        [
            (A, 0x41),
            (B, 0x42),
            (C, 0x43),
            (D, 0x44),
            (E, 0x45),
            (F, 0x46),
            (G, 0x47),
            (H, 0x48),
            (I, 0x49),
            (J, 0x4A),
            (K, 0x4B),
            (L, 0x4C),
            (M, 0x4D),
            (N, 0x4E),
            (O, 0x4F),
            (P, 0x50),
            (Q, 0x51),
            (R, 0x52),
            (S, 0x53),
            (T, 0x54),
            (U, 0x55),
            (V, 0x56),
            (W, 0x57),
            (X, 0x58),
            (Y, 0x59),
            (Z, 0x5A),
            (Digit0, 0x30),
            (Digit1, 0x31),
            (Digit2, 0x32),
            (Digit3, 0x33),
            (Digit4, 0x34),
            (Digit5, 0x35),
            (Digit6, 0x36),
            (Digit7, 0x37),
            (Digit8, 0x38),
            (Digit9, 0x39),
            (F1, 0x70),
            (F2, 0x71),
            (F3, 0x72),
            (F4, 0x73),
            (F5, 0x74),
            (F6, 0x75),
            (F7, 0x76),
            (F8, 0x77),
            (F9, 0x78),
            (F10, 0x79),
            (F11, 0x7A),
            (F12, 0x7B),
            (F13, 0x7C),
            (F14, 0x7D),
            (F15, 0x7E),
            (F16, 0x7F),
            (F17, 0x80),
            (F18, 0x81),
            (F19, 0x82),
            (F20, 0x83),
            (F21, 0x84),
            (F22, 0x85),
            (F23, 0x86),
            (F24, 0x87),
            (Space, 0x20),
            (Insert, 0x2D),
            (Delete, 0x2E),
            (Home, 0x24),
            (End, 0x23),
            (PageUp, 0x21),
            (PageDown, 0x22),
            (ArrowUp, 0x26),
            (ArrowDown, 0x28),
            (ArrowLeft, 0x25),
            (ArrowRight, 0x27),
            (Pause, 0x13),
            (Numpad0, 0x60),
            (Numpad1, 0x61),
            (Numpad2, 0x62),
            (Numpad3, 0x63),
            (Numpad4, 0x64),
            (Numpad5, 0x65),
            (Numpad6, 0x66),
            (Numpad7, 0x67),
            (Numpad8, 0x68),
            (Numpad9, 0x69),
        ]
    };

    fn hk(ctrl: bool, alt: bool, shift: bool, win: bool, key: HotkeyKey) -> Hotkey {
        Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key,
        }
    }

    /// The groups with each group and the list sorted, so the comparison does not
    /// depend on the order the code lists them in.
    fn normalized(poll: &[Vec<u16>]) -> Vec<Vec<u16>> {
        let mut groups: Vec<Vec<u16>> = poll
            .iter()
            .map(|g| {
                let mut g = g.clone();
                g.sort_unstable();
                g
            })
            .collect();
        groups.sort();
        groups
    }

    #[test]
    fn every_key_maps_to_its_virtual_key_code() {
        // T-051 row 22: all 82 keys of the closed set (HotkeyKey::all) against the
        // literal table above, and the table itself covers each key once with
        // distinct VKs. Bite: a wrong or swapped code (Up/Down, PageUp/PageDown,
        // Num0 vs 0), a key with no code, two keys sharing one.
        assert_eq!(VK_TABLE.len(), HotkeyKey::all().len());
        for key in HotkeyKey::all() {
            assert_eq!(
                VK_TABLE.iter().filter(|(k, _)| k == key).count(),
                1,
                "{key:?} is not in the literal table exactly once"
            );
        }
        let mut vks: Vec<u16> = VK_TABLE.iter().map(|(_, vk)| *vk).collect();
        vks.sort_unstable();
        vks.dedup();
        assert_eq!(vks.len(), VK_TABLE.len(), "literal VKs are not distinct");

        let mut wrong = Vec::new();
        for (key, vk) in VK_TABLE {
            let got = hotkey_codes(&hk(true, true, false, false, key)).vk;
            if got != vk {
                wrong.push(format!("{key:?}: {got:#04x}, expected {vk:#04x}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn every_modifier_combination_maps_to_mod_bits_with_norepeat() {
        // T-051 row 22 (R-1): the 16 combinations of Ctrl/Alt/Shift/Win give
        // exactly their MOD_* bits ORed with MOD_NOREPEAT, and one poll group per
        // pressed key: Ctrl [VK_CONTROL], Alt [VK_MENU], Shift [VK_SHIFT], Win
        // [VK_LWIN, VK_RWIN] (there is no generic Win VK), plus [the key's VK].
        // Bite: MOD_NOREPEAT dropped (auto-repeat presses), a bit swapped
        // (MOD_ALT 1 vs MOD_CONTROL 2), a modifier not polled, only VK_LWIN
        // polled for Win, the key not polled.
        let mut wrong = Vec::new();
        for bits in 0u8..16 {
            let (ctrl, alt, shift, win) =
                (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let codes = hotkey_codes(&hk(ctrl, alt, shift, win, HotkeyKey::Space));
            let mut modifiers = MOD_NOREPEAT;
            let mut poll = vec![vec![0x20u16]];
            if ctrl {
                modifiers |= MOD_CONTROL;
                poll.push(vec![VK_CONTROL]);
            }
            if alt {
                modifiers |= MOD_ALT;
                poll.push(vec![VK_MENU]);
            }
            if shift {
                modifiers |= MOD_SHIFT;
                poll.push(vec![VK_SHIFT]);
            }
            if win {
                modifiers |= MOD_WIN;
                poll.push(vec![VK_LWIN, VK_RWIN]);
            }
            let label = format!("ctrl={ctrl} alt={alt} shift={shift} win={win}");
            if codes.modifiers != modifiers {
                wrong.push(format!(
                    "{label}: modifiers {:#06x}, expected {modifiers:#06x}",
                    codes.modifiers
                ));
            }
            if codes.vk != 0x20 {
                wrong.push(format!("{label}: vk {:#04x}, expected 0x20", codes.vk));
            }
            if normalized(&codes.poll) != normalized(&poll) {
                wrong.push(format!(
                    "{label}: poll {:?}, expected {:?}",
                    codes.poll, poll
                ));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn released_iff_some_group_has_no_key_down() {
        // T-051 row 23 (R-1: the first key found up counts as the release): over
        // every set of down keys among Ctrl, LWIN, RWIN and Space for the groups
        // [Ctrl] [LWIN, RWIN] [Space], released == !(Ctrl && (LWIN || RWIN) &&
        // Space). Bite: all groups required up (release only when everything is
        // up), a group counted held only when all its keys are down (Win needing
        // both), the last group ignored.
        let poll = vec![vec![VK_CONTROL], vec![VK_LWIN, VK_RWIN], vec![0x20u16]];
        let keys = [VK_CONTROL, VK_LWIN, VK_RWIN, 0x20u16];
        let mut wrong = Vec::new();
        for mask in 0u8..16 {
            let down: Vec<u16> = keys
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, vk)| *vk)
                .collect();
            let is = |vk: u16| down.contains(&vk);
            let want = !(is(VK_CONTROL) && (is(VK_LWIN) || is(VK_RWIN)) && is(0x20));
            let got = released(&poll, |vk| down.contains(&vk));
            if got != want {
                wrong.push(format!("down {down:x?}: released {got}, expected {want}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn hotkey_poll_groups_release_on_any_key_and_need_both_win_keys_up() {
        // T-051 row 23 through the real table: for Ctrl+Win+F9 and Alt+Shift+Win+A,
        // over every set of down keys among the combination's VKs plus VK_RWIN,
        // the hold is released iff some key of the combination is up, Win counting
        // as up only when VK_LWIN and VK_RWIN are both up (the user may hold the
        // right Win key). Bite: only VK_LWIN polled (a hold on right Win releases
        // at once), the key not polled (releasing F9 alone never stops).
        let cases = [
            (hk(true, false, false, true, HotkeyKey::F9), 0x78u16),
            (hk(false, true, true, true, HotkeyKey::A), 0x41u16),
        ];
        for (h, key_vk) in cases {
            let poll = hotkey_codes(&h).poll;
            let mut keys = vec![key_vk, VK_LWIN, VK_RWIN];
            if h.ctrl {
                keys.push(VK_CONTROL);
            }
            if h.alt {
                keys.push(VK_MENU);
            }
            if h.shift {
                keys.push(VK_SHIFT);
            }
            let mut wrong = Vec::new();
            for mask in 0u32..(1 << keys.len()) {
                let down: Vec<u16> = keys
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, vk)| *vk)
                    .collect();
                let is = |vk: u16| down.contains(&vk);
                let held = is(key_vk)
                    && (is(VK_LWIN) || is(VK_RWIN))
                    && (!h.ctrl || is(VK_CONTROL))
                    && (!h.alt || is(VK_MENU))
                    && (!h.shift || is(VK_SHIFT));
                let got = released(&poll, |vk| down.contains(&vk));
                if got != !held {
                    wrong.push(format!(
                        "down {down:x?}: released {got}, expected {}",
                        !held
                    ));
                }
            }
            assert!(wrong.is_empty(), "{h}:\n{}", wrong.join("\n"));
        }
    }

    #[test]
    fn menu_mask_key_is_0xe8_and_no_key_or_modifier_of_the_set() {
        // T-006 red-test row 1 (R-2): the key the hotkey thread injects to keep Alt's
        // release from opening a menu is the unassigned VK 0xE8 (windows 0.62.2
        // names no VK_* for it). It must be none of the 82 keys of the closed set
        // (else it would press a real key in the focused app) and none of the
        // modifiers (else it would itself start or end a hold). Bite: the constant
        // missing, set to a real key (0x20 Space, 0x12 Alt) or to 0 (SendInput
        // ignores VK 0, so the menu opens).
        let mask = menu_mask_vk();
        assert_eq!(mask, 0xE8, "menu-mask VK");
        let mut collisions: Vec<String> = VK_TABLE
            .iter()
            .filter(|(_, vk)| *vk == mask)
            .map(|(key, vk)| format!("{key:?} ({vk:#04x})"))
            .collect();
        for modifier in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN] {
            if modifier == mask {
                collisions.push(format!("modifier {modifier:#04x}"));
            }
        }
        assert!(collisions.is_empty(), "mask collides: {collisions:?}");
    }

    #[test]
    fn modifier_wait_keys_are_exactly_shift_ctrl_alt_and_both_win_keys() {
        // T-006 red-test row 1 (data-model "DeliveryDecision", FR-010): the paste
        // waits until Shift, Ctrl, Alt and both Win keys are up, whatever the
        // hotkey. The set is exactly {0x10, 0x11, 0x12, 0x5B, 0x5C}, each once.
        // Bite: only the hotkey's own modifiers, VK_LWIN without VK_RWIN (a right
        // Win key held turns Ctrl+V into Win+Ctrl+V), a key of the closed set in
        // the set (the wait never ends while it is held), a duplicate.
        let keys = modifier_wait_keys();
        let mut sorted = keys.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec![VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN],
            "modifier-wait set {keys:x?}"
        );
        assert!(
            !keys.contains(&menu_mask_vk()),
            "the mask key is in the wait set"
        );
    }

    #[test]
    fn target_is_elevated_above_our_level_or_when_either_is_unknown() {
        // T-051 row 24 (R-12; analysis Q4: our own level unknown is the safe
        // side): unknown target -> elevated; unknown own level -> elevated; a
        // target strictly above ours (MEDIUM_PLUS over MEDIUM included, UIPI
        // compares the RID) -> elevated; equal or lower -> not. Bite: the
        // comparison reversed, `>=` (equal levels refused), an unknown level
        // treated as safe, a comparison of a coarse "elevated" flag that misses
        // MEDIUM_PLUS.
        let levels = [LOW, MEDIUM, MEDIUM_PLUS, HIGH, SYSTEM];
        let mut wrong = Vec::new();
        for own in levels {
            for target in levels {
                let want = target.0 > own.0;
                let got = target_elevated(Some(own), Some(target));
                if got != want {
                    wrong.push(format!(
                        "own {own:?} target {target:?}: {got}, expected {want}"
                    ));
                }
            }
            if !target_elevated(Some(own), None) {
                wrong.push(format!("own {own:?}, target unknown: not elevated"));
            }
            if !target_elevated(None, Some(own)) {
                wrong.push(format!("own unknown, target {own:?}: not elevated"));
            }
        }
        if !target_elevated(None, None) {
            wrong.push("both unknown: not elevated".to_string());
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
        assert!(
            target_elevated(Some(MEDIUM), Some(MEDIUM_PLUS)),
            "MEDIUM_PLUS over MEDIUM"
        );
        assert!(!target_elevated(Some(MEDIUM), Some(MEDIUM)), "equal levels");
    }
}
