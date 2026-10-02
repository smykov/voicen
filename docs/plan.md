# Plan

<!-- What will be built, in which order, and how we know a stage is done.
     Requirements live in requirements.md; this file references them by FR-NN, it doesn't restate them. -->

## Goal

Release 1 of Voicen: a Windows 10/11 dictation tool, public on GitHub Releases, that pastes dictated text into the focused field — via an OpenAI-compatible API, built-in whisper.cpp or a local server, with optional LLM post-processing. Done when all three success criteria of `docs/requirements.md` §2 hold (speed per NFR-01, 2 weeks of daily use without a crash, installer works on a clean Windows 11). Date: OQ-01.

## Stages

| Stage | Scope (FR-NN) | Done when | Target date | Status |
|---|---|---|---|---|
| 0 — Skeleton | FR-18 | `make check` green; Windows CI builds, installs and launches the app with the commit in its log | 2026-10-02 (repo `smykov/voicen` exists, decisions #14; CI run 37035261228 on df52798 green for `gate` and `windows`) | in progress |
| 1 — Dictation via API | FR-01, FR-02, FR-03, FR-04, FR-05, FR-06, FR-10, FR-11, FR-12, FR-20, FR-21, FR-22, FR-24, FR-25, FR-13 (engine part) | hotkey → API transcript pasted into Notepad on the owner's PC; failures notified and retryable | OQ-01 | not started |
| 2 — Local engines | FR-07, FR-08, FR-17, FR-28 | offline dictation with a downloaded `small` model; local server works | OQ-01 | not started |
| 3 — Post-processing and robustness | FR-09, FR-23, FR-26, FR-27, FR-14, FR-29 | LLM step with fallback; ordering; sleep/resume and mic changes survive | OQ-01 | not started |
| 4 — Release polish | FR-15, FR-16, FR-19, NFR-01..NFR-12 checks | installer in GitHub Releases; NFR-01 benchmark and NFR-08 checklist pass | OQ-01 | not started |

## Out of scope (for now)

- macOS and Linux builds — requirements §3 no-gos.
- Provider-specific APIs other than OpenAI-compatible; automatic fallback between engines; GPU acceleration; streaming transcription.
- Code signing — OQ-02.

## Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R-1 | Windows behaviour cannot run on the Linux dev host | regressions reach users | Windows CI job; owner's manual checklist (NFR-08) |
| R-2 | Auto-paste unreliable in some apps | users lose trust | FR-10 fallback to clipboard; NFR-08 checklist |
| R-3 | Unsigned installer with hotkey + input simulation flagged by SmartScreen / antivirus | success criterion 3 fails | RegisterHotKey instead of a low-level hook; OQ-02 |
| R-4 | Local engine too slow on CPU | users think the app is broken | recommend `small`, warm model (NFR-03), progress indicator |
