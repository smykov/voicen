# Specification Quality Checklist: Call Mode 1a — Record and Read

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-10-22
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- Clarifications resolved by the owner 2026-10-22 (spec › Clarifications): FR-043 → a command-line switch `voicen.exe --call record` to the running instance, also user-facing, indicator always shown, the "any local process can start a recording" trade-off noted (decisions #184); FR-047 → a pinned ≥ 30 min LibriSpeech test-clean subset (CC BY 4.0), read speech noted as optimistic (decisions #185, OQ-65 (a)). Added with the answers: US1 scenario 13, FR-003 switch sentence, an Assumptions line for the switch when Voicen is not running.
- Deliberate exception to "no implementation details": the approved requirements themselves fix the stored artefacts and the wire behaviour this feature must honour — file names and formats (`mic.opus`, `system.opus` Ogg Opus mono 16 kHz, `meta.json`, `transcript.json` schema 1, `transcript.md`), 16 kHz mono WAV chunk uploads, `verbose_json` → `json` fallback, WASAPI loopback of the default console device, `WM_ENDSESSION`, `/S`, `/P`, `/PURGECALLS`, `%LOCALAPPDATA%\Voicen\calls` (requirements FR-28, FR-38, FR-47, FR-60, §7, §8). The spec names them because the requirements do; it chooses no crate, window mechanism, IPC shape or module split (plan, OQ-45, OQ-48).
- SC-003, SC-004 and SC-011 name providers, models and call apps because NFR-13, CA-7 and NFR-19 define the measurement by them; they are outcomes, not implementation.
- Traceability: every user story scenario and every spec FR cites its requirement IDs (req FR-NN / NFR-NN, CA-N, decisions #N, `(v6, r3#N)` / `(v7, r4#N)`). 1a list coverage: FR-30…FR-42, FR-45…FR-52, FR-59 (storage), FR-60, FR-63…FR-65, FR-67, FR-71…FR-73, FR-75…FR-77 (FR-74 is 1c by its own row, see Assumptions), FR-80 (1a items, FR-040), FR-81…FR-84; NFR-13 (API), NFR-15…NFR-19, NFR-21, NFR-22; amendments FR-22, FR-28, NFR-02, NFR-06; T-102 and T-007 as dependencies.
- Must failure branches in scope, each in a scenario: FR-30 (US1 8–11), FR-33 (US9 2–3), FR-37 (US1 4), FR-38 (US4 2–4), FR-39 (US6 4–5), FR-41 (US6 2), FR-42 (US6 3–4), FR-45 (US2 7), FR-46 (US6 6), FR-47 (US4 1, 5), FR-48/FR-65 (US5 1), FR-49 (US4 1, 4), FR-50 (US4 6–8), FR-60 (US2 4, 8), FR-71 (US3 2, US8 6), FR-72 (US3 4), FR-73/FR-75 (US8 1–2), FR-76 (US3 6), FR-77 (US8 4–5), FR-80 (US9 4), FR-81 (US5 6), FR-82 (US2 11), FR-83 (US5 2), FR-84 (Edge Cases; FR-20 branch), FR-28 (US10 2).
- Validation iteration 1: all items except the clarification item passed; no spec change needed beyond the FR-74 assumption added during the check.
- Validation iteration 2 (after the owner's answers): all items pass; 0 [NEEDS CLARIFICATION] markers remain.
