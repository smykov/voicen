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
