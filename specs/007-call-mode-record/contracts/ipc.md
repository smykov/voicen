# Contract: Call IPC (UI ↔ `src-tauri`), windows and message catalog

The UI talks to Rust only through these commands and events, and Playwright mocks exactly these (`e2e/support/tauriMock.ts`). Every wire shape has a fixture under `e2e/fixtures/` that a core test pins (`calls-wire`, `call-indicator-wire`; the settings-wire precedent). Wire field names are camelCase. One IPC module per contract: `src/lib/calls/callsApi.ts` and `src/lib/callIndicator/indicatorApi.ts`.

## Windows

| Label | Route | Created | Focus | Product window (census) |
|---|---|---|---|---|
| `calls` | `/calls` | on tray "Calls", a "Transcript ready" → Open, or the indicator's "Open Calls" | normal window | yes |
| `call-indicator` | `/call-indicator` | hidden at the first Record, shown at each Record start, hidden at stop | shown without activation (research R12) | yes, never in `helper-windows.txt`, never shown at app start |
| `settings` (exists) | `/settings` | — | — | yes; gains the Calls tab |

Each window has its own label, capability file and opener thread (settings-ui.md).

## Wire types

```ts
type CallState = "recording" | "queued" | "transcribing" | "ready" | "failed" | "recovered" | "damaged";
type FailReason = "key" | "network" | "timeout" | "provider" | "storage" | "engineNotSetUp";
interface CallSummary { id: string; root: string; title: string; startedAt: string; durationMs: number | null;
                        state: CallState; failReason?: FailReason }
interface CallList { calls: CallSummary[]; unavailableRoots: string[] }   // one entry per earlier or current root that is not available (FR-033)
interface Speaker { id: "me" | "s1"; name: string }
interface Line { startMs: number; endMs: number; track: "mic" | "system"; speaker: "me" | "s1"; lang: string | null; text: string }
interface CallDetail { summary: CallSummary; speakers: Speaker[]; lines: Line[] }
type CallError = { code: "notFound" | "inUse" | "emptyName" | "recordingCall" | "ioFailed"; file?: string; reason?: string };

// indicator events (Rust → indicator), at most 10 Hz for levels
interface IndicatorTick { elapsedMs: number; micLevel: number; systemLevel: number; micSilent: boolean; systemSilent: boolean }
interface IndicatorWarning { id: number; kind: "communicationsDevice" | "trackLost" | "micChanged" | "cantHearOtherParty"
                             | "cantHearYou" | "lowDisk" | "providerNotice"; text: string }   // text rendered in Rust
interface IndicatorState { recording: boolean; reminderVisible: boolean; reminderText: string; headphonesHint: string;
                           warnings: IndicatorWarning[] }
```

## Commands

| Command | Args | Returns | Errors | Spec |
|---|---|---|---|---|
| `calls_list` | — | `CallList` | — | FR-033 |
| `calls_get` | `{root, id}` | `CallDetail` | `notFound`, `ioFailed` | FR-034 |
| `calls_rename` | `{root, id, title}` | `CallSummary` | `emptyName`, `ioFailed` | FR-035 |
| `calls_rename_speaker` | `{root, id, speaker, name}` | `CallDetail` | `emptyName`, `ioFailed` | FR-035 |
| `calls_transcribe` | `{root, id}` | `CallSummary` (now `queued`) | `recordingCall`, `notFound` | FR-017, FR-027 |
| `calls_copy_markdown` | `{root, id}` | `null` | `ioFailed` | FR-036 |
| `calls_export` | `{root, id, format: "md" \| "json"}` | `{saved: boolean}` (false = dialog cancelled) | `ioFailed{reason}` | FR-036 |
| `calls_open_folder` | `{root, id}` | `null` | `ioFailed` | FR-033, FR-036 |
| `calls_delete` | `{root, id}` | `null` | `recordingCall`, `inUse{file}`, `ioFailed` | FR-037 |
| `call_indicator_stop` | — | `null` | — | FR-005 |
| `call_indicator_dismiss` | `{warningId}` | `null` | — | FR-006 |
| `call_indicator_moved` | `{x, y}` | `null` | — | FR-005 |
| `call_indicator_state` | — | `IndicatorState` | — | FR-005 (on load) |

Paths cross the wire only as opaque `root` strings that Rust returned in `calls_list`. Every command checks that `root` is the current or a remembered earlier root and that `id` is a folder name of the call-id grammar. Anything else is `notFound`, never a filesystem access (P-009).

Settings commands are unchanged: the `calls` section is part of the existing settings wire (`get_settings` / `save_settings`, settings-wire fixture extended). Save errors add `FieldId`s `calls.hotkey` (code `hotkey.equals_dictation`; a registration conflict stays `hotkey.unavailable`), `calls.reminder`, `calls.primary_language` (unknown: `language.unsupported`), `calls.storage_root` (code `storage.unwritable`). `FieldError` gains an optional snake_case `reason`, present only for `storage.unwritable`, one of `missing`, `not_a_folder`, `relative`, `access_denied`, `disk_full`, `other`; omitted otherwise (decision #196, T-120).

## Events (Rust → UI)

| Event | Payload | Target |
|---|---|---|
| `calls://changed` | `{root, id}` | `calls` window (state change, rename, delete, pass progress) |
| `calls://open` | `{root, id}` | `calls` window (opened from a toast or the tray) |
| `indicator://tick` | `IndicatorTick` | `call-indicator` |
| `indicator://state` | `IndicatorState` | `call-indicator` |

## UI components

- `src/routes/calls/+page.svelte`:
  - list newest first, with title, date and time, duration and state;
  - "No calls yet" with how to start one; unavailable-root lines;
  - detail view: lines with time, speaker colour and label, text; search that highlights matches or shows "No matches";
  - actions: rename call and speakers inline, "Transcribe" / "Transcribe again", copy, export, open folder, delete with confirmation.
- `src/routes/call-indicator/+page.svelte`:
  - "● Recording" with a timer, two level bars, the Stop button;
  - the reminder and headphones hint for 10 s at start;
  - the warning list, each warning with a dismiss button;
  - drag region;
  - all controls are buttons with accessible names.
- Settings Calls tab (`SettingsTab::Calls`, added to `TABS`): call hotkey, reminder, primary language, storage folder (with the reason on refusal), and the provider line "the other party's audio goes to provider <host>" when the host is not loopback (FR-031, FR-040).

Playwright specs (`e2e/calls.spec.ts`, `e2e/call-indicator.spec.ts`, `e2e/settings-calls.spec.ts`) cover:
- every state badge, including "Queued", failed with each reason, recovered and damaged;
- the empty list; search with matches and with none;
- rename refusals; delete of a recording call and the in-use error;
- the indicator's warning list (add, dismiss, re-arm);
- the hotkey-equal refusal; an unwritable root;
- the provider line.

## Message catalog (EN + RU, `messages!`)

The texts live in the one catalog at the repository root, `i18n/en.json` and `i18n/ru.json` (decision #13); Rust-originated ids are declared in `messages!` in `crates/voicen-core/src/i18n.rs`, the only Rust code that parses and renders them; UI-only ids are `t()` literals. `i18n/conformance.json` is the shared render-rule fixture of the Rust and UI suites and gains no call ids.

| Id | English |
|---|---|
| `tray.call` / `tray.call.record` / `tray.call.stop` / `tray.calls` | Call / Record / Stop / Calls |
| `indicator.recording` | ● Recording |
| `indicator.stop` | Stop |
| `indicator.reminder` | Tell the other party the call is being recorded |
| `indicator.headphones` | Better with headphones |
| `indicator.dismiss` | Dismiss |
| `warning.communications_device` | Calls may use a different output device than the one being recorded |
| `warning.track_lost.mic` / `.system` | Microphone lost — recording silence / Output device lost — recording silence |
| `warning.mic_changed` | Microphone changed to the Windows default |
| `warning.cant_hear_other_party` | Can't hear the other party — check the output device |
| `warning.cant_hear_you` | Can't hear you — check the microphone |
| `warning.low_disk` | Less than 1 GB free in the calls folder |
| `warning.provider_notice` | The other party's audio goes to provider {host} (indicator kind `providerNotice`, FR-031) |
| `notice.call_refused.{mic,system,root}.{reason}` | Can't start recording: <part> — <reason> |
| `notice.call_saved` | Call saved — transcribing |
| `notice.transcript_ready` / `notice.open` | Transcript ready / Open |
| `notice.pass_failed.{reason}` | Transcription failed: <reason> |
| `notice.stopped.{sleep,max_length,low_disk,storage}` | Recording stopped: … |
| `notice.call_hotkey_conflict` | The call hotkey is in use by another program |
| `exit.recording.question` | A call is being recorded. Save it and exit? |
| `calls.title_default` | Call {day}.{month} {hour}:{minute} (each a two-digit value; the catalog allows only `{[a-z][a-z0-9_]*}` placeholders) |
| `calls.empty` | No calls yet — choose Call › Record in the tray or press {hotkey} |
| `calls.state.{recording,queued,transcribing,ready,failed,recovered,damaged}` | … ("Recovered after a crash") |
| `calls.failed.{key,network,timeout,provider,storage,engine_not_set_up}` | … ("Set up the API engine") |
| `calls.root_unavailable` | Folder not available: {root} |
| `calls.search` / `calls.no_matches` | Search / No matches |
| `calls.transcribe` / `calls.transcribe_again` / `calls.copy` / `calls.export_md` / `calls.export_json` / `calls.open_folder` / `calls.delete` / `calls.delete_confirm` | … |
| `calls.error.in_use` / `calls.error.empty_name` / `calls.error.recording` / `calls.error.io` | could not delete: {file} is in use / … |
| `speaker.me` / `speaker.other` | Me / Other party (RU: Я / Собеседник) |
| `settings.tab.calls` + one label per new `FieldId` | Calls / Call hotkey / Reminder / Primary language / Storage folder |
| `settings.calls.provider_notice` | The other party's audio goes to provider {host} |
| `error.storage.unwritable` (T-120, no placeholder) | An unwritable storage folder uses this id; there is no `settings.calls.storage_unwritable` (#198) |

RU texts are added with the EN ones in the same task; the existing catalog checks (`Catalog::parity_problems` and the ids-exist-in-both-catalogs tests in `i18n.rs`) enforce parity. Byte counts use `formatSize`.
