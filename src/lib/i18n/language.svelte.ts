// The UI language `t()` renders in, as reactive state (T-005).
//
// The UI never derives a language itself (no navigator.language): it starts with
// `en` and renders whatever language it is given through `setLanguage`.
import type { UiLanguage } from "./index";

let current = $state<UiLanguage>("en");

/** The current UI language; reading it inside a template or effect tracks it. */
export function currentLanguage(): UiLanguage {
  return current;
}

/** Replace the current UI language. */
export function setCurrentLanguage(lang: UiLanguage): void {
  current = lang;
}
