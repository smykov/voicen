# Contract: shell ↔ UI IPC for this feature

The overlay is display-only (no buttons, never focused). Only the shell drives it. The e2e tests mock exactly these items through `window.__TAURI_INTERNALS__`.

## Event `overlay://state` (shell → overlay webview)

Emitted to the window labelled `overlay` on every change of `OverlayState`. The same payload is the reply to `overlay_ready`.

```ts
type OverlayPayload = {
  seq: number;          // the shell's number for this state, in the controller's order; higher = newer
  lang: "en" | "ru";    // the UI language the text is rendered in; the page calls setLanguage(lang)
  state: OverlayState;
};
type OverlayState =
  | { kind: "recording"; elapsedMs: number }   // time since the shell received this Recording; shown as m:ss
  | { kind: "processing" }
  | { kind: "message"; text: string }          // full text, rendered by Rust in `lang`
  | { kind: "hidden" };                        // the shell destroys the window after this
```

Built only by core `voicen_core::overlay::overlay_payload(&OverlayState, UiLanguage, seq, elapsed)`, serialized by its serde impl (T-053), pinned for the e2e mock by `e2e/fixtures/overlay-wire.json` (core test `e2e_overlay_wire_fixture_matches_core`).

- **Text is rendered in Rust.** A message's text is `voicen_core::i18n::text(lang, id, params)`, so a nested `mic_reason.*` argument is already resolved in the same language (decision #64). The page shows `text` as given. No message id, params, severity, duration or expiry reach the wire. The page renders only its own UI-only ids, `overlay.recording` (`{elapsed}` = m:ss) and `overlay.processing`, with `t` in `lang`. Catalog texts: [messages.md](messages.md).
- **Ordering.** The `overlay_ready` reply and an event can arrive in either order. The page applies a payload only if its `seq` is higher than the one it shows.
- **No UI timer.** Core expires every message (3 s, or what is left of it after a release) and publishes the next state. The page changes nothing on its own; only the elapsed m:ss ticks.

## Command `overlay_ready` (overlay → shell)

`invoke("overlay_ready") → OverlayPayload`. Returns the current state, so a freshly created webview does not miss the first event. Its `elapsedMs` counts from when the shell received the Recording state, so a webview that loads late still shows the true m:ss.

## Command `open_settings` (existing settings window, owned by 004)

`open_settings({ section: "hotkey" | "engine" | null })` is used by the shell itself (hotkey error, engine = none, second instance) and is listed here because 004's settings page must accept the `section` argument and focus the field.

## Command `set_hotkey` (settings UI → shell; the UI is owned by 004)

```ts
invoke("set_hotkey", { binding: { modifiers: ("ctrl"|"alt"|"shift"|"win")[], key: string, mode: "hold"|"toggle" } })
  → { ok: true } | { ok: false, error: "hotkey_unavailable" }
```

The shell registers the new binding before it unregisters the old one. On failure the old binding stays active, and the UI shows `failure.hotkey_unavailable` (spec FR-012). On success the binding is persisted through 004's settings store. A success also clears the tray `HotkeyError` state (FR-011).

## Not IPC

The tray, toasts, hotkey, capture, clipboard and paste never cross IPC. The UI cannot trigger recording or paste.
