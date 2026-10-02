# Contract: shell ↔ UI IPC for this feature

The overlay is display-only (no buttons, never focused). Only the shell drives it. The e2e tests mock exactly these items through `window.__TAURI_INTERNALS__`.

## Event `overlay://state` (shell → overlay webview)

Emitted to the window labelled `overlay` on every change of `OverlayState`, and once right after the window has loaded (in reply to `overlay_ready`).

```ts
type OverlayStatePayload =
  | { kind: "recording"; elapsedMs: number }            // elapsed is shown as m:ss
  | { kind: "processing"; pending: number }             // pending = jobs not yet released
  | { kind: "message"; key: MessageKey; params: Record<string, string>; durationMs: 3000; severity: "error" | "info" }
  | { kind: "hidden" };                                 // the shell destroys the window after this
```

`MessageKey` values and texts: [messages.md](messages.md). The overlay formats the text from the catalog in the current UI language.

## Command `overlay_ready` (overlay → shell)

`invoke("overlay_ready") → OverlayStatePayload`. Returns the current state, so a freshly created webview does not miss the first event.

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
