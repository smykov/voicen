# Decision log

<!-- Append-only. A new decision is a new row; old rows are never edited.
     A reversed decision gets a new row whose "Was" references the old #.
     "Task" is the task the decision came from (the rca task for a recurrence exit), or —.
     "Proposed by" is who suggested the option; "Decided by" is who chose (the owner decides forks,
     escalations, architecture reviews and amendments of PRINCIPLES.md). -->

| # | Date | Task | Question | Was | Decided | Why | Proposed by | Decided by |
|---|---|---|---|---|---|---|---|---|
| 1 | 2026-10-02 | — | Maximum recording length and audio format (OQ-04) | 5 min / 10 min / configurable | **10 min, 16 kHz mono WAV** | fits OpenAI's 25 MB upload limit (roast r1#12) | agent | owner |
| 2 | 2026-10-02 | — | Stack (requirements §9) | Tauri 2 + Svelte / Tauri 2 + React / .NET 8 WPF | **Tauri 2 + Svelte; areas core (Rust) and ui (TypeScript)** | size/RAM limits, in-process whisper.cpp, UI testable on the Linux host | agent | owner |
| 3 | 2026-10-02 | — | Serena languages | typescript + rust | **typescript only** | rust-analyzer needs a host Rust toolchain; Rust runs only in the `voicen-rust` Docker image (requirements §9 installs row). Rust code is navigated with built-in tools | agent | agent (follows #2 and requirements §9) |
| 4 | 2026-10-02 | — | GitHub repository and the Windows CI job (what "deployed" means) | create now / later | **later: a sprint-1 task** | the workflow is written now; its first green run needs the remote | agent | owner |
| 5 | 2026-10-02 | — | Where the Tauri shell (`src-tauri`) is checked | a local `core` command in a webkit image / Windows CI only | **Windows CI only** (`cargo test --workspace`, `tauri build`, install smoke); the local gate covers `voicen-core` and the UI | the shell is Windows-only; a Linux webkit build would test a platform the product does not ship | agent | agent (follows #2 and requirements §9) |
| 6 | 2026-10-02 | — | Main branch | master / main | **main** | matches the repository default | agent | owner |
| 7 | 2026-10-02 | — | Requirements gaps found by the specs (FR-13, FR-14, FR-15, FR-21, FR-24, FR-28, FR-29, NFR-03, NFR-07) | gaps left to the developer | **closed in requirements v4 as the specs proposed; built-in engine timeout 120 s** | specs 001–006 findings | agent | owner |
| 8 | 2026-10-02 | — | 30 clarify answers of specs 001–006 | provisional recommended answers | **all confirmed** | each listed in its spec's Clarifications | agent | owner |
| 9 | 2026-10-02 | — | New dependencies and the license policy | ask per task / consent now | **consent now**: reqwest, cpal, windows, whisper-rs, rubato, thiserror, serde_json, chrono, tauri-plugin-single-instance, tauri-plugin-opener; dev: wiremock, tokio; `cargo-about` in `voicen-rust` image. Accepted licenses: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, Unlicense, CC0-1.0 | each added in the task that needs it | agent | owner |
| 10 | 2026-10-02 | — | Uninstaller | stock Tauri checkbox (default off) / custom NSIS template | **custom NSIS template**: the FR-28 question defaults to yes; install folder stays `%LOCALAPPDATA%\Voicen` | FR-28 default yes | agent | owner |
| 11 | 2026-10-02 | — | whisper.cpp process model | in-process / child worker | **in-process** (requirements §9), with model size and header checks before load; a ggml abort is an accepted risk for release 1 | NFR-07 vs §9, spec 002 research R-8 | agent | owner |
| 12 | 2026-10-02 | — | Where whisper-rs is built | inside voicen-core / own crate | **own crate `crates/voicen-whisper`** behind the `SpeechModel` trait, built and tested on Windows CI | keeps the native build out of the Linux gate | agent | agent (follows #5) |
| 13 | 2026-10-02 | — | Message catalog location | `locales/` (spec 001) / `i18n/` (spec 004) | **`i18n/en.json`, `i18n/ru.json` at the repository root**, read by Rust (`include_str!`) and the UI | one catalog for shell and UI (P-010) | agent | agent |
| 14 | 2026-10-02 | T-002 | GitHub repository name taken by the earlier Python version | push to the old repo as `main` / new name / rename the old one | **old repo renamed to `smykov/voicen-python`; new public `smykov/voicen`** | keeps the old code and history, the product keeps its name | agent | owner |
| 15 | 2026-10-02 | T-002 | How T-002 confirms the Windows install facts of spec 006 (install dir, `uninstall.exe`, Start-menu shortcut) | (A) exact-path assertions in the CI smoke step + a push / (B) record only what run 37035261228 observed, leave the rest to T030 | **(A) exact-path assertions in the `windows` smoke step** | the green run proved only the log path and commit line; T030 builds on these paths and a wrong one would surface there mixed with uninstaller failures | agent (problem-investigator) | owner |
