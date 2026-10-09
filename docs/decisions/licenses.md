# Licenses and third-party notices

**Code:** `about.toml`, `about.hbs`, `scripts/licenses/*` (`check.mjs`, `notices.mjs`, `workspace.mjs`, `rust.sh`, `fixture-check.sh`, `stale-check.sh`), `licenses/manual.json`, `licenses/texts/`, `licenses/fixtures/gpl/`, `licenses/fixtures/stale/`, plugin `voicen-bundled-packages` in `vite.config.js`, Makefile `licenses*`, `THIRD-PARTY-NOTICES.txt` · **Tests that pin it:** `scripts/licenses/*.test.mjs` (`node --test`), `scripts/licenses/fixture-check.sh`, `scripts/licenses/stale-check.sh` (`make licenses-stale-guard`), `make licenses-stale`

Tasks: T-027, T-063 (bundling the file into the installer: T-025, spec 006 T029/T030). Decisions: #9, #24, #29. Spec: `specs/006-diagnostics-and-release` (US6).

## Invariants

### Every shipped component passes the one accepted list

- **Defect that produced it:** none yet (found in planning, T-027). Decision #9 and research R13 each held a different list (P-010).
- **What breaks if you violate it:** a GPL or unlicensed component ships (NFR-12).
- **Where it is enforced:** `about.toml` `accepted` is the only list (#9 plus the five of #24). The values that mean "no license" (`UNLICENSED`, `UNKNOWN`, `NONE`, `NOASSERTION`) are one set, `NO_LICENSE` in `scripts/licenses/spdx.mjs`; `check.mjs` reports them as "no license". Three readers use it: cargo-about (`targets = x86_64-pc-windows-msvc`, build and dev dependencies ignored), `scripts/licenses/check.mjs` (npm bundle), and the same check over `licenses/manual.json`. `make licenses-check` runs in `make check`.
- **Don't:** keep a second list in a script or in a spec; add CPL-1.0 or another license to accept a tool (NSIS uses zlib compression instead, #29).

### npm is checked from the bundle, not from `package.json` sections

- **Defect that produced it:** found in T-027 analysis: Svelte and SvelteKit are devDependencies but their runtime ships in `build/_app`; `pnpm licenses list --prod` lists only `@tauri-apps/api`. Checking all packages instead fails on dev tools that never ship (minimatch BlueOak-1.0.0, lightningcss MPL-2.0).
- **What breaks if you violate it:** shipped code goes unchecked and unlisted, or the gate fails on code we do not ship.
- **Where it is enforced:** the Vite plugin writes `target/licenses/npm-bundled.json` from the modules actually bundled; `check.mjs --require svelte` fails on an empty or missing list. `make licenses-bundle` deletes the previous list before `pnpm build`, so a build that records nothing cannot pass on a stale list.
- **Don't:** go back to `pnpm licenses --prod` or to `dependencies` vs `devDependencies`.

### The bundle check fails closed for what the plugin sees: every bundled module and file-backed asset is ours or a package's

- **Defect that produced it:** T-027 review round 1, finding 1. Modules without a node_modules path were dropped silently. Vite's preload helper (`\0vite/preload-helper.js`) ships in `build/_app`, but `vite` was in neither the check nor the notices. Any plugin that generates code through a virtual id, or emits an asset from a package, was a second path past the gate.
- **What breaks if you violate it:** code ships unchecked and unlisted (NFR-12, FR-032).
- **Where it is enforced:** `classifyBundle` in `scripts/licenses/bundle.mjs`, called by the plugin. Each module id and each emitted asset's original file is one of three things:
  - project source: a file under the Vite root, outside `node_modules`. This includes `.svelte-kit/generated` and `i18n/*.json`.
  - a package's file, found by its `node_modules` path. Two build-tool namespaces are attributed by rule. `\0vite/*` (`preload-helper.js`, `modulepreload-polyfill.js` and the others) goes to `vite`, resolved from the root. `\0rolldown/*` (`runtime.js`, Vite 8's bundler runtime) goes to `rolldown`, resolved through vite's directory, because under pnpm it is vite's dependency. Both are then checked and listed like any other package.
  - unattributed: any other virtual id, with or without the `\0` prefix; a file outside the root; a `node_modules` file of no package. It is written to the list, and `check.mjs` fails, naming it. `notices.mjs` refuses to generate.
- **Assets without an original file pass, on an assumption.** SvelteKit's `_app/version.json` is an example. The plugin cannot tell what such an asset holds; it passes because, with today's plugins, such assets are generated from build data and carry no third-party code (orchestrator decision, T-027 round 1; T-063). An asset that does come from a file is classified like a module.
- **What the plugin cannot see.** It reads the bundle in its own `generateBundle` hook, so it never sees: files written in `writeBundle` or `closeBundle` (e.g. a service worker written by a workbox plugin); assets emitted by plugins whose `generateBundle` runs after ours (post plugins); code added in `renderChunk`, `banner`, `intro` or `footer`; files copied from `static/`. None of today's plugins does any of this, and `static/` holds only project files (T-027 review round 2). Adopting a Vite plugin that emits files or injects code, or putting third-party files in `static/`, is a license-review point: attribute its output in `classifyBundle` or `licenses/manual.json` in the same change.
- **Don't:** add an id allowlist that exempts a virtual module from the check. Attribute it to the package that generates it (a rule in `BUILD_TOOL_MODULES`), or keep it out of the bundle. `plugin.test.mjs` pins the rule: a new unattributed virtual id fails.

### The committed notices are byte-equal to a fresh generation

- **Defect that produced it:** none yet. The Windows job has no cargo-about, so the file is generated on Linux and committed (FR-032).
- **What breaks if you violate it:** the installer ships notices that do not match its contents.
- **Where it is enforced:** `make licenses-stale` diffs the committed file (`NOTICES`, default `THIRD-PARTY-NOTICES.txt`) against `target/licenses/THIRD-PARTY-NOTICES.txt` and fails, naming the file, when it is stale (with the diff) or not committed. `make licenses-stale-guard` (`scripts/licenses/stale-check.sh`, in `licenses-check`) runs that real recipe without the generation on `licenses/fixtures/stale/{equal,stale,missing}` and checks that `licenses-check` depends on it (T-063). Regenerate with `make licenses` and commit whenever dependencies change.
- **The project's own crates are not listed.** `about.hbs` skips crates without a source. These are path crates: the workspace's `voicen` and `voicen-core`, and any later member. They are not third-party, but cargo-about still checks them like every other crate (T-027 review round 1). `notices.test.mjs` reads the workspace members from `Cargo.toml` (`workspaceMemberNames` in `scripts/licenses/workspace.mjs`, which refuses a glob member) and requires that none of them is listed, and that every `Cargo.lock` package without a source is one of them (`pathCratesOutsideWorkspace`, T-063).
- **Don't:** edit `THIRD-PARTY-NOTICES.txt` by hand; add machine-specific paths or clearlydefined lookups to the output. Vendor third-party code as a path crate, through a path dependency or `[patch]`: the template would leave it out of the notices, and `notices.test.mjs` fails, naming the crate. If that is ever needed, list it in the notices (e.g. `licenses/manual.json`) and change the test in the same change.

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
| Drop virtual modules with no node_modules path, or exempt them by an id allowlist | ships code the gate never sees; a new plugin's virtual id would pass unnoticed | T-027 review round 1 |
| Accept CPL-1.0 for NSIS's LZMA module | new license; zlib compression is enough (NFR-09) | decisions #29 |

## Open

- None.
