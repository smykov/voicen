# Voicen Constitution

The engineering principles are defined in `PRINCIPLES.md` (P-001 … P-016), the single source.
This constitution references them and does not restate them; it adds the product-level
principles that follow from the approved requirements (`docs/requirements.md` v3). A change to
`PRINCIPLES.md` updates this file in the same commit.

## Core Principles

### I. Engineering principles: PRINCIPLES.md

Every spec, plan and task MUST comply with `PRINCIPLES.md`. Most relevant to plans:
P-004 (a guarantee needs a failing test), P-005 (tests must bite), P-006 (verify the running
system against logs), P-009 (secrets never reach logs, commits or docs), P-010 (one source of
truth per cross-cutting value), P-011 (one shared seam, not parallel paths).

### II. Privacy by default

- Audio and transcript text MUST NOT leave the machine except to the engine and post-processing
  endpoints the user configured (NFR-05). No telemetry.
- Transcript text, audio and API keys MUST NOT appear in logs, crash files or errors (FR-20, NFR-04).
- API keys MUST be stored only in Windows Credential Manager (NFR-04).
- Audio is kept only until its result is delivered; at most one failed recording is kept for
  retry (NFR-06).

### III. Never lose the user's words, never paste garbage

- Every failure path MUST deliver nothing wrong: no paste on error, no paste of hallucinated text
  on silence (FR-12), text left in the clipboard when the target window cannot be pasted into
  (FR-10), audio kept for retry (FR-11).
- Every Must requirement's failure branch from `docs/requirements.md` MUST appear in the spec
  and have a test.

### IV. Platform-independent core, thin Windows shell

- Pipeline logic (VAD gate, engines, post-processing, ordering, retry, timeouts, settings,
  history) lives in `crates/voicen-core` and MUST be testable on Linux with fakes.
- Windows APIs (hotkey, capture, clipboard, SendInput, Credential Manager, tray, toasts) live
  only in `src-tauri` or behind a core trait (decisions #5).
- Engines sit behind one trait; adding a provider MUST NOT change recording or delivery code
  (NFR-11). All OpenAI-compatible endpoints share one client (FR-06, FR-17).

### V. Measured, not assumed

- Performance claims (NFR-01, NFR-02, NFR-03) are checked from the log timings (FR-20), never
  from the code's own description.
- Behaviour that cannot run on the Linux host is verified on the Windows CI runner or by the
  owner's documented manual check; a spec states which, per requirement.

## Constraints

- Platform: Windows 10/11 x64 (Windows 11 checked per release, Windows 10 best-effort).
- Stack: Tauri 2; Rust 1.99 (`voicen-core`, `src-tauri`); TypeScript + Svelte 5 UI; NSIS
  per-user installer; ≤ 100 MB installed without models (NFR-09); idle RAM ≤ 150 MB (NFR-03).
- Dependencies: MIT-compatible licenses only (NFR-12).
- Installs on the dev host and in CI only with the owner's consent; Rust runs in the
  `voicen-rust:1.99` image.

## Development Workflow

- Implementation goes through `/teamwright:flow` and `docs/tasks/T-NNN.md`, never
  `/speckit-implement`. Spec Kit tasks are turned into teamwright tasks with `design_ref`.
- The gate is `make check`; it MUST be green before review. CI runs the same gate plus the
  Windows build, silent install and smoke check.
- A spec that contradicts `docs/requirements.md` is raised to the owner, never silently changed.

## Governance

This constitution follows `PRINCIPLES.md` and the approved requirements; on conflict, those
win and this file is corrected. Amendments: a commit that changes this file with a row in
`docs/decisions.md`; the owner decides. Versioning: MAJOR for a removed or redefined principle,
MINOR for an added one, PATCH for wording. Every plan's Constitution Check lists principles I–V.

**Version**: 1.1.1 | **Ratified**: 2026-10-02 | **Last Amended**: 2026-10-07
