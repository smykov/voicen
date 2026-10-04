// Hotkey capture: the mechanical KeyboardEvent.code -> token rule (T-004; spec 004 T043).
import { describe, expect, it } from "vitest";
import { hotkeyText, isFocusMove, keyToken, type KeyPress } from "./hotkey";

const press = (code: string, mods: Partial<KeyPress> = {}): KeyPress => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

describe("hotkey capture", () => {
  it("maps codes to core's tokens by rule", () => {
    expect(
      ["KeyA", "Digit1", "F5", "Numpad0", "ArrowUp", "Space", "Insert", "PageDown", "Escape"].map(keyToken),
    ).toEqual(["A", "1", "F5", "Num0", "Up", "Space", "Insert", "PageDown", "Esc"]);
  });

  it("puts modifiers in core's order Ctrl, Alt, Shift, Win", () => {
    expect(hotkeyText(press("Space", { metaKey: true, shiftKey: true, altKey: true, ctrlKey: true }))).toBe(
      "Ctrl+Alt+Shift+Win+Space",
    );
    expect(hotkeyText(press("KeyD", { altKey: true }))).toBe("Alt+D");
  });

  it("keeps no allowlist: an unmodified or unknown key is passed on for core to refuse", () => {
    expect(hotkeyText(press("Escape", { ctrlKey: true }))).toBe("Ctrl+Esc");
    expect(hotkeyText(press("KeyQ"))).toBe("Q");
    expect(hotkeyText(press("IntlBackslash", { ctrlKey: true }))).toBe("Ctrl+IntlBackslash");
  });

  it("a modifier alone is not a hotkey yet", () => {
    for (const code of ["ControlLeft", "AltRight", "ShiftLeft", "MetaLeft", "OSRight"]) {
      expect(hotkeyText(press(code, { ctrlKey: true }))).toBeNull();
    }
  });

  it("Tab and Shift+Tab move focus; Ctrl+Tab is captured", () => {
    expect(isFocusMove(press("Tab"))).toBe(true);
    expect(isFocusMove(press("Tab", { shiftKey: true }))).toBe(true);
    expect(isFocusMove(press("Tab", { ctrlKey: true }))).toBe(false);
  });
});
