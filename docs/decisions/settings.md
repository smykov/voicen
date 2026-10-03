# Settings and keys (core types and pure rules)

**Code:** `crates/voicen-core/src/{secrets,hotkey_registrar,models}.rs`, `src/settings/{mod,url,validate,gate,hotkey}.rs`, `src/post_process/settings.rs` (T-003); `SettingsFile` and `SettingsService` are added by T-032 · **Tests that pin it:** `settings::tests::{missing_fields_take_defaults, settings_json_has_no_key_field}`, `secrets::tests::{key_slot_target_names, key_edits_debug_never_shows_a_key, key_presence_is_per_slot_and_serializes_only_booleans}` and the `compile_fail` doctest in `secrets.rs`, `hotkey::tests::{every_key_and_modifier_combination_round_trips, rejects_invalid_strings}`

Tasks: T-003, T-032. Contract: `specs/004-settings-and-first-run/{data-model.md,contracts/core-traits.md}`. Decisions: #17, #21, #23, #25, #26, #27, #28.

## Invariants

### A key can never reach the settings file or a window

- **Defect that produced it:** none yet (found in planning, T-003). Specs 001–003 each planned their own key port (`CredentialStore`/`SecretString`, `SecretStore`, `SecretRef`) and their own Credential Manager impl beside spec 004's, so a key could travel through several unreviewed paths (P-010, P-011); decision #20.
- **What breaks if you violate it:** an API key in `settings.json`, a `SettingsView`, a log or a debug print (NFR-04, FR-20).
- **Where it is enforced:** `Settings` has no key field; `Secret` has no `Serialize` (compile-fail doctest), prints `***` and is zeroed on drop; windows get only `KeyPresence` booleans; one port `secrets::CredentialStore` over `KeySlot`; only `SettingsService` (T-032) writes or deletes.
- **Don't:** add a key field to `Settings`, derive `Serialize` on `Secret`, or declare a second key trait in a feature.

### Defaults come from one function, and the serde default is container-level

- **Defect that produced it:** none yet (found in analysis, T-003). A field-level `#[serde(default)]` fills a missing field with the field type's `Default` (serde_derive 1.0.229, `de.rs`), so an older file without `auto_paste` loaded `false`, without `history.size` loaded `0` (invalid) and without `api.base_url` loaded `""`. Spec 001 also planned its own defaults and loader.
- **What breaks if you violate it:** a file written by an older version silently changes behaviour or fails validation; two defaults sources diverge (P-010).
- **Where it is enforced:** `settings::defaults(os_tag)` is the only builder; `Settings` and each nested struct use a container-level `#[serde(default = "…")]` that calls it; `missing_fields_take_defaults` covers `{}`, `{"schema_version":1}` and partial nested objects.
- **Don't:** put `#[serde(default)]` on a field, give a settings struct `#[derive(Default)]` for loading, or build defaults in a consumer.

### The consumer owns the shared trait

- **Defect that produced it:** none yet (found in `/speckit-analyze`, decisions #20, #21). Each spec planned its own port for the same concern, with dependency cycles T-003 → T-016 → T-004 → T-003 and T-003 → T-006 → T-003.
- **What breaks if you violate it:** parallel ports, parallel impls, and a task that cannot start before the task that waits on it.
- **Where it is enforced:** the settings core defines `CredentialStore`, `HotkeyRegistrar`, `DownloadedModels` and `post_process::settings`; 001, 002 and 003 implement or read them; fakes are public behind feature `test-fakes` (decisions #23 N4).
- **Don't:** declare another key, hotkey or model-list trait in a feature crate or module, or enable `test-fakes` in a release build.

### The hotkey grammar is strict and closed

- **Defect that produced it:** none yet (found in T-003 review of the contract, decisions #25, #26). The first contract added numpad operator keys that research R-6 does not contain.
- **What breaks if you violate it:** two spellings of one hotkey in the file, UI and logs (P-010), or a key the shell cannot map to a virtual-key code.
- **Where it is enforced:** `settings::hotkey::parse_hotkey` accepts the canonical text only (exact case, order `Ctrl`, `Alt`, `Shift`, `Win`, each once, key last) from the closed 82-key set; the rest is `hotkey.invalid`; `every_key_and_modifier_combination_round_trips`, `rejects_invalid_strings`.
- **Don't:** accept lowercase or reordered input and normalize it, or add keys outside R-6 without changing the research and the spec.

### A key cannot enter through a base URL

- **Defect that produced it:** none yet (found in T-003 review round 1, decision #27(2)). A base URL such as `https://user:secret@host/v1` is well-formed, so it passed `url.malformed` and would have been stored in `settings.json` and sent to logs and windows.
- **What breaks if you violate it:** key bytes in the settings file, a `SettingsView` or a log line, bypassing the credential store (NFR-04, FR-20).
- **Where it is enforced:** `settings::url::check_base_url` returns `UrlError::Credentials` for a non-empty username or password, and `validate` maps it to `url.credentials`; a query string is allowed but never logged.
- **Don't:** accept userinfo in a base URL, strip it silently, or log a base URL with its query.

### One language list

- **Defect that produced it:** none yet (found in T-003 review round 1, decisions #27, #28). The contract said "a code from the Whisper language list" without naming it, so the validator, the UI picker and the engines could each carry their own list, and Whisper's `jw` is not ISO 639-1 (`jv`).
- **What breaks if you violate it:** the UI offers a language the core refuses, or an engine receives a code it rejects (P-010).
- **Where it is enforced:** `settings::WHISPER_ISO_639_1` is the only list (Whisper `LANGUAGES` minus `jw`, `haw`, `yue`); `validate` gives `language.unsupported` (`engine.speech_language`) for anything else, case-sensitive, for every engine.
- **Don't:** copy the list into the UI or an engine crate, or accept a code by case-folding it.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Field-level `#[serde(default)]` per field | fills with the type's `Default`, not `defaults()` | analysis T-003, hypothesis 4 |
| Each feature plans its own key/settings port | duplicate seams, cycles | decisions #20, #21 |
| Hand-written URL parser | R-8 requires the `url` crate | decisions #17 |
| `tokio::sync::watch` for `subscribe` | no tokio in core | decisions #22 |

## Open

- None.
