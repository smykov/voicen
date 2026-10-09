//! T-012 (FR-27; spec 001 US6, FR-030/FR-031; data-model `MicrophoneChoice`):
//! the device a press records from, decided only here, and the notify-once
//! memory of the fallback notice. Red tests first; the implementation follows the
//! pinned API below (docs/tasks/T-012.md, analysis option A).
//!
//! Pinned API (T-012 `## Tests`):
//! - `pub fn choose(selected: Option<&str>, devices: &[InputDevice]) -> Choice`:
//!   `selected` is the saved `settings::Microphone::id` (the endpoint id), the list
//!   is `AudioSource::devices()` taken at that press;
//! - `pub enum Choice { Use { id: DeviceId, kind: DeviceKind }, NoDevice }`;
//! - `pub struct MicrophoneState` (`new()`, in memory only, OQ-24 (A)) with
//!   `opened(&mut self, id: &DeviceId, kind: DeviceKind) -> bool`: the press whose
//!   capture opened `id` as `kind`; `true` exactly when `notice.mic_fallback` is
//!   due (Selected → Fallback(x), Fallback(x) → Fallback(y)).
//!
//! `platform::{DeviceId(pub String), InputDevice { id, name, is_default }}` are the
//! port's types.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::DeviceKind;
    use crate::platform::{DeviceId, InputDevice};

    // Fake endpoint ids, shaped like WASAPI's (`{0.0.1.00000000}.{guid}`), never a
    // real device.
    const USB: &str = "{0.0.1.00000000}.{fake-usb-headset-0001}";
    const ARRAY: &str = "{0.0.1.00000000}.{fake-mic-array-0002}";
    const DOCK: &str = "{0.0.1.00000000}.{fake-dock-mic-0003}";

    fn id(s: &str) -> DeviceId {
        DeviceId(s.to_string())
    }

    fn dev(s: &str, name: &str, is_default: bool) -> InputDevice {
        InputDevice {
            id: id(s),
            name: name.to_string(),
            is_default,
        }
    }

    fn usb() -> InputDevice {
        dev(USB, "USB Headset (fake)", false)
    }
    fn array_default() -> InputDevice {
        dev(ARRAY, "Microphone Array (fake)", true)
    }
    fn dock_default() -> InputDevice {
        dev(DOCK, "Dock Mic (fake)", true)
    }

    fn use_(s: &str, kind: DeviceKind) -> Choice {
        Choice::Use { id: id(s), kind }
    }

    #[test]
    fn choose_follows_the_closed_rule_table() {
        // T-012 invariant (1), the `choose` table of `## Investigation`: selected
        // present -> that id, Selected (whether or not it is also the default);
        // nothing selected -> the default, Selected (data-model: none = default, no
        // notice); selected missing -> the default, Fallback; no device flagged
        // default (an empty list included) -> NoDevice, whatever is selected (FR-04
        // branch). Bite: always the default (row 1), the first device instead of the
        // default (rows 2 and 3: the USB device is first), Fallback for "nothing
        // selected" (row 3), Selected for a missing one (row 4), the first device
        // when none is default (row 5), a panic or a device on an empty list.
        let rows: Vec<(&str, Option<&str>, Vec<InputDevice>, Choice)> = vec![
            (
                "selected present, not the default",
                Some(USB),
                vec![usb(), array_default()],
                use_(USB, DeviceKind::Selected),
            ),
            (
                "selected present and the default",
                Some(ARRAY),
                vec![usb(), array_default()],
                use_(ARRAY, DeviceKind::Selected),
            ),
            (
                "nothing selected",
                None,
                vec![usb(), array_default()],
                use_(ARRAY, DeviceKind::Selected),
            ),
            (
                "selected missing",
                Some(USB),
                vec![dev(DOCK, "Dock Mic (fake)", false), array_default()],
                use_(ARRAY, DeviceKind::Fallback),
            ),
            (
                "selected missing, another default",
                Some(USB),
                vec![dock_default()],
                use_(DOCK, DeviceKind::Fallback),
            ),
            (
                "selected missing, no default flagged",
                Some(USB),
                vec![
                    dev(DOCK, "Dock Mic (fake)", false),
                    dev(ARRAY, "Array", false),
                ],
                Choice::NoDevice,
            ),
            (
                "nothing selected, no default flagged",
                None,
                vec![dev(DOCK, "Dock Mic (fake)", false)],
                Choice::NoDevice,
            ),
            (
                "empty list, nothing selected",
                None,
                vec![],
                Choice::NoDevice,
            ),
            (
                "empty list, a selection",
                Some(USB),
                vec![],
                Choice::NoDevice,
            ),
        ];
        let mut wrong = Vec::new();
        for (name, selected, devices, want) in rows {
            let got = choose(selected, &devices);
            if got != want {
                wrong.push(format!("{name}: got {got:?}, expected {want:?}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn choose_matches_the_selected_device_by_its_whole_id_only() {
        // Research R-3 / `## Investigation`: the saved id is the endpoint id and
        // matching is by id only, exactly. A device whose name equals the selected
        // id, or whose id merely starts with it, is not the selected one: the
        // default is used as a fallback. Bite: a prefix / contains match, a match
        // on the name.
        let devices = vec![
            dev(&format!("{USB}-other"), "another device (fake)", false),
            dev(DOCK, USB, false),
            array_default(),
        ];
        assert_eq!(
            choose(Some(USB), &devices),
            use_(ARRAY, DeviceKind::Fallback)
        );
    }

    #[test]
    fn tracker_notifies_exactly_on_entering_or_changing_the_fallback() {
        // T-012 invariant (2), FR-27 "once per device change": a notice on
        // Selected -> Fallback(x) and on Fallback(x) -> Fallback(y); none on
        // Fallback(x) -> Fallback(x), on -> Selected, or on Selected -> Selected;
        // after the selected device came back, the next fallback (even to the same
        // x) notifies again. The memory starts as "selected" (in memory only, OQ-24
        // (A)), so the first fallback after a start notifies. Bite: notify on every
        // fallback press (step 3), only on the first fallback ever (steps 4 and 6),
        // a memory keyed on the kind only (step 4: x -> y silent), a notice when the
        // selected device returns (step 5), no reset on Selected (step 6).
        let mut m = MicrophoneState::new();
        let steps: Vec<(&str, &str, DeviceKind, bool)> = vec![
            ("1 selected", USB, DeviceKind::Selected, false),
            (
                "2 selected -> fallback(array)",
                ARRAY,
                DeviceKind::Fallback,
                true,
            ),
            (
                "3 fallback(array) again",
                ARRAY,
                DeviceKind::Fallback,
                false,
            ),
            (
                "4 fallback(array) -> fallback(dock)",
                DOCK,
                DeviceKind::Fallback,
                true,
            ),
            ("5 back to selected", USB, DeviceKind::Selected, false),
            (
                "6 fallback(dock) after selected",
                DOCK,
                DeviceKind::Fallback,
                true,
            ),
            ("7 fallback(dock) again", DOCK, DeviceKind::Fallback, false),
        ];
        let mut wrong = Vec::new();
        for (name, dev_id, kind, want) in steps {
            let got = m.opened(&id(dev_id), kind);
            if got != want {
                wrong.push(format!("{name}: notice {got}, expected {want}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn the_first_press_after_a_start_on_the_fallback_notifies() {
        // OQ-24 (A): in memory only, starting from "selected", so a fallback as
        // the very first press of a run notifies once, the second does not. Bite:
        // a memory that starts as "already notified" or as Fallback of the first
        // device seen.
        let mut m = MicrophoneState::new();
        assert!(m.opened(&id(ARRAY), DeviceKind::Fallback), "first press");
        assert!(!m.opened(&id(ARRAY), DeviceKind::Fallback), "second press");
    }
}
