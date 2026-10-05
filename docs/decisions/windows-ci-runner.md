# Windows CI runner: what a test may assume about windows-latest

**Code:** `src-tauri/tests/runner_probe.rs` (one `#[ignore]` test, `std` and `windows` only), `.github/workflows/ci.yml` (windows job, the last step "Runner capability probe"), `src-tauri/Cargo.toml` (the release `windows` features line) · **Tests that pin it:** the probe step's guard in every windows job run (exactly one end line; its count equals the well-formed fact lines and all `runner-probe:` lines but the end line); `make check-shell-windows` type-checks the probe on Linux and never runs it

Task: T-059 (from the T-006 Refresh, Q5; design in `docs/tasks/T-059.md` › Investigation). Class `ci-toolchain`; no `docs/failures.md` entry: this is a need, not a defect. Decisions: #5 and #66 (where the shell is checked). Related: `docs/decisions/ci-toolchain.md` (shell tests only in `src-tauri/tests`; a test exe that cannot start turns the windows job red), F-003 (raw values, no model) and F-005 (Windows-sized waits).

## Why this area exists

T-006 (hotkey thread, `SendInput` paste, clipboard), T-057 (the overlay never takes the foreground) and T-052 (tray icon rect) plan Windows CI tests. Those tests hold only if a cargo test exe on windows-latest can:

- make its own window the foreground window;
- inject keys that reach that window;
- fire a `RegisterHotKey` hotkey with injected keys;
- read the injected keys with `GetAsyncKeyState`;
- own the clipboard.

Before T-059 nothing had observed any of this on the runner. The install smoke reads only window existence, visibility, owner, ex-style, class and title (`scripts/ci/visible-windows.ps1`). Every other shell test runs on tauri's mock runtime or calls no window API. And libtest swallows what a passing test prints, so a fact printed from the normal `cargo test -p voicen` step never reaches the job log.

## Invariants

### A Windows CI test asserts a runner capability only if the probe's recorded runs show it `ok`, and it asserts that precondition loudly

- **Defect that produced it:** none (a need: T-006 Refresh Q5).
- **What breaks if you violate it:**
  - a red test that cannot run on the runner reports nothing about the code: it fails for a runner reason, or blocks every `deploys: true` task behind the windows job;
  - a precondition that is not checked passes vacuously. Example: a "the target stays in front" test on a runner where nothing is ever in front.
- **The capabilities:** foreground window, injected keyboard input reaching a window, `RegisterHotKey` fired by injected keys, `GetAsyncKeyState` seeing injected keys, clipboard, notification area (`taskbar`).
  - A capability counts as available only when its fact is `ok` in every recorded run below (at least runs A and B).
  - A fact that is missing (any status but `ok`) or differs between runs moves the Acceptance lines it carries to the owner's manual check (consequence table).
- **Where it is enforced:**
  - The probe prints the facts in every windows job, so a runner image change shows in the log.
  - Review (T1): a Windows CI test that needs a capability cites this file's latest facts, and asserts the precondition with a message naming it. The probe forecasts and does not guarantee.
- **Don't:**
  - assert a capability in a Windows CI test because "it worked on my PC" or because a third party's run says so (runner-images issue 14049 is a forecast only);
  - read a capability from the install smoke, which never reads foreground, focus, input, hotkeys or the clipboard.

### The probe reports; it never fails because a capability is missing

- **Defect that produced it:** none. It is what keeps the facts flowing: a probe that went red on a missing capability would hide the other facts, and invite `continue-on-error`.
- **What breaks if you violate it:** the facts stop, or the step turns green with no facts. The way to the second: `--ignored` on a test that is not ignored, which runs zero tests (the end-line check catches it). The probe writes its lines to stdout directly (`io::stdout().lock()`), which libtest's capture does not intercept (it covers only `print!`-family output; measured on libtest 1.99, T-059 review 1 #2), so `--no-capture` is belt-and-braces: it keeps any future `print!`/`eprint!` live.
- **Where it is enforced:**
  - `runner_probe.rs`:
    - every fact is exactly one line, printed by the main thread, which makes no Win32 call;
    - every wait is a `PeekMessageW` poll against `WAIT` = 3 s at `POLL` = 10 ms; `OpenClipboard` gets 10 tries 50 ms apart;
    - a 60 s `WATCHDOG` prints `timeout` for every fact still missing, then the end line;
    - the test fails only on its own defect: a probe thread that panicked, ended before sending its facts, or sent a fact twice.
  - The step:
    - `cargo test -p voicen --test runner_probe -- --ignored --no-capture --test-threads=1 | tee runner-probe.log` under `shell: bash`, so `set -e` and `pipefail` apply;
    - then the guard: exactly one end line, whose count equals both the well-formed fact lines and all `runner-probe:` lines but the end line;
    - it is the last step of the windows job with `if: ${{ !cancelled() }}` and `timeout-minutes: 10`, so a probe defect cannot skip the deploy steps, and the facts print even when an earlier step failed;
    - no `continue-on-error` and no `|| true`.
  - The `cargo test -p voicen --no-fail-fast` step still starts the probe exe (its test is ignored), so a start failure of the F-002 class stays red there.
- **Don't:**
  - add a capability assertion to the probe;
  - run it inside the normal `cargo test -p voicen` step (captured, and run twice with its own step);
  - drop `#[ignore]` or `--ignored`, drop `--no-capture` (kept for `print!`-family output), or spell it `--nocapture` (deprecated in libtest 1.99);
  - print fact lines from a probe thread;
  - wait without a bound (`GetMessageW`, an unbounded loop);
  - give the step `continue-on-error`;
  - move the step before the deploy steps.

### The probe touches only its own state and puts it back

- **Where it is enforced:** in `runner_probe.rs`:
  - one window thread owns a top-level window of the system `EDIT` class. There is no `RegisterClassW`, so no `Win32_Graphics_Gdi` is needed;
  - guards undo what the thread did, on every path: the key-ups (`Release`), the hotkey registration (`Hotkey`), emptying the clipboard (`ClipboardCleanup`) and `DestroyWindow` (`Window`), all on that thread;
  - Alt is released only after the unassigned virtual key 0xE8, so the release activates no menu (T-006 R-2);
  - in the hotkey probe, keyboard messages are taken from the queue but not dispatched, because Alt+Space dispatched to the window could open its menu, a modal loop;
  - `CloseClipboard` runs on every path through the `Opened` guard. A block the clipboard took is never freed. `GlobalUnlock`'s `Err` with code 0 (the last unlock, windows-result 0.4.1) counts as success;
  - the clipboard holds only the fake text `voicen-probe-<pid>`.

## Line grammar

One line per fact, in this order: `image`, `session`, `station`, `desktop`, `foreground_lock`, `taskbar` (session thread), then `foreground`, `sendinput`, `clipboard`, `clipboard_null_owner`, `hotkey`, `async_keys` (window thread). Then the end line.

```text
^runner-probe: (?<fact>[a-z_]+)=(?<status>ok|denied|lost|absent|skipped|timeout)(\((?<detail>[^()]*)\))?$
runner-probe: end(facts=N)
```

- The detail is `k=v` pairs joined by `,`. Any character of a key or value outside `A-Za-z0-9_.:-` is printed as `_`.
- OS codes are printed as `err=0xHHHHHHHH` (`windows::core::Error::code()`). The `ok` lines of the six window-thread facts (`foreground` … `async_keys`) carry a measured `ms`; the session-thread facts carry none, and `foreground_lock`'s `ms` is the system setting (`SPI_GETFOREGROUNDLOCKTIMEOUT`), not a time.
- Status:

| Status | Meaning |
|---|---|
| `ok` | the capability was observed (window-thread facts with the measured `ms`) |
| `denied` | the API reported failure |
| `lost` | the API reported success, but the effect was not observed within `WAIT` |
| `absent` | an object was not found |
| `skipped(reason=…)` | a precondition of the probe itself is missing: `no_window` (`CreateWindowExW` failed, with `err`); `no_thread` and `probe_thread_ended` are probe defects and the test fails too |
| `timeout(ms=…)` | no answer before the watchdog (ms since the probe started) |

| Fact | Detail (raw values; the doc interprets) |
|---|---|
| `image` | `os`, `version`: the system variables `ImageOS`, `ImageVersion` (`none` when unset; then the status is `absent`) |
| `session` | `id` (`ProcessIdToSessionId` of the probe's pid) or `err`; `console` (`WTSGetActiveConsoleSessionId`, 4294967295 = no console session); `remote` (`GetSystemMetrics(SM_REMOTESESSION)`, nonzero = remote) |
| `station` | `name` (`UOI_NAME` of `GetProcessWindowStation()`), `visible` (`UOI_FLAGS` & `WSF_VISIBLE`); on failure `step=get\|name\|flags` and `err` |
| `desktop` | `thread` (`UOI_NAME` of `GetThreadDesktop(GetCurrentThreadId())`) or `thread_err`; `input` (`UOI_NAME` of `OpenInputDesktop(0, false, DESKTOP_READOBJECTS)`) or `input_err` |
| `foreground_lock` | `ms` (`SPI_GETFOREGROUNDLOCKTIMEOUT`) or `err` |
| `taskbar` | `FindWindowW("Shell_TrayWnd", NULL)`: `ok`, or `absent` (with `err`, the thread's last error after the lookup, when nonzero: it is not cleared before `FindWindowW`, so it may be stale from an earlier call; the status is right) |
| `foreground` | `ms` (from `SetForegroundWindow` until `GetForegroundWindow()` is the probe window); `before` (class of the window in front before the probe window existed, `none`); `created_fg` (in front right after creation); `set` (`SetForegroundWindow`'s BOOL); `focus` (`GetFocus()`: `self`, `none` or a class); `now` (class in front, when not `ok`). `lost` = `set=1` but never in front; `denied` = `set=0` |
| `sendinput` | `ms` (to `WM_KEYDOWN` `VK_A` at the window); `inserted`; `keydown`; `char` (the `WM_CHAR` code `TranslateMessage` made, hex, or `none`); `text_len_before`, `text_len` (the EDIT's text length); `fg` (the window was in front at `SendInput`); `err` when 0 inserted |
| `clipboard` | `ms`, `open_tries` (`OpenClipboard(window)`); on failure `step` = `open`, `empty`, `alloc`, `lock`, `unlock` or `set` (`denied`), or `reopen`, `get`, `read_lock` or `compare` (`lost`, after a successful `SetClipboardData`), with `err` or `read_len` |
| `clipboard_null_owner` | the same with `OpenClipboard(NULL)` (T-006 design 3's open point) |
| `hotkey` | `RegisterHotKey(window, Ctrl+Alt+Space, MOD_NOREPEAT)` and one `SendInput` of Ctrl↓ Alt↓ Space↓: `ms` (to `WM_HOTKEY` with the probe's id), `inserted`; `lost` adds `wm_hotkey=0`; `denied` has `step=register` (0x80070581 = 1409: another process holds the combination) or `step=sendinput`, with `err`; `fg`. When `RegisterHotKey` fails the Ctrl+Alt+Space press is still sent (review ruling, T-059 review 1 #6): it reaches whoever holds the combination, and that holder may swallow Space for `async_keys` |
| `async_keys` | `GetAsyncKeyState` of Ctrl, Alt, Space while held and after the release (0xE8 down/up, Space↑ Alt↑ Ctrl↑): `ms` (slowest key read down), `up_ms` (slowest read up), `down` / `up` (`ctrl:1.alt:1.space:1`), `released` (inserted count of the 5-event release batch), `inserted` when `lost`, `err` when `denied`, `fg` (`GetAsyncKeyState` may read 0 while another process's thread is in front). Sent also when the `hotkey` registration failed: see `hotkey` |

## Consequences per outcome

T-006 test numbers are those of the T-006 Refresh table. The orchestrator rewords the follow-up Acceptance from this table.

| Probe fact | `ok` in every recorded run → stays on Windows CI | missing (any status but `ok`, or different between runs) → |
|---|---|---|
| `foreground` (with `focus=self`) | T-006 tests 5, 11, 12, 13, 18, 19, which carry T-006 Acceptance line 1 and line 3's "start window replaced"; T-057 Acceptance line 1 (target in front, no `WM_KILLFOCUS`) | T-006: line 1 moves to the owner (owner step 2 already pastes into Notepad). Line 3's replaced/elevated branch stays on CI only through `delivery.rs` with a `Paster` wrapper (tests 20–22 unaffected); the real replacement is owner step 5. T-057 line 1: if `foreground.before` shows a window, CI asserts "`GetForegroundWindow()` unchanged through Recording, Processing and Message, and never the overlay", and `WM_KILLFOCUS` / "target stays foreground" move to the owner (line 3). With no window in front at all, line 1 is the owner's |
| `sendinput` | T-006 tests 3, 4, 5, 12, 15, 18 | those move to the owner (steps 2 and 4; the design-4 risk). Tests that call session inputs directly (19–23) are unaffected |
| `hotkey`: registration | T-006 tests 6, 7, smoke 24 | `denied(step=register,err=0x80070581)`: the tests take another combination through `win32_data` (product unchanged), recorded here. Any other code moves them to the owner |
| `hotkey`: `WM_HOTKEY` from injected keys | T-006 tests 3, 4, 5, 18 | move to the owner (step 2) |
| `async_keys` | T-006 tests 3, 4 (release poll), 15 (modifier wait) | move to the owner (steps 2 and 4) |
| `clipboard` | T-006 tests 8, 9, 10, 12, 13, 18, 19; line 1's "clipboard holds it, excluded from history" | moves to the owner (Win+V in step 2) |
| `clipboard_null_owner` | — (a design fact for T-006 design 3) | `WinClipboard` opens with its own hidden window instead of `None` |
| `session` / `station` / `desktop` / `foreground_lock` / `image` | interpretation only: session 0, a station other than WinSta0, or an input desktop that is not the thread's predicts the rows above failing; `image` dates the facts | no line of its own; recorded |
| `taskbar` | T-052 test 8 may assert `rect().is_some()` (today a captured eprintln in `tray.rs`, which never reaches the log) | T-052 shows the icon only through `tray_by_id` and the smoke's `tray_icon_app` window; the visible icon is the owner's |

T-052's `lifecycle.rs` (real Wry, `any_thread`, `run_return`) needs none of these. Creating windows in the runner's session is already shown by the install smoke.

## Observed runs

Run A is recorded below (T-059 technical-writer commit). The push that carries this commit gives run B, which must re-check it; the verify record checks this table against both runs.

| Run id | Date | Commit | `image` | Fact lines (verbatim, in order) |
|---|---|---|---|---|
| A: 37255557467 (windows job 111593430791) | 2026-10-05 | 2e414bf | `os=win25-vs2026`, `version=20260925.250.1` | see below |
| B | pending | | | |

Run A lines (`gh run view 37255557467 --job 111593430791 --log`, step "Runner capability probe", checked 2026-10-05):

```text
runner-probe: image=ok(os=win25-vs2026,version=20260925.250.1)
runner-probe: session=ok(id=2,console=2,remote=0)
runner-probe: station=ok(name=WinSta0,visible=1)
runner-probe: desktop=ok(thread=Default,input=Default)
runner-probe: foreground_lock=ok(ms=2147483647)
runner-probe: taskbar=ok
runner-probe: foreground=ok(ms=2,before=CASCADIA_HOSTING_WINDOW_CLASS,created_fg=1,set=1,focus=self)
runner-probe: sendinput=ok(ms=23,inserted=2,keydown=1,char=0x61,text_len_before=19,text_len=20,fg=1)
runner-probe: clipboard=ok(ms=2,open_tries=1)
runner-probe: clipboard_null_owner=ok(ms=0,open_tries=1)
runner-probe: hotkey=ok(ms=1,inserted=3,fg=1)
runner-probe: async_keys=ok(ms=1,up_ms=0,down=ctrl:1.alt:1.space:1,up=ctrl:1.alt:1.space:1,released=5,fg=1)
runner-probe: end(facts=12)
```

Reading of run A: every capability is `ok`. The probe ran in interactive session 2 (console 2, not remote) on `WinSta0` with input desktop `Default`; a Windows Terminal window (`CASCADIA_HOSTING_WINDOW_CLASS`) was in front before the probe window; the probe window became foreground in 2 ms and focus was `self`; the injected `A` reached the EDIT (`WM_KEYDOWN`, `WM_CHAR` 0x61, text 19 to 20 characters); both clipboard variants worked on the first open; the hotkey fired in 1 ms; `GetAsyncKeyState` saw all three keys down and up. `foreground_lock` 2147483647 is the raw setting value; no test depends on it.

Consequence today (the table above applied to run A alone): foreground, `SendInput`, `RegisterHotKey` / `WM_HOTKEY`, `GetAsyncKeyState`, clipboard and notification area (`taskbar`) are all `ok`, so the Acceptance lines in the "stays on Windows CI" column may be asserted on CI: T-006 tests 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 18, 19 and smoke 24 (T-006 Acceptance line 1 and line 3's "start window replaced"), T-057 Acceptance line 1 (target in front, no `WM_KILLFOCUS`), and T-052 test 8 (`rect().is_some()`). Rule: per the invariant a capability counts only when `ok` in every recorded run, at least A and B. Until run B (the next CI run, the push of this commit) shows the same facts, no test may rely on them and the Acceptance lines are not reworded. A fact that differs in run B counts as missing.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| No `#[ignore]`: the probe runs captured in the `cargo test -p voicen` step and again in its own step | runs twice; the first run changes global state (clipboard, foreground) and shows nothing | T-059 option B |
| A PowerShell probe in `ci.yml` with `Add-Type` P/Invoke, like `visible-windows.ps1` | measures a pwsh process started by the runner, not a cargo test exe, and foreground rights depend on the process | T-059 option C |
| A `harness = false` `[[test]]` table that prints without libtest | touches the guarded manifest grammar (`docs/decisions/ci-toolchain.md`); not needed with `--no-capture` | T-059 option D |

## Open

- `capture_endpoints` (`waveInGetNumDevs`, `Win32_Media_Audio`) is not probed: the orchestrator decided against it (T-059 Notes, Q1), because T-006's CI tests inject audio.
- T-052's dev-only `[target.'cfg(windows)'.dev-dependencies] windows` line (`Win32_UI_WindowsAndMessaging`) duplicates a feature that the release line now declares; T-052's review decides whether it stays.
