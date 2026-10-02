# Requirements Quality Checklist: Local Transcription

**Purpose**: Unit tests for the requirements of feature 002 (completeness, clarity, consistency, measurability, coverage) before planning and task breakdown
**Created**: 2026-10-02
**Feature**: [spec.md](../spec.md)
**Depth / audience**: Standard; reviewer (PR) — defaults, the owner could not be asked

**Review Ownership**: This checklist is a reviewer-owned requirements-quality review artifact. Items marked `[x]` were resolved by an edit to spec.md in this session (the edit is named); open items need the owner or another feature.

## Requirement Completeness

- [x] CHK001 Are the five offered models named unambiguously, including which variant "multilingual" excludes? [Clarity, Spec §FR-001, Assumptions] — resolved: Assumptions define multilingual as the non-`.en` files.
- [x] CHK002 Is download progress specified by content and update frequency? [Clarity, Spec §FR-002] — resolved: bytes, total, percentage, at least once per second.
- [x] CHK003 Is "interrupted download" defined by observable conditions? [Measurability, Spec §FR-004] — resolved: connection/HTTP/disk error, 30 s without data, 5 s connect timeout.
- [x] CHK004 Is cancelling a running download specified? [Gap, Spec §FR-005] — resolved by Clarification 3.
- [x] CHK005 Is the behaviour on insufficient disk space specified? [Edge Case, Spec §FR-007] — resolved.
- [x] CHK006 Is cleanup of a partial file after the app exits or crashes mid-download specified? [Recovery, Spec §FR-008] — resolved.
- [ ] CHK007 Is integrity of an already-downloaded model re-checked before it is loaded (file altered after download)? [Gap, Spec §Edge Cases] — spec checks size only at load; a full SHA-256 re-check on each cold load is an owner trade-off (1–3 s extra on cold start).
- [ ] CHK008 Is a timeout or upper bound defined for built-in transcription? [Gap, req FR-24] — requirements define none; reported to the owner.
- [x] CHK009 Is "10 minutes idle" defined with a precise start and hold rule? [Clarity, Spec §FR-013] — resolved by Clarification 2.
- [x] CHK010 Are unload triggers other than the idle timer specified? [Completeness, Spec §FR-014] — resolved.
- [ ] CHK011 Is user feedback during a cold model load (several seconds for `small`) specified? [Gap, UX] — the overlay's processing state is owned by 001; no requirement says the user is told the model is loading.
- [ ] CHK012 Is memory use while a model is loaded bounded? [Gap, req NFR-03] — NFR-03 bounds idle RAM only with no model loaded.
- [x] CHK013 Are the strings this feature adds required in both UI languages? [Coverage, Spec §FR-025] — resolved by adding FR-025.
- [ ] CHK014 Is the licence of the downloaded model files listed per NFR-12? [Gap, req NFR-12] — the licence list is owned by 006; the whisper models' licence (MIT) must be included there.
- [ ] CHK015 Is download behaviour behind a proxy specified? [Gap] — not in the requirements; plan assumes system/env proxy settings are honoured where the HTTP client supports them.

## Requirement Clarity

- [x] CHK016 Is the 60 s local-server timeout specified as a total request timeout distinct from the 5 s connect timeout? [Clarity, Spec §FR-019] — resolved.
- [x] CHK017 Is the model name sent to a local server specified? [Ambiguity, Spec §FR-017] — resolved by Clarification 4.
- [x] CHK018 Is "available offline" made testable? [Measurability, Spec §FR-008, US1 scenario 7] — resolved.

## Requirement Consistency

- [x] CHK019 Is "engine becomes none" after deletion consistent with FR-21's hotkey-with-engine-none branch? [Consistency, Spec §FR-022] — yes; the next hotkey press follows req FR-21's failure branch (owned by 001/004).
- [ ] CHK020 Is the separate local-server model and key consistent with req FR-13's single "model" / "API key" settings list? [Consistency, req FR-13] — needs 004 to show per-engine fields; flagged for coordination.
- [x] CHK021 Are local-server failure messages consistent with the shared client's API-engine messages (req FR-06, FR-11)? [Consistency, Spec §US3] — resolved: same client, same reasons.

## Scenario and Edge Case Coverage

- [x] CHK022 Is deleting a model that is transcribing covered? [Edge Case, Spec §FR-023] — resolved.
- [x] CHK023 Is a retry of a pending built-in recording after the model was deleted covered? [Recovery, Spec §Edge Cases] — resolved.
- [x] CHK024 Is the hotkey pressed while the model is still loading covered? [Edge Case, Spec §Edge Cases] — resolved.
- [x] CHK025 Are several recordings in flight with the single built-in model ordered? [Coverage, Spec §FR-016] — resolved.
- [x] CHK026 Is an upstream change of a model file (pinned hash no longer matching) addressed? [Assumption, Spec §Assumptions] — resolved by pinning one repository revision.

## Non-Functional Requirements

- [ ] CHK027 Can SC-001 be measured while the reference machine is undefined? [Assumption, req NFR-01, OQ-03] — blocked on OQ-03.
- [ ] CHK028 Does "no network request" (NFR-05) account for traffic the app does not make itself (the WebView2 runtime's own update/SmartScreen traffic)? [Measurability, Spec §SC-003] — owner to confirm the requirement covers the app's own code only.
- [x] CHK029 Are logged events for download, load and unload defined without transcript, audio or key content? [Security, Spec §FR-024] — resolved.

## Notes

- Open items: CHK007, CHK008, CHK011, CHK012, CHK014, CHK015, CHK020, CHK027, CHK028.
