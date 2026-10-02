# Requirements Quality Checklist: Dictation via an OpenAI-compatible API

**Purpose**: Unit tests for the requirements of feature 001 (completeness, clarity, consistency, measurability, coverage) before planning and task breakdown
**Created**: 2026-10-02
**Feature**: [spec.md](../spec.md)
**Depth / audience**: Standard; reviewer (PR) — defaults, the owner could not be asked

**Review Ownership**: This checklist is a reviewer-owned requirements-quality review artifact. Items marked `[x]` were resolved by an edit to spec.md in this session (the edit is named); open items need the owner or another feature.

## Requirement Completeness

- [x] CHK001 Is the scope of the 30 s transcription timeout (connect, upload, response) defined? [Clarity, Spec §FR-018] — resolved by Clarification 1.
- [x] CHK002 Is behaviour specified when modifiers are still held after the 1 s wait? [Gap, Spec §FR-023] — resolved by Clarification 2: no paste, "copied — paste manually".
- [x] CHK003 Is the position of a retry in the delivery order specified? [Gap, Spec §FR-029] — resolved by Clarification 3.
- [x] CHK004 Is it specified whether a retry uses the settings at failure time or at retry time? [Gap, Spec §FR-027] — resolved by Clarification 4 (current settings).
- [x] CHK005 Is the overlay's state between stop and delivery specified? [Gap, Spec §FR-008] — resolved by Clarification 5.
- [x] CHK006 Is "release" defined for a multi-key hold hotkey when keys are released one by one? [Clarity, Spec §FR-003] — resolved: release of any key of the combination.
- [x] CHK007 Is the accepted Bluetooth start loss of req NFR-02 carried into the spec? [Completeness, Spec §FR-007] — resolved.
- [x] CHK008 Is the recording size at the 10-minute cap checked against the provider upload limit? [Edge Case, Spec §Assumptions] — resolved: ≈ 19.2 MB < 25 MB; a lower provider limit gives 413 → server error, audio kept.
- [x] CHK009 Is it specified whether the previous clipboard content is restored? [Gap, Spec §Assumptions] — resolved: not restored (text stays for manual paste, req FR-10).
- [ ] CHK010 Is the overlay's screen position (which monitor, where on it) specified? [Gap, Spec §FR-008] — not in the requirements; owner or UI design decision.
- [ ] CHK011 Is the behaviour behind an HTTP proxy specified? [Gap] — not in the requirements; plan assumes system/env proxy settings are honoured where the HTTP client supports them (same open item as 002 CHK015).
- [ ] CHK012 Is accessibility of the overlay and failure messages (screen readers, high contrast) specified? [Gap, Coverage] — not in the requirements; owner.

## Requirement Clarity

- [x] CHK013 Is every failure reason mapped to one observable condition and message? [Clarity, Spec §FR-019] — yes; 413/429/5xx map to "server error (HTTP <code>)" (Edge Cases).
- [x] CHK014 Is an HTTP 200 with an empty transcript distinguished from a failure? [Clarity, Spec §Edge Cases] — yes: treated as "no speech detected", nothing kept.
- [x] CHK015 Is "once per device change" for the microphone fallback notification defined? [Clarity, Spec §FR-030, US6 scenario 2] — yes.
- [ ] CHK016 Is the energy-threshold fallback of FR-016 given a measurable threshold or acceptance fixture? [Measurability, Spec §FR-016] — left to the plan; the committed fixtures (Assumptions) must classify the same way as with the Silero model.

## Requirement Consistency

- [x] CHK017 Is the tray-menu content of FR-001 consistent with the "Retry last failed dictation" entry of FR-027? [Consistency] — yes; shown only while a pending recording exists.
- [x] CHK018 Are the hotkey-error persistence rules of FR-011 and FR-028 consistent with req FR-25? [Consistency] — yes.
- [x] CHK019 Is the pending-recording replacement rule (FR-026) consistent with ordering (FR-029)? [Consistency] — yes; a successful later dictation does not remove the pending one (US2 scenario 8).
- [x] CHK020 Is the "History" tray item of FR-001 available before feature 005 lands? [Dependency, Spec §FR-001] — resolved in the plan (research R-17): hidden until 005 provides the window.

## Scenario and Edge Case Coverage

- [x] CHK021 Are sleep while recording, start window closed, and Esc during hold covered? [Edge Case, Spec §Edge Cases] — yes.
- [x] CHK022 Is a retry clicked while a recording is on covered? [Edge Case, Spec §Edge Cases] — resolved by Clarifications 3, 4.
- [x] CHK023 Is head-of-line blocking of ordered delivery bounded? [Recovery, Spec §FR-029] — yes, by the 30 s total timeout of FR-018 (plus 15 s post-processing in 003).
- [x] CHK024 Is a hotkey press while the previous recording is still in the speech-detection step covered for ordering? [Coverage, Spec §FR-029, Assumptions] — resolved: every stopped recording enters the order at stop, before speech detection; a no-speech result leaves it without blocking.

## Non-Functional Requirements

- [x] CHK025 Are logged events limited to non-secret fields, excluding error response bodies that can echo key fragments? [Security, Spec §FR-033] — resolved: reason code and HTTP status only.
- [ ] CHK026 Can SC-004 / SC-006 be automated? [Measurability, req §9] — no; tray, hotkey, toasts and auto-paste carry a reviewed `verify_exception` with the owner's manual check, plus Windows-runner integration tests.
- [x] CHK027 Is SC-001 measurable independently of the undefined reference machine (OQ-03)? [Measurability] — yes; the API figure does not depend on OQ-03.

## Notes

- Open items: CHK010, CHK011, CHK012, CHK016, CHK026.
