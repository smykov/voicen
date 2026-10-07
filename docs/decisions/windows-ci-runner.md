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

Before T-059 nothing had observed any of this on the runner. The install smoke's window census reads only window existence, visibility, owner, ex-style, class and title (`scripts/ci/visible-windows.ps1`); since T-006 Refresh 5 two further smoke steps (the "hotkey hold" steps, `scripts/ci/hotkey-hold.ps1`) inject Ctrl+Alt+Space with `SendInput`, read it with `GetAsyncKeyState` and fire the app's real `WM_HOTKEY`, so they depend on the `sendinput`, `hotkey` and `async_keys` facts. Their loud preconditions: "inserted n of 3" (`SendInput`), "did not read down within 5 s" and "still read down 5 s after the key-ups" (`GetAsyncKeyState`), "premise: Ctrl+Alt+Space is not free before the launch" and "does not hold Ctrl+Alt+Space within 15 s" (`RegisterHotKey`). Every other shell test runs on tauri's mock runtime or calls no window API. And libtest swallows what a passing test prints, so a fact printed from the normal `cargo test -p voicen` step never reaches the job log.

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
- **Exception — a fact re-measured in every job** (orchestrator, 2026-10-05, T-006 Refresh 3): a probe fact may back a test precondition from its first run, because the probe step runs in every windows job and so re-measures it next to the tests that rely on it. Such a precondition fails loudly through `win32_support::precondition_rechecked`, whose message points at the same job's `runner-probe: <fact>=…` line instead of runs A and B. Today this applies to `foreground_again` only. Its consequence row applies as soon as a recorded run shows it missing.
- **Don't:**
  - assert a capability in a Windows CI test because "it worked on my PC" or because a third party's run says so (runner-images issue 14049 is a forecast only);
  - read a capability from the install smoke's window census, which never reads foreground, focus, input, hotkeys or the clipboard (the two hold steps use `sendinput`, `hotkey` and `async_keys` but measure none: they are dependents of those facts, not evidence for them);
  - read `foreground` as "any window of the test exe can be brought to the front": it is measured on the exe's first window, which has foreground rights from its start. A later window needs the injected-Alt path of `foreground_again` (T-006 verify 1: paste.rs's later tests got `set=0` with Windows Terminal in front, `foreground_lock` being infinite).

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
  - `foreground_again` destroys the first window (after emptying the clipboard), creates a second `EDIT` window on the same thread and destroys it the same way; its Alt key-up and the mask key are armed in a `Release` guard, and the probe waits until Alt reads up;
  - in the hotkey probe, keyboard messages are taken from the queue but not dispatched, because Alt+Space dispatched to the window could open its menu, a modal loop;
  - `CloseClipboard` runs on every path through the `Opened` guard. A block the clipboard took is never freed. `GlobalUnlock`'s `Err` with code 0 (the last unlock, windows-result 0.4.1) counts as success;
  - the clipboard holds only the fake text `voicen-probe-<pid>`.

### A CI step plants and observes Credential Manager entries only through `scripts/ci/credentials.ps1`

- **Defect that produced it:** T-061 verify 3 evidence, run 37444780779. The `--purge-credentials` step planted with `cmdkey /generic:` and decided "planted" from cmdkey's filtered `/list` text, with the exit code and output discarded. It failed with `premise: could not plant Voicen/ci-purge-a`, `voicen.exe` never ran, and the log could not say whether the write or the read-back failed (class `windows-premise-unchecked-on-host`, since T-066 `ci-premise-unchecked-before-main`).
- **What breaks if you violate it:** a premise read from another tool's text, written with another writer than the product's, goes red or green for reasons outside the product (e.g. a stored TargetName the product's `CredEnumerateW "Voicen/*"` would not list).
- **Where it is enforced:** `scripts/ci/credentials.ps1` (dot-sourced; `Add-VoicenCredential` = `CredWriteW` GENERIC, LOCAL_MACHINE, user `voicen`; `Test-VoicenCredential` = `CredReadW` GENERIC; `Get-VoicenCredentials` = `CredEnumerateW "<prefix>*"`; `Remove-VoicenCredential` = `CredDeleteW` GENERIC; every failed call throws with its Win32 error, `ERROR_NOT_FOUND` is "none" where the API means it). Make check `check-ci-credentials` (`scripts/ci/ci-credentials.test.sh` → `scripts/ci/ci-credentials.sh`; invariant form since T-066, T-061 review 5 #2) refuses a non-comment line of a workflow or of any file at the top of `scripts/ci` (other than the helper and the tripwire's own two files) that names a Credential Manager entry point: a Win32 `Cred<Upper>...` call, `cmdkey`, `vaultcmd`, `keymgr`, `PasswordVault` or the `CredentialManager` module; and a helper that does not declare the four calls. So step 18 names only the helper's functions, in its messages too. Whether the helper's calls work on the runner is not modelled on the host: the `wip/<ID>` run shows it before review (next entry). T-025's uninstall steps use the same helper.

### A runner premise is run on a `wip/<ID>` run before review

- Pointer: the invariant lives in `docs/decisions/ci-toolchain.md` › "Every CI-only premise runs on a `wip/<ID>` run before review" (P-016, rca T-066, decisions #90). For this area: a new Windows CI test, fixture or `scripts/ci` helper is first run on `wip/<ID>`, never first on `main`; runner facts are re-measured on `wip` runs too (the probe step runs in every run of `ci.yml`, whatever the ref), and a fact that differs in a `wip` run counts as missing, as for a `main` run.

## Line grammar

One line per fact, in this order: `image`, `session`, `station`, `desktop`, `foreground_lock`, `taskbar` (session thread), then `foreground`, `sendinput`, `clipboard`, `clipboard_null_owner`, `hotkey`, `async_keys`, `foreground_again` (window thread; `foreground_again` added by T-006 verify 1, so runs A and B have 12 lines and later runs 13). Then the end line.

```text
^runner-probe: (?<fact>[a-z_]+)=(?<status>ok|denied|lost|absent|skipped|timeout)(\((?<detail>[^()]*)\))?$
runner-probe: end(facts=N)
```

- The detail is `k=v` pairs joined by `,`. Any character of a key or value outside `A-Za-z0-9_.:-` is printed as `_`.
- OS codes are printed as `err=0xHHHHHHHH` (`windows::core::Error::code()`). The `ok` lines of the seven window-thread facts (`foreground` … `foreground_again`) carry a measured `ms`; the session-thread facts carry none, and `foreground_lock`'s `ms` is the system setting (`SPI_GETFOREGROUNDLOCKTIMEOUT`), not a time.
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
| `foreground_again` | What a later test of the same exe meets: the first probe window is destroyed, then `back` (class in front after up to `WAIT`, `none`) and `back_other` (1 = a window of another process is in front, e.g. Windows Terminal); a second `EDIT` window is created (`created_fg`), Alt is injected and read down (`alt`), `SetForegroundWindow` (`set`), `ms` (from `SetForegroundWindow` until the second window is in front), `focus` (as for `foreground`), then 0xE8 down/up and Alt up (`released`, the inserted count of those 3 events); `now` when not `ok`. `lost` = `set=1` but never in front, `denied` = `set=0`, or `denied(step=sendinput,err=…)` when the Alt key-down was not inserted. This is the path `win32_support::bring_to_front` takes for every T-006 test window |
| `async_keys` | `GetAsyncKeyState` of Ctrl, Alt, Space while held and after the release (0xE8 down/up, Space↑ Alt↑ Ctrl↑): `ms` (slowest key read down), `up_ms` (slowest read up), `down` / `up` (`ctrl:1.alt:1.space:1`), `released` (inserted count of the 5-event release batch), `inserted` when `lost`, `err` when `denied`, `fg` (`GetAsyncKeyState` may read 0 while another process's thread is in front). Sent also when the `hotkey` registration failed: see `hotkey` |

## Consequences per outcome

T-006 test numbers are those of the T-006 Refresh table. The orchestrator rewords the follow-up Acceptance from this table.

| Probe fact | `ok` in every recorded run → stays on Windows CI | missing (any status but `ok`, or different between runs) → |
|---|---|---|
| `foreground` (with `focus=self`) | T-006 tests 5, 11, 12, 13, 18, 19, which carry T-006 Acceptance line 1 and line 3's "start window replaced"; T-057 Acceptance line 1 (target in front, no `WM_KILLFOCUS`) | T-006: line 1 moves to the owner (owner step 2 already pastes into Notepad). Line 3's replaced/elevated branch stays on CI only through `delivery.rs` with a `Paster` wrapper (tests 20–22 unaffected); the real replacement is owner step 5. T-057 line 1: if `foreground.before` shows a window, CI asserts "`GetForegroundWindow()` unchanged through Recording, Processing and Message, and never the overlay", and `WM_KILLFOCUS` / "target stays foreground" move to the owner (line 3). With no window in front at all, line 1 is the owner's |
| `foreground_again` (re-measured in every job: see the exception above) | every T-006 test that brings a test window to the front through `bring_to_front`: tests 3, 4, 5, 11, 12, 13, 14, 15, 18, 19, 20 (Acceptance line 1 and line 3's "start window replaced") | as the `foreground` row: line 1 moves to the owner (owner step 1 already pastes into Notepad); line 3's replaced/elevated branch stays on CI only through `delivery.rs` with a `Paster` wrapper; the real replacement is the owner's |
| `sendinput` | T-006 tests 3, 4, 5, 12, 15, 18; the install smoke's two hotkey-hold steps | those move to the owner (steps 2 and 4; the design-4 risk). Tests that call session inputs directly (19–23) are unaffected. The hold steps have no owner substitute: they go red, and with them every `deploys: true` task and the main Telegram send, until the steps are reworked |
| `hotkey`: registration | T-006 tests 6, 7, smoke 24, the hold steps' hotkey-free premise | `denied(step=register,err=0x80070581)`: the tests take another combination through `win32_data` (product unchanged), recorded here. Any other code moves them to the owner |
| `hotkey`: `WM_HOTKEY` from injected keys | T-006 tests 3, 4, 5, 18; the install smoke's two hotkey-hold steps | tests move to the owner (step 2); the hold steps go red as for `sendinput` |
| `async_keys` | T-006 tests 3, 4 (release poll), 15 (modifier wait); the install smoke's two hotkey-hold steps (press and release read) | tests move to the owner (steps 2 and 4); the hold steps go red as for `sendinput` |
| `clipboard` | T-006 tests 8, 9, 10, 12, 13, 18, 19; line 1's "clipboard holds it, excluded from history". Tests 9 and 10 also need a hold that refuses a second open: the test's `ClipboardHolder` opens with a message-only window of its own thread, like this fact (a NULL-owner hold did not refuse a second NULL open of the same process, T-006 verify 1), and each test asserts the refusal as a loud precondition before it writes; contention is not a probe fact | moves to the owner (Win+V in step 2) |
| `clipboard_null_owner` | — (a design fact for T-006 design 3) | `WinClipboard` opens with its own hidden window instead of `None` |
| `session` / `station` / `desktop` / `foreground_lock` / `image` | interpretation only: session 0, a station other than WinSta0, or an input desktop that is not the thread's predicts the rows above failing; `image` dates the facts | no line of its own; recorded |
| `taskbar` | T-052 test 8 may assert `rect().is_some()` (today a captured eprintln in `tray.rs`, which never reaches the log) | T-052 shows the icon only through `tray_by_id` and the smoke's `tray_icon_app` window; the visible icon is the owner's |

T-052's `lifecycle.rs` (real Wry, `any_thread`, `run_return`) needs none of these. Creating windows in the runner's session is already shown by the install smoke.

## Observed runs

Run A is recorded below (T-059 technical-writer commit); run B re-checked it (T-059 validation 1). "Agree" means the same status and the same non-timing values for every fact; the `ms` and `up_ms` timings are not compared.

| Run id | Date | Commit | `image` | Fact lines (verbatim, in order) |
|---|---|---|---|---|
| A: 37255557467 (windows job 111593430791) | 2026-10-05 | 2e414bf | `os=win25-vs2026`, `version=20260925.250.1` | see below |
| B: 37257516843 (windows job 111599179675) | 2026-10-05 | f647d45 | `os=win25-vs2026`, `version=20260925.250.1` | the same 12 facts, all `ok`; only the timings differ from run A (see "Reading of run B") |

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

Reading of run B (`gh run view 37257516843 --job 111599179675 --log`, step "Runner capability probe", checked 2026-10-05): same image and same values as run A for every fact. Timings differ: `foreground` ms=4, `sendinput` ms=879 (A: 23), `hotkey` ms=1026 (A: 1), `async_keys` ms=1026 (A: 1), `clipboard` ms=1. Windows CI budgets for `SendInput`, `WM_HOTKEY` and `GetAsyncKeyState` are sized for about 1 s and more (waits of 3 s or more; holds timed from the observed press, not from `SendInput`; F-005).

Run B lines (verbatim, same step):

```text
runner-probe: image=ok(os=win25-vs2026,version=20260925.250.1)
runner-probe: session=ok(id=2,console=2,remote=0)
runner-probe: station=ok(name=WinSta0,visible=1)
runner-probe: desktop=ok(thread=Default,input=Default)
runner-probe: foreground_lock=ok(ms=2147483647)
runner-probe: taskbar=ok
runner-probe: foreground=ok(ms=4,before=CASCADIA_HOSTING_WINDOW_CLASS,created_fg=1,set=1,focus=self)
runner-probe: sendinput=ok(ms=879,inserted=2,keydown=1,char=0x61,text_len_before=19,text_len=20,fg=1)
runner-probe: clipboard=ok(ms=1,open_tries=1)
runner-probe: clipboard_null_owner=ok(ms=0,open_tries=1)
runner-probe: hotkey=ok(ms=1026,inserted=3,fg=1)
runner-probe: async_keys=ok(ms=1026,up_ms=0,down=ctrl:1.alt:1.space:1,up=ctrl:1.alt:1.space:1,released=5,fg=1)
runner-probe: end(facts=12)
```

Consequence today (the table above applied to runs A and B): foreground, `SendInput`, `RegisterHotKey` / `WM_HOTKEY`, `GetAsyncKeyState`, clipboard and notification area (`taskbar`) are all `ok`, so the Acceptance lines in the "stays on Windows CI" column may be asserted on CI: T-006 tests 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 18, 19 and smoke 24 (T-006 Acceptance line 1 and line 3's "start window replaced"), T-057 Acceptance line 1 (target in front, no `WM_KILLFOCUS`), and T-052 test 8 (`rect().is_some()`). Rule: per the invariant a capability counts only when `ok` in every recorded run, at least A and B. Runs A (37255557467, 2e414bf) and B (37257516843, job 111599179675, f647d45) agree on all 12 facts (same status and non-timing values, same image), so every capability counts as `ok` in every recorded run and the "stays on Windows CI" column applies: no Acceptance line moves to the owner for a probed capability (T-006 Q6 resolved). A fact that differs in a later run counts as missing.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| No `#[ignore]`: the probe runs captured in the `cargo test -p voicen` step and again in its own step | runs twice; the first run changes global state (clipboard, foreground) and shows nothing | T-059 option B |
| A PowerShell probe in `ci.yml` with `Add-Type` P/Invoke, like `visible-windows.ps1` | measures a pwsh process started by the runner, not a cargo test exe, and foreground rights depend on the process | T-059 option C |
| A `harness = false` `[[test]]` table that prints without libtest | touches the guarded manifest grammar (`docs/decisions/ci-toolchain.md`); not needed with `--no-capture` | T-059 option D |

## Open

- `capture_endpoints` (`waveInGetNumDevs`, `Win32_Media_Audio`) is not probed: the orchestrator decided against it (T-059 Notes, Q1), because T-006's CI tests inject audio. So no fact says the runner has a capture endpoint: T-006's default-device capture test (test 17: frames with rate and channels, none after `stop`; NFR-02, "the microphone opens on press only") is not a CI assertion and is the owner's manual check. The error map (test 16) stays in CI.
- T-052's dev-only `[target.'cfg(windows)'.dev-dependencies] windows` line (`Win32_UI_WindowsAndMessaging`) duplicates a feature that the release line now declares; T-052's review decides whether it stays.
