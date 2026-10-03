# Licenses and third-party notices

**Code:** `about.toml`, `about.hbs`, `scripts/licenses/*` (`check.mjs`, `notices.mjs`, `rust.sh`, `fixture-check.sh`), `licenses/manual.json`, `licenses/texts/`, `licenses/fixtures/gpl/`, plugin `voicen-bundled-packages` in `vite.config.js`, Makefile `licenses*`, `THIRD-PARTY-NOTICES.txt` · **Tests that pin it:** `scripts/licenses/*.test.mjs` (`node --test`), `scripts/licenses/fixture-check.sh`, `make licenses-stale`

Tasks: T-027 (bundling the file into the installer: T-025, spec 006 T029/T030). Decisions: #9, #24, #29. Spec: `specs/006-diagnostics-and-release` (US6).

## Invariants

### Every shipped component passes the one accepted list

- **Defect that produced it:** none yet (found in planning, T-027). Decision #9 and research R13 each held a different list (P-010).
- **What breaks if you violate it:** a GPL or unlicensed component ships (NFR-12).
- **Where it is enforced:** `about.toml` `accepted` is the only list (#9 plus the five of #24). Three readers use it: cargo-about (`targets = x86_64-pc-windows-msvc`, build and dev dependencies ignored), `scripts/licenses/check.mjs` (npm bundle), and the same check over `licenses/manual.json`. `make licenses-check` runs in `make check`.
- **Don't:** keep a second list in a script or in a spec; add CPL-1.0 or another license to accept a tool (NSIS uses zlib compression instead, #29).

### npm is checked from the bundle, not from `package.json` sections

- **Defect that produced it:** found in T-027 analysis: Svelte and SvelteKit are devDependencies but their runtime ships in `build/_app`; `pnpm licenses list --prod` lists only `@tauri-apps/api`. Checking all packages instead fails on dev tools that never ship (minimatch BlueOak-1.0.0, lightningcss MPL-2.0).
- **What breaks if you violate it:** shipped code goes unchecked and unlisted, or the gate fails on code we do not ship.
- **Where it is enforced:** the Vite plugin writes `target/licenses/npm-bundled.json` from the modules actually bundled; `check.mjs --require svelte` fails on an empty or missing list.
- **Don't:** go back to `pnpm licenses --prod` or to `dependencies` vs `devDependencies`.

### The committed notices are byte-equal to a fresh generation

- **Defect that produced it:** none yet. The Windows job has no cargo-about, so the file is generated on Linux and committed (FR-032).
- **What breaks if you violate it:** the installer ships notices that do not match its contents.
- **Where it is enforced:** `make licenses-stale` diffs the committed file against `target/licenses/THIRD-PARTY-NOTICES.txt`. Regenerate with `make licenses` and commit whenever dependencies change.
- **Don't:** edit `THIRD-PARTY-NOTICES.txt` by hand; add machine-specific paths or clearlydefined lookups to the output.

### The fixture guard is tied to cargo-about 0.9.2 wording

- **Defect that produced it:** none yet. Without `--fail`, cargo-about only warns about an undeterminable license, so a mention of the crate name proves nothing.
- **What breaks if you violate it:** the Rust check could pass while rejecting nothing.
- **Where it is enforced:** `fixture-check.sh` runs the real `about.toml` on `licenses/fixtures/gpl/` and requires a non-zero exit plus the exact 0.9.2 error forms for both crates. The version is pinned in `docker/rust.Dockerfile`.
- **Don't:** bump cargo-about without re-running the guard and updating the expected wording in the same change.

### Offline is "cannot run", not a license result

- **Defect that produced it:** none yet (decision #24, D3). cargo-about needs crate sources; `cargo metadata --offline` fails with `failed to download`.
- **What breaks if you violate it:** a network outage reads as a license failure, or worse, as a pass.
- **Where it is enforced:** `fixture-check.sh` exits 3 and `rust.sh` reports "cannot run" when cargo-about, docker or the network is missing. The CI `gate` job caches the cargo registry.
- **Don't:** map "cannot run" to success to unblock the gate, or to a license error.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| `pnpm licenses list --prod` for npm | misses the shipped Svelte runtime | T-027 |
| `cargo-deny` plus a notice generator | two tools, two lists | research R13 |
| Accept CPL-1.0 for NSIS's LZMA module | new license; zlib compression is enough (NFR-09) | decisions #29 |

## Open

- None.
