# Quickstart: Diagnostics and Release

How to exercise this feature once its tasks are implemented. Commands follow `CLAUDE.md` (Rust only in Docker via `scripts/tw-run core`; the app itself only on Windows).

## Linux host

```sh
make core-image                                             # after cargo-about is added to docker/rust.Dockerfile (owner consent)
scripts/tw-run core -- cargo test -p voicen-core diag       # log format, redaction, rolling, retention, session, crash files
scripts/tw-run core -- cargo test -p voicen-core paths
pnpm test -- src/lib/about                                  # About component logic
pnpm e2e -- e2e/about.spec.ts                               # About with mocked IPC: values, failures, buttons
make licenses-check                                         # crates + npm against about.toml; notices up to date
scripts/check-version.sh                                    # versions agree
make check                                                  # the gate (includes all of the above)
```

Expected: all green. A redaction test plants a fake transcript `"secret words 123"` and key `sk-test-PLANTED` into every input path and asserts 0 occurrences in the temp log folder.

## Windows CI runner (every push to `main`)

The `windows` job log shows, in order:
1. `cargo test --workspace` including `crash_probe` (panic → crash file kind `panic`; null deref → kind `native_fault`).
2. `installed size: NN.N MB (limit 100 MB)`.
3. `version: <MAJOR.MINOR>.<run number>` from the step `Version of this build` (a `vX.Y.Z` tag: `X.Y.Z`; T-077, decisions #98), then `voicen <that version> (<commit>) started` from the installed app.
4. After a forced kill and relaunch: `previous session ended abnormally` and one `crash-*.txt` with `kind: abnormal_end`.
5. Reinstall keeps the log; `/S /KEEPDATA` keeps `%LOCALAPPDATA%\Voicen` and the `ci-test` credential; `/S` removes both.
6. Artifacts `voicen-installer-<commit>` and `voicen-symbols-<commit>`.

## Release (owner)

```sh
# version bumped in Cargo.toml (workspace) and package.json in a normal commit, gate green
git tag v0.2.0 && git push origin v0.2.0                    # owner session only
gh release view v0.2.0                                      # installer, SHA256SUMS.txt, symbols zip, notes
```

A tag that does not match the version fails the `release` job with both versions printed and publishes nothing.

## Owner's manual checks on Windows

1. **Clean install** (success criterion 3, SC-009): in Windows Sandbox, download the installer from the release, run it (no UAC prompt; SmartScreen "More info → Run anyway"), complete one dictation.
2. **About**: Settings → General → "About Voicen" opens the dialog showing `Voicen <version> (<commit>)` matching the release; "Third-party licenses" opens the notices in Notepad; "Open logs folder" opens Explorer at `%LOCALAPPDATA%\Voicen\logs`.
3. **Crash bookkeeping**: shut Windows down with the app running → after logon no new `crash-*.txt`; end the process in Task Manager → next start adds one `abnormal_end` file.
4. **Uninstall question**: Settings → Apps → Voicen → Uninstall shows the question in the Windows display language; "No" keeps `%LOCALAPPDATA%\Voicen`; "Yes" removes it and the Voicen entries in Credential Manager.
5. **Two-week run** (success criterion 2): after 14 days of daily use, `%LOCALAPPDATA%\Voicen\logs` has no `crash-*.txt` dated in that period.
