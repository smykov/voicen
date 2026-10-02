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
