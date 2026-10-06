# Overlay window (shell)

**Code:** `src-tauri/src/overlay.rs` (`OverlayPart`, `start`, the "overlay" thread, `overlay_ready`, `placement`), `src-tauri/src/dictation.rs` (`ShellIndicator::set_overlay` forwards to it), `src-tauri/capabilities/overlay.json`, core `crates/voicen-core/src/overlay.rs` (`OverlayLifecycle`, `WindowPhase`, `WindowAction`; the payload is T-053), `scripts/ci/visible-windows.ps1` (the smoke predicate) · **Tests that pin it:** core `overlay::lifecycle_tests` (Linux gate), `src-tauri/tests/overlay.rs`, `overlay_ipc.rs`, `lifecycle.rs`, `smoke_predicate.rs` (real Wry runtime / mock runtime, Windows CI only)

Task: T-057 (split from T-053 by its analysis, 2026-10-05). Class `overlay`; no `docs/failures.md` entry of this class yet, so the defects below are the risks the T-057 analysis found in the pinned sources (tauri 2.12.1, tauri-runtime-wry 2.12.1, tao 0.37.1), not incidents. Look and position: OQ-10. Why the window's contents are Rust-rendered: T-053, `docs/architecture.md` (`overlay_payload` row). Related: `docs/decisions/dictation-session.md` (the `Indicator` rule), `docs/decisions/windows-shell.md`.

## Invariants

The overlay shows exactly the session's published `OverlayState`, never activates, and exists only while that state is not `Hidden`.

### 1. `ShellIndicator::set_overlay` only records the newest state and wakes the overlay thread

- **Defect that produced it:** none in the failure log. `create_window` off the main thread sends a message and blocks on `rx.recv()` (runtime-wry `lib.rs:300-336`); on the main thread it runs inline (`:263-279`), and tauri documents window creation in sync commands and event handlers as a deadlock on Windows (`webview_window.rs:56-59`). `set_overlay` is called under the session lock.
- **What breaks if you violate it:** a window built (or any window call made) inside `set_overlay` blocks the hotkey path under the session lock, or deadlocks the main thread.
- **Where it is enforced:** `OverlayPart::set_overlay` stores `(seq, state, received-at)` in the mailbox (mutex + condvar) and wakes; it never touches a window or the settings and never calls the session (port contract, `platform.rs:111-117`). Test: `overlay.rs::set_overlay_returns_while_the_main_thread_is_blocked`.
- **Don't:** build, emit or destroy from `set_overlay`, from an event handler or from a sync command.

### 2. Every window operation runs on the one "overlay" thread, and a window is built only when none exists and none is being destroyed

- **Defect that produced it:** tauri frees a label only when the window's `Destroyed` event is delivered; `destroy()` is only posted (runtime-wry `lib.rs:2129-2136`; `app.rs:2709-2715` -> `manager/mod.rs:643-644`). A build before that fails with `WindowLabelAlreadyExists` (`manager/window.rs:70-71`). A tap shorter than 0.3 s followed by a new press gives Recording -> Hidden -> Recording, so this is a normal sequence.
- **What breaks if you violate it:** "label already exists" on the second dictation, or two overlay windows.
- **Where it is enforced:** build, emit and destroy run only on the thread started by `overlay::start` (from `wire`), which holds no lock while doing it. It follows the core reducer `OverlayLifecycle` (phase `Absent | Live | Destroying`; answers `Build | Emit | Destroy | Nothing`), proven on Linux including 2000 seeded interleavings against a model of tauri's label. "Destroyed" comes only from the window's own `on_window_event(Destroyed)` listener: runtime-wry hands `RunEvent::WindowEvent` to tauri first (`lib.rs:4224-4229`, label freed), and the window's own listeners run after (`:4230-4233`), so a rebuild triggered by it cannot hit the label error. A failed build returns the phase to `Absent` and retries only on the next state change, never in a loop. Tests: core `lifecycle_tests`; `overlay.rs::hidden_then_recording_at_once_leaves_exactly_one_visible_overlay`.
- **Don't:** build on `Hidden`-then-shown without waiting for `Destroyed`; learn "destroyed" from the return of `destroy()`; add a second place that creates or destroys label `overlay`.

### 3. The window is visible only after its styles are set, only through a non-activating show, and no setter touches it while visible

- **Defect that produced it:** none in the failure log. tao rewrites `GWL_EXSTYLE` from its own flags on any flag change of a visible window (`window_state.rs:412-431`) and ends in `ShowWindow(SW_SHOW)` (`:455-467`); a setter whose flags do not change is a no-op (`:318-320`), a hidden window only gets `SW_HIDE` (`:405-409`). `focusable(false)` gives `WS_EX_NOACTIVATE` (`:293-295`); `focused(false)` makes the creation show `SW_SHOWNOACTIVATE` through `MARKER_DONT_FOCUS`, a marker that exists only during creation (`window.rs:1265`, `:1415-1420`).
- **What breaks if you violate it:** the overlay takes focus from the window being dictated into (the paste then goes to the wrong place), or a raw style bit set by hand is dropped by the next setter.
- **Where it is enforced:** `overlay.rs::build` builds visible with `focused(false)`, `focusable(false)`, `always_on_top`, `skip_taskbar`, `decorations(false)`, `transparent`, `shadow(false)`, `resizable(false)` and a position, so creation shows it with `SW_SHOWNOACTIVATE` and nothing follows. `Hidden` destroys the window (NFR-03). Test: `overlay.rs::recording_processing_and_message_show_the_overlay_while_the_target_keeps_the_foreground` (foreground kept, `WS_EX_NOACTIVATE` and `WS_EX_TOPMOST`, no `WM_KILLFOCUS`).
- **Don't:** call `show`, `set_focus`, `set_ignore_cursor_events`, `set_position` or any other tao setter on the overlay while it is visible.

### 4. `overlay_ready` and every emit carry the payload of the mailbox's newest state, built outside the session lock

- **Defect that produced it:** none (T-057 analysis hypothesis 2(b)): a second copy of the state would break the seq rule on the page.
- **What breaks if you violate it:** a page that catches up through `overlay_ready` shows an older state than the last emit.
- **Where it is enforced:** one producer, core `overlay_payload`, fed with the mailbox entry and `SettingsService::snapshot().ui_language`, read on the overlay thread or in the command. Tests: `overlay_ipc.rs::overlay_ready_returns_the_newest_published_state_as_the_last_emit_carries_it`; the capability test `overlay_window_gets_nothing_beyond_listen_and_unlisten`.
- **Don't:** read the settings or the session under the session lock; keep a second state copy.

### 5. The install smoke counts every visible, unowned, top-level window of the process, tool windows included; it excludes a window only by pinned helper class AND helper shape

- **Defects that produced it:** F-003 (a smoke that decides on a guess instead of raw window facts): the predicate dropped `WS_EX_TOOLWINDOW` windows, so an overlay shown at start would have passed. T-037 review 1 #1: tao's event-target window (visible, unowned) was counted as the app's window. T-057 VERIFY_FAIL 3 (run 37419266451): the first T-057 rule excluded only tao's class, on the premise that tao's window is the only always-visible helper in a tauri process. That premise was false since T-052 registered tauri-plugin-single-instance 2.5.2, whose setup creates a second helper of the same shape (class `dev.voicen.app-sic`), so the first launch saw two shown windows.
- **What breaks if you violate it:** a stray window at start goes unnoticed (rule too wide), or every launch of the installed app fails the smoke (helper list incomplete).
- **Where it is enforced:** `scripts/ci/visible-windows.ps1` `VoicenWindows.Shown` (behind `Get-ShownWindows`) excludes a window only when (a) its class is on the pinned helper list, one entry per pinned framework source with its citation (tao 0.37.1 `Tao Thread Event Target`; tauri-plugin-single-instance 2.5.2 `<identifier>-sic` = `dev.voicen.app-sic`), and (b) its ex-style has all four helper bits `WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW`. A helper the list does not know fails the smoke, and `Format-ShownWindows` prints its class and ex-style in hex. Tests: `smoke_predicate.rs` (`a_visible_unowned_tool_window_counts_as_shown`, `the_single_instance_helper_window_is_not_shown` (reads the identifier from `tauri.conf.json`, so a drift of the pinned class fails), `taos_event_target_window_is_not_shown`, `a_single_instance_class_window_without_the_helper_bits_counts`, `a_tao_class_window_without_the_helper_bits_counts`, `a_click_through_overlay_window_counts`); the smoke's first, loaded and second launch. The tests run the script through `run_pwsh` (bounded, one at a time, stall reported with its `stage:` marker; F-006). The set of helpers a real `voicen.exe` has is proven only by the install smoke (no test process can register the plugin).
- **Don't:** add an "it is our overlay" exception by class or title; exclude by style alone (tao maps click-through to `WS_EX_TRANSPARENT|WS_EX_LAYERED`, so a click-through overlay would drop out), by size or by title; add a helper class without its pinned source citation.

### Last window and exit

Destroying the overlay when it is the last window raises `ExitRequested{code: None}` (runtime-wry `lib.rs:4256-4270`), so every `Hidden` with no settings window open does. `on_run_event` prevents it while the tray exists (`docs/decisions/windows-shell.md`); that guard stays the one place that decides. Without a tray the app ends after the first dictation, the same as closing the settings window without a tray (T-057 analysis Q1, default applied). Tests: `lifecycle.rs::destroying_the_overlay_as_the_last_window_keeps_the_app_while_the_tray_exists`, `without_the_tray_destroying_the_last_window_ends_the_app`.

## How the implementation differs from the T-057 analysis

1. **Placement by raw Win32.** `placement()` reads `GetForegroundWindow`, `GetWindowRect`, `MonitorFromPoint`, `GetMonitorInfoW` and `GetDpiForMonitor` instead of tauri's `monitor_from_point(..).work_area`: `MockRuntimeHandle::monitor_from_point` is `unimplemented!()` in tauri 2.12.1 (`mock_runtime.rs:259-261`), which the mock-runtime tests would hit. It also needs no main-thread round trip. Cost: the `windows` features `Win32_Graphics_Gdi` and `Win32_UI_HiDpi` (feature flags only, no new crate). Position: bottom-centre of the work area of the monitor holding the centre of the window in front (primary monitor when none), 48 px above the bottom, 320x56 logical px (OQ-10 proposal). A monitor that cannot be read leaves the position to tao.
2. **Click-through and `WS_EX_TOOLWINDOW` are not implemented.** Both would need a setter or a raw style change after creation (invariant 3); the analysis left them to the developer. The window is therefore not click-through and appears in Alt+Tab. Follow-up: OQ-10 (no decision recorded).
3. **Destroyed outside the reducer's request, and window generations.** Every `Build` the reducer returns gets a new, higher generation (`OverlayLifecycle::generation`); the shell's `Destroyed` listener of that window posts it, so the mailbox holds a list of generations, not a count. Each wake hands every posted `Destroyed` and the newest state to `OverlayLifecycle::on_wake` together, and it answers one action: the current window's `Destroyed` is applied first (counted once however often posted), then the state, so the order they arrived in does not matter (review 1 finding 2). A `Destroyed` of another generation, or with no window, is ignored (finding 3). An unrequested `Destroyed` of the current window (it went away by itself: Alt+F4, exit) drops the handle and leaves no window; it is rebuilt only for a newer shown state, which may come in the same wake (finding 1). A requested `Destroyed` plus a newer state in one wake builds once, for the newer state. A failed `destroy()` writes `warning kind=overlay_failed`; if the label is already free (the window is gone and may post no `Destroyed`), the thread posts a synthetic `Destroyed` for that window's generation so the next shown state can build, and the real one, if it still comes after a rebuild, is stale and ignored; if the window still exists it stays until its own `Destroyed`. Tests: core `lifecycle_tests` (`on_wake` and generation tests, `any_wake_sequence_ignores_stale_destroyed_and_rebuilds_a_window_gone_with_a_newer_state`); `overlay.rs::an_overlay_destroyed_from_outside_is_rebuilt_by_a_newer_shown_state`.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Build the window inside `set_overlay` | blocks under the session lock or runs inline on the main thread (invariant 1) | T-057 analysis |
| Show hidden, then set styles and `ShowWindow(SW_SHOWNOACTIVATE)` by hand | works only if no tao setter follows; creation with `focused(false)` gives the same without raw style edits | T-057 analysis, `approach` |
| Keep the overlay alive while hidden | breaks NFR-03 | spec 001 R-13 |

## Open

- OQ-10: position and look; click-through and tool window not done.
