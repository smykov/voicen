# Quickstart: Call Mode 1a — Record and Read

How to exercise this feature once its tasks are implemented. Commands follow `CLAUDE.md`: Rust runs only in Docker through `scripts/tw-run core`, and the app itself runs only on Windows. Formats and shapes are in [contracts/](contracts/), and entities in [data-model.md](data-model.md).

## Linux host

```sh
make core-image                                                   # after cmake is added to docker/rust.Dockerfile (approved, decisions #186)
scripts/tw-run core -- cargo test -p voicen-core call::session    # toggles, refusals, single session, stop reasons, FR-019
scripts/tw-run core -- cargo test -p voicen-core call::capture    # timeline: ±50 ppm → ≤ 20 ms at minute 60, gaps, silence fill, FR-013
scripts/tw-run core -- cargo test -p voicen-core call::codec      # Ogg Opus write, ≤ 40 MB/h for 2 tracks, truncated-tail read
scripts/tw-run core -- cargo test -p voicen-core call::storage    # meta.json states, recovery, damaged, disk thresholds, write failures, delete
scripts/tw-run core -- cargo test -p voicen-core call::final_pass # chunker at pauses / hard 120 s / silence not sent; verbose_json → json (wiremock)
scripts/tw-run core -- cargo test -p voicen-core call::transcript # merge, schema 1 validate, Markdown, renames
scripts/tw-run core -- cargo test -p voicen-core diag             # new log lines: no text, titles, paths, hosts, friendly names
pnpm test -- src/lib/calls src/lib/callIndicator
pnpm e2e -- e2e/calls.spec.ts e2e/call-indicator.spec.ts e2e/settings-calls.spec.ts
make licenses && make check                                       # notices current; the gate
```

Expected: all green. The privacy test runs a whole WAV-sourced call with a planted transcript (`"secret words 123"`), a planted title, a planted speaker name and a planted device friendly name. It asserts 0 occurrences in the temp log folder (SC-009).

## Windows CI runner (pushes to `main`, `wip/**`, `v*`)

The `windows` job log shows, in order:

1. `cargo test --workspace --exclude voicen`, including `SpeechSpans` on the fetched Silero model.
2. `cargo test -p voicen` with `tests/call_*.rs`: the WAV chain to a transcript, notifier register/unregister, the device-less loopback refusal, and the indicator style bits.
3. `crt-imports.sh` and `opus-cmake-cache.sh` green (libopus `/MT`).
4. Install smoke: `call_requested source=switch`, then `call_refused part=system reason=no_device` (FR-043); the app is still running; no call folder; window census unchanged.
5. Uninstall `/S`: the planted `calls\2026-01-01_000000` survives. After a reinstall, `/S /PURGECALLS` removes it, and the folder in a non-default root survives.

## Owner's manual checks on Windows (`verify_exception` where no kit adapter exists)

1. **First call** (SC-001): with the API engine set up, press Ctrl+Alt+R during a Telegram Desktop call and talk for 5 min with headphones, then press Ctrl+Alt+R again. Expected:
   - "Call saved — transcribing", then "Transcript ready" → Open;
   - lines from Me and Other party in the Calls window.
2. **Indicator**:
   - it appears without taking focus from the call app;
   - the reminder and hint show for 10 s;
   - drag it, restart, and it reappears where you left it;
   - Stop works;
   - it stays above a full-screen meeting window.
3. **Device switches**: during a recording, switch the default output (speakers → headset) and unplug the USB microphone. Expected:
   - the indicator warns;
   - the audio continues with gaps ≤ 2 s;
   - `meta.json` lists the changes;
   - the log has `call_device` lines with endpoint ids only.
4. **Crash recovery** (SC-005): end `voicen.exe` in Task Manager at minute N. On the next start the call shows "Recovered after a crash" with ≥ N min − 10 s of audio. Transcribe it.
5. **Shutdown during a call**: sign out while recording. After sign-in the call is Queued and transcribes by itself.
6. **Sleep**: close the lid while recording. On wake: "Recording stopped: sleep"; the call is saved.
7. **SC-003**: for a 60-min call, stop → "Transcript ready" ≤ 5 min. Measure it from the log's `final_finished ms=` with OpenAI `whisper-1` and with Groq `whisper-large-v3-turbo`.
8. **WER** (SC-004, FR-047): run `scripts/tw-run core -- cargo run -p voicen-core --example wer -- <mode> ...` with `VOICEN_WER_KEY` (and, for Groq, `VOICEN_WER_BASE_URL` / `VOICEN_WER_MODEL`) in the environment of that one command; modes, defaults and the FLAC→WAV preparation are in the example's header (`crates/voicen-core/examples/wer.rs`). English: `librispeech <test-clean-dir>` on the pinned manifest subset (≥ 30 min, #185; `verify-archive` first). Russian: `pairs <dir> ru` on the owner's 2 × 10-min calls (`<name>.wav` + `<name>.txt`). Both sets stay outside the repository; the output is ids and counts only. Expected: ≤ 10 % EN, ≤ 15 % RU.
9. **Uninstall**: Settings → Apps → Voicen → Uninstall. The calls question is asked with No as the default; after No, `%LOCALAPPDATA%\Voicen\calls` is still present.
10. **NFR-19 short set** (SC-011): the owner's call apps and headset per requirements NFR-19.
