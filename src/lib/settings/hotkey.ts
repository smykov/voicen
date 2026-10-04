// Hotkey capture for the settings window (spec 004 T043; T-004 Investigation 3).
//
// A key press becomes canonical hotkey text by a mechanical rule over
// `KeyboardEvent.code`; there is no allowlist here. Core's closed key set (R-6) is the
// only rule: whatever this produces, core accepts it or refuses it with a FieldError.

/** The parts of a KeyboardEvent the capture reads. */
export interface KeyPress {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

/** The token of a non-modifier key code: `KeyA`->`A`, `Digit1`->`1`, `ArrowUp`->`Up`, `Numpad0`->`Num0`, `Escape`->`Esc`. */
export function keyToken(code: string): string {
  if (code === "Escape") return "Esc";
  return code.replace(/^(Key|Digit|Arrow)(?=.)/, "").replace(/^Numpad(?=.)/, "Num");
}

/**
 * The hotkey text of a press (modifiers in core's order Ctrl, Alt, Shift, Win, then the
 * key), or null when the press is a modifier alone (the user is still choosing).
 */
export function hotkeyText(press: KeyPress): string | null {
  if (MODIFIER_CODES.test(press.code)) return null;
  const parts: string[] = [];
  if (press.ctrlKey) parts.push("Ctrl");
  if (press.altKey) parts.push("Alt");
  if (press.shiftKey) parts.push("Shift");
  if (press.metaKey) parts.push("Win");
  parts.push(keyToken(press.code));
  return parts.join("+");
}

/** Tab and Shift+Tab without other modifiers move focus and are not captured. */
export function isFocusMove(press: KeyPress): boolean {
  return press.code === "Tab" && !press.ctrlKey && !press.altKey && !press.metaKey;
}
