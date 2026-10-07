# CLAUDE.md

<!--
This file is a MAP, not an encyclopedia. It is loaded into every session in full,
so every line is paid for on every run.
Size budget: a hook warns when it exceeds 40 KB (TEAMWRIGHT_CLAUDE_MD_MAX_KB in
.claude/settings.json). That is a ceiling, not a target: a map is usually far shorter.
Put in here only what costs you BEFORE you'd think to open a file.
Everything else goes to docs/decisions/<area>.md and is linked from the index below.
Replace every <placeholder>; delete sections that don't apply.
-->

## Process

This repository follows the teamwright process: start with `AGENTS.md` (roles, sessions, rules). Statuses and transitions: `docs/process/lifecycle.md`. A hook refused or warned: `docs/process/gates.md` (each gate's exits). Who does what: `docs/process/roles.md`, `docs/process/sessions.md`. A `TEAMWRIGHT_PAUSE` file at the root stops all pushes. In Claude Code, `/teamwright:flow` picks the next step.

## Project

Voicen — a Windows desktop dictation tool: a global hotkey records the microphone, the audio is transcribed (OpenAI-compatible API, built-in whisper.cpp, or a local OpenAI-compatible server), optionally post-processed by an LLM, then copied to the clipboard and pasted into the focused input field. Public open-source (MIT). Requirements: `docs/requirements.md` (approved v3).

**Stack:** Tauri 2; Rust 1.99 core library (`crates/voicen-core`) and Windows shell (`src-tauri`); UI in TypeScript + Svelte 5 (SvelteKit static, Vite); NSIS per-user installer. Target: Windows 10/11 x64. Dev host: Linux.

## Where things live

| Path | What |
|---|---|
| `crates/voicen-core/` | Platform-independent core (area `core`): engines, pipeline, settings, VAD gate, build info |
| `src-tauri/` | Tauri shell: tray, hotkey, capture, clipboard, paste, IPC commands — Windows-only, built and tested only in Windows CI |
| `src/` | Web UI (area `ui`); unit tests `src/**/*.test.ts` |
| `e2e/` | Playwright UI tests, Tauri IPC mocked via `window.__TAURI_INTERNALS__`; specs import `test`/`expect` from `e2e/support/boot` (boot fixture); each run builds and serves privately (`docs/decisions/ui-e2e.md`) |
| `docker/rust.Dockerfile` | Toolchain image `voicen-rust:1.99` for area `core` (`make core-image`) |
| `docker/ui.Dockerfile` | Toolchain image `voicen-ui:1.63.0` for area `ui` (Node 24, pnpm, Playwright; `make ui-image`) |
| `.github/workflows/ci.yml` | Linux gate + Windows build / silent install / smoke (= "deployed"); runs on push to `main` and `wip/**` (the pre-review run of P-016); caches are saved and the installer is sent to Telegram from `main` (and `v*` tags) only, through `scripts/ci/telegram-send.sh`, whose failure branch runs with a fake token off `main` and tags (`make check`: `check-ci-wip`, `check-telegram-send`, `check-telegram-failure-branch`, `check-telegram-guard`; `docs/decisions/ci-toolchain.md`) |
| `scripts/ci/helper-windows.txt` | Helper-window manifest: framework windows the install smoke does not count; read by the smoke predicate, the shell overlay tests and the `make check` tripwire (`docs/decisions/overlay.md` §5) |
| `scripts/ci/credentials.ps1` | The only way a Windows CI step plants or observes Credential Manager entries (Win32 `CredWriteW`/`CredReadW`/`CredEnumerateW`/`CredDeleteW`); `make check` refuses any other Credential Manager entry point (Win32 `Cred*` call, `cmdkey`, `vaultcmd`, ...) in workflows and `scripts/ci` (`docs/decisions/windows-ci-runner.md`) |
| `docs/plan.md` | Stages and what is in each |
| `docs/requirements.md` | What must be true (FR-NN, NFR-NN) |
| `docs/architecture.md` | Components, data flow, boundaries |
| `docs/decisions.md` | Append-only decision log |
| `docs/decisions/<area>.md` | Why an area is shaped the way it is: invariants and the defects behind them |
| `docs/open-questions.md` | Unresolved questions (OQ-NN) |
| `docs/failures.md` | What broke, why, which rule it produced (F-NNN) |
| `docs/tasks/` | One file per task (T-NNN); `<ID>.reviews/` review records, `<ID>.verify/` verification records |
| `specs/` | Spec Kit feature specs, plans, tasks; tasks link them via `design_ref` |
| `docs/sprints/` | One file per sprint |
| `PRINCIPLES.md` | Engineering principles with enforcement tiers |

## What you must know before opening any file

- **Windows-only code lives in `src-tauri` or behind a trait in `voicen-core`.** The local gate builds only `voicen-core` on Linux; anything Windows-specific is proven only by the Windows CI job (decisions #5).
- **Rust runs only in Docker** (`scripts/tw-run core -- ...`); there is no host toolchain. Rebuild the image with `make core-image` after changing `docker/rust.Dockerfile`.
- **Never log transcript text, audio or API keys** (FR-20, NFR-04). Keys live in Windows Credential Manager only.
- **Shell tests live only in `src-tauri/tests/*.rs`** (they get the Common-Controls v6 manifest from `src-tauri/build.rs`); no `#[cfg(test)]` in `src-tauri/src`, keep `[lib] doctest = false` (and no `--doc`/`rustdoc` in any workflow step: the scan cannot tell the package) — `make check` refuses them (`docs/decisions/ci-toolchain.md`).

## Commands

```sh
pnpm install && make core-image && make ui-image  # setup
make check                                      # gate — must be green before NEEDS_REVIEW
scripts/tw-run core -- cargo test -p voicen-core <filter>   # single core test
pnpm test -- <file>                             # single UI unit test
pnpm dev                                        # UI in a browser (no Rust side; IPC calls fail)
pnpm e2e                                        # UI end-to-end (Playwright, Chromium, mocked IPC); runs only through this entry, in the ui image (Docker required; `make ui-image` after a Dockerfile change). A bare `playwright test` on the host is refused; `BASE_URL` against a host server is unsupported
make licenses                                   # regenerate THIRD-PARTY-NOTICES.txt; commit it whenever dependencies change (make check fails when stale)
```

Gotchas: the app itself (`pnpm tauri dev/build`) runs only on Windows; on Linux verify the UI with mocked IPC and core with fakes. The gate's license check needs crates.io access; offline it says "cannot run" (not a license failure), see `docs/decisions/licenses.md`.

## Rules

- Follow the task lifecycle: no code before the task's `analysis` block is filled; status changes are commits `chore(task): T-NNN FROM→TO`.
- Gate green before `NEEDS_REVIEW`. Commit locally; **never push from a DEV session**, except the orchestrator's CI-only `git push origin HEAD:wip/<ID>` (not a delivery; the branch is deleted at `CODE_COMPLETE`).
- A task with a diff outside `docs/**` cites under `## Tests` `CI: <run> @ <sha>`, a green full `ci.yml` run on `wip/<ID>` before `CODE_COMPLETE` (P-016): `<sha>` is the reviewed head or a commit after which only `docs/**` changed; a green re-run at the same sha counts only if each red attempt's failure is outside the diff and logged as its own task or F-entry, cited next to the line. Steps gated on `main` never run on `wip/`. The next push to `main` is never evidence.
- Self-review doesn't count — `CODE_COMPLETE` is set only by a separate review.
- Large work (more than one seam or service, or unsettled behaviour) starts in the spec tool; its tasks carry `design_ref`. Small work goes straight to a task.
- On a recurrence of a known defect class: stop patching, open a `type: rca` task (`recurrence_of: [...]`); exits — code fix, `/arch-review`, or a `PRINCIPLES.md` amendment (owner decides the last two).
- `VERIFIED` / `DONE` need a `PASS` verify record for the current code (per the task's `surface`); the verifier never edits code.
- Gather context with the configured context and docs tools (`AGENTS.md` › Tools) before text search or memory.
- Docs affected by a change are updated in the same commit.
- Never put secrets, tokens or customer data in code, logs, commits or docs.
- Unsure about a product decision → add to `docs/open-questions.md` and ask; don't guess.
- Owner decisions go into `docs/decisions.md` immediately (Was → Decided).
