# The gate: every area's checks, each through scripts/tw-run in the area's toolchain.
.PHONY: check check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-core \
	check-ci-credentials \
	check-clippy-lines \
	check-core-test-os-answer check-webview-data check-ci-wip check-telegram-send check-telegram-failure-branch \
	check-version-stamp check-release-check check-release check-release-guard check-nsis-fork check-e2e-entry-fixtures check-area-image-fixtures \
	check-whisper-cmake-cache check-opus-cmake-cache check-whisper-model-key check-crt-imports check-whisper \
	check-shell-windows check-ban-canary check-role-tools check-guard-trigger check-wip-push check-wip-run \
	check-ui core-image ui-image core-image-present \
	licenses licenses-check licenses-unit licenses-fixture licenses-rust licenses-bundle \
	licenses-npm licenses-generate licenses-stale licenses-stale-guard

# check-ban-canary and check-shell-windows after check-core: both build voicen-core, so a core
# error is reported first by the faster native check-core, before the slower runs.
check: check-shell-layout check-shell-layout-fixtures check-helper-windows-fixtures check-ci-credentials check-role-tools check-guard-trigger check-wip-push check-wip-run \
	check-clippy-lines \
	check-core-test-os-answer check-webview-data check-ci-wip check-telegram-send check-telegram-failure-branch check-version-stamp check-release-check check-release check-release-guard check-nsis-fork check-whisper-cmake-cache check-opus-cmake-cache check-whisper-model-key check-crt-imports check-core check-whisper \
	check-ban-canary check-shell-windows check-e2e-entry-fixtures check-area-image-fixtures check-ui \
	licenses-check

# Shell tests only in src-tauri/tests/*.rs (T-035, T-038, T-092, F-002; docs/decisions/ci-toolchain.md):
# cargo metadata's voicen targets (no kind-test rows) equal scripts/ci/shell-targets.txt (lib and
# bin test = false, doctest = false; no example, bench or extra target), and line 1 of the lib and
# bin crate roots is the crate-level cfg pin. cargo metadata in the core image, host python3 -I.
check-shell-layout:
	scripts/ci/area-image.sh --present core
	scripts/ci/shell-test-layout.sh

# Guard: the layout check must give its documented exit code on every fixture shell dir in
# scripts/ci/fixtures/shell-test-layout/ (0 allowed, 1 violation, 3 cannot run), and the pins
# must empty the crate under every cargo test selector (mechanism crate). Host bash, core image.
check-shell-layout-fixtures:
	scripts/ci/area-image.sh --present core
	scripts/ci/shell-test-layout.test.sh

# T-065 (rca smoke-window-predicate): the helper-window tripwire scripts/ci/helper-windows.sh
# must hold its contract on every fixture in scripts/ci/fixtures/helper-windows/ and on the
# real Windows dependency graph with scripts/ci/helper-windows.txt, and the predicate's copies
# must read that one manifest. Host bash; the real-graph case reaches the core image.
check-helper-windows-fixtures:
	scripts/ci/area-image.sh --present core
	scripts/ci/helper-windows.test.sh

# T-061, T-066: CI reaches Credential Manager only through scripts/ci/credentials.ps1: no other
# line of a workflow or a scripts/ci file, comments included (T-090), names a Credential Manager entry point
# (a Win32 Cred* call, cmdkey, vaultcmd, keymgr, PasswordVault, the CredentialManager module);
# the self-test runs the tripwire scripts/ci/ci-credentials.sh on its cases and on the real
# repo. Host bash.
check-ci-credentials:
	scripts/ci/ci-credentials.test.sh

# T-089 (P-020; docs/decisions/ci-toolchain.md, docs/tasks/T-089.md): the voicen-core call bans
# (a reqwest client built outside engine::http, decisions #106, #113; Instant::now / elapsed
# outside crate::clock and tests/common, T-080 I2) are rustc's path resolution: the
# disallowed-types / -methods lints, configured next to crates/voicen-core/Cargo.toml and pinned
# at deny by its [lints] table (check-core adds -D warnings).
# check-ban-canary (decides_on: switch): scripts/ci/ban-canary.sh lints the owned probe files
# (features ban-canary-lib, ban-canary-tests) in the core image and compares every diagnostic
# with scripts/ci/ban-canary.expected.txt, so each config entry and the deny level are proven.
check-ban-canary:
	scripts/ci/area-image.sh --present core
	scripts/ci/ban-canary.sh

# The token guard below (decides_on: token + literal): every tracked line holding the lint
# tool's name (any case, comments included) in a *.rs, Cargo.toml, .cargo config or make file
# is an owned line of the guard's list, so no allow, cap, config or edit of check-core's lint
# command can lower the bans unseen; this file's lines holding the name are owned too. The
# self-test runs the guard on trees built in a temp dir, on copies of the real repository and
# on the real repository. Host bash, git.
check-clippy-lines:
	scripts/ci/clippy-lines.test.sh

# check-role-tools (T-082, decisions #145; T-113, decisions #154, P-020 decides_on: literal): every
# file under .claude/agents (recursive, any name) has an owned head scripts/ci/role-heads/<name>,
# its front matter is byte-equal to that head from byte 0 through the closing --- line (cmp, no
# YAML reading, no CRLF/BOM normalisation), every head has its role file, and a role whose head
# names an mcp__ tool keeps both owned "not connected" fallback lines in its Tools block. The
# self-test runs the tripwire scripts/ci/role-tools.sh on trees built in a temp dir and on the real
# .claude/agents. Host bash.
check-role-tools:
	scripts/ci/role-tools.test.sh

# check-guard-trigger (T-113, decisions #154, #155, P-020 "How checked"): the git pre-commit hook
# scripts/hooks/pre-commit refuses a staged guard change unless the active task has checker: true
# and a non-empty analysis.decides_on. Guard change (raw token, #155): a staged path (any status,
# no rename detection) under scripts/ci/ or scripts/licenses/, the path Makefile, or a path ending
# in .mk; no Make, shell or content reading. The self-test runs the hook in fixture git repos.
# Host bash, git, python3.
check-guard-trigger:
	scripts/ci/guard-trigger.test.sh

# check-wip-push (T-192, decisions #208, #219): scripts/tw-wip-push writes only refs/heads/wip/<ID>
# on origin, <ID> from branch tw/<ID>, and refuses anything else with no ref moved; the PreToolUse
# hook scripts/hooks/wip-push-guard.sh refuses the git -C and git -c push forms; .claude/settings.json
# allows the script only, denies those forms and wires the hook. The self-test copies the script into
# a temporary clone of a temporary bare remote; no network transport is allowed. Host bash, git, python3.
check-wip-push:
	scripts/ci/tw-wip-push.test.sh

# check-wip-run (T-199, decisions #228, #241): scripts/tw-wip-run is the only CI read of a stage
# session. Its gh argv is one of two owned templates (run list --branch wip/<ID> ..., run view <id>
# ...), the ID from branch tw/<ID>; it prints CI: <url> @ <sha> on a green run of the commit, names the
# failed jobs on a red one, waits inside its window, has a distinct exit for pending, no run and a gh
# failure, and refuses off a tw/<ID> branch, with an option or a second argument, and under
# TEAMWRIGHT_PAUSE, gh never called. The self-test puts a fake gh first on PATH; no network. Host bash, git.
check-wip-run:
	scripts/ci/tw-wip-run.test.sh

# check-core-test-os-answer (T-080 rca, class os-answer-deadline-race; T-089, decides_on: token):
# no core test outside crates/voicen-core/tests/common names an OS-answer target (.invalid,
# 127.0.0.1:1, 192.0.2.1), any case, comments included; targets come with their deadlines from
# tests/common (docs/decisions/core-tests.md, I1). The self-test runs the tripwire
# scripts/ci/core-test-os-answer.sh on scripts/ci/fixtures/core-test-os-answer/ and the real
# tests dir. Host bash.
check-core-test-os-answer:
	scripts/ci/core-test-os-answer.test.sh

# T-062 (P-020, decides_on: token; docs/decisions/ci-toolchain.md guard table): every webview keeps
# its WebView2 data under Voicen\webview through the one app-level override
# (paths::route_app_local_data): scripts/ci/webview-data.sh refuses the raw token data_directory in
# src-tauri/src/**/*.rs and dataDirectory in src-tauri/*.json, and requires app_directories_override
# exactly once in src-tauri/src, in paths.rs. The self-test runs the tripwire on
# scripts/ci/fixtures/webview-data/ and on the real shell dir. Host bash.
check-webview-data:
	scripts/ci/webview-data.test.sh

# T-066 (rca, P-016, decisions #90): .github/workflows/ci.yml runs on push to main and wip/**
# (the pre-review run of every non-docs task) unless the push changes only docs/tasks/** (T-083,
# decisions #144), and no workflow step saves a cache on a ref
# other than main. Since T-088 both are release-guard rules (R7 on ci.yml, R11-R13): this name
# stays, as a prerequisite-only alias, because PRINCIPLES.md P-016 names it.
check-ci-wip: check-release-guard

# T-070 (decisions #93): scripts/ci/telegram-send.sh, the one Telegram send path of ci.yml, keeps
# the token and chat id out of curl's argv and its output, sends the caption as a form string,
# and turns every failure into one ::warning:: line with exit 0. Stub curl on PATH, offline.
# Host bash.
check-telegram-send:
	scripts/ci/telegram-send.test.sh

# T-070 (review 2 #1): scripts/ci/telegram-failure-branch.sh, the verdict of ci.yml's fake-token
# failure-branch step (off main and v* tags), is red only on the send script's contract (exit
# non-zero, no ::warning::, the fake token printed) and a warning when Telegram was not reached.
# Canned outputs, offline. Host bash.
check-telegram-failure-branch:
	scripts/ci/telegram-failure-branch.test.sh

# T-077 (decisions #98): scripts/ci/version-stamp.sh, the windows job's one version step, writes
# <repo MAJOR.MINOR>.<run number> (a vX.Y.Z tag: X.Y.Z) into Cargo.toml, tauri.conf.json,
# package.json and the two workspace members of Cargo.lock, no other line, and exports
# VOICEN_VERSION to GITHUB_ENV; a malformed tag, repo version, disagreeing files or a bad run
# number exit 1 naming the value and write nothing. Fixture trees and a copy of the real files,
# offline. Host bash, diff, awk.
check-version-stamp:
	scripts/ci/version-stamp.test.sh

# T-026 (decisions #102, #103): "release job version check". scripts/ci/release-check.sh, the one
# release verdict of ci.yml (release job and the windows job's premise / failure branch), accepts
# a tag vX.Y.Z only when X.Y is the committed MAJOR.MINOR, Z >= the run number, exactly one
# installer carries _X.Y.Z_ and (with GH_TOKEN) no release exists; every refusal exits 1 naming
# both values, and it never calls a gh write subcommand. Stub gh on PATH, offline. Host bash.
check-release-check:
	scripts/ci/release-check.test.sh

# T-085 (rca, decisions #120): scripts/ci/release.sh, the release job's one step, refuses before
# any gh call without GH_TOKEN, runs release-check.sh and, only on its pass, writes
# SHA256SUMS.txt and the notes and calls gh release create --verify-tag exactly once; a refusal
# writes nothing and a failed create is not a pass. Stub gh on PATH, offline. Host bash.
check-release:
	scripts/ci/release.test.sh

# T-026, T-085 (rca, decisions #120, #121, #122, P-020), T-084 (#119), T-087 (#124): raw-text
# rules R1-R10 over every file in .github/workflows: the workflow-level permissions: contents: read block (R1), the release job
# to the end of its file byte-identical to the guard's owned copy (R2), permissions only there
# (R3), no secret but the two Telegram ones and no vars (R4), the word gh only inside the release
# job (R5), no YAML escape that spells a character or joins two lines (R6), the header of the
# file holding the release job, line 1 to jobs:, byte-identical to scripts/ci/release-header.yml
# (R7; decisions #121), no YAML tag: the raw texts !!, !< and %TAG nowhere (R8; decisions
# #122), the one installer upload byte-identical to scripts/ci/release-upload.yml (R9; per name, T-206: teamwright-agent.yml's one upload is the tail scripts/ci/contour-upload.yml, to EOF), and the
# raw token secrets.TELEGRAM_ (any case, comments included) only in the one Telegram send step,
# byte-identical to scripts/ci/telegram-send-step.yml (R10; replaces telegram-guard, T-087:
# T-070 validation 1 M6 and probes T1-T4 are its fixtures); and (T-088, replaces ci-wip and
# setup-node-cache) R7 also on ci.yml (its trigger: push to main and wip/**, skipped when only
# docs/tasks/** changed, T-083), every line with the word uses or an action name (actions/cache, setup-node, rust-cache, cache/save,
# pnpm/action-setup) an owned line of scripts/ci/action-lines.txt or inside a byte-exact owned
# step fragment of scripts/ci/cache-steps/ (whose own text the guard checks: saves from main
# only, no setup-node cache) (R11), no column-0 line after jobs: (R12) and no YAML merge key
# (R13); T-202 (decisions #253) R14: the files teamwright-agent.yml and teamwright-next.yml
# (exact names) are the only others with a write token; their header is the owned copy
# scripts/ci/contour-heads/<name>.
# The self-test runs the tripwire scripts/ci/release-guard.sh on
# scripts/ci/fixtures/release-guard/ and the real workflows dir (also mutated). Host bash and awk.
check-release-guard:
	scripts/ci/release-guard.test.sh

# T-025 (decisions #76): the NSIS template is a minimal-diff fork of the one in the pinned
# tauri-cli, so scripts/ci/nsis-fork.sh refuses a tree where src-tauri/windows/installer.nsi's
# '; upstream: tauri-cli X.Y.Z' header, the exact @tauri-apps/cli in package.json and the
# version of the installed node_modules/@tauri-apps/cli/package.json differ, or tauri.conf.json
# does not use the fork; pnpm-lock.yaml is not read (T-093, P-020: the specifier equality is
# pnpm --frozen-lockfile's). No installed CLI: cannot run. T-156 (decisions #197): every line of
# the fork and of installer-hooks.nsh holding the raw token 'rmdir' (any case, comments included)
# must be a line of scripts/ci/nsis-rmdir-lines.txt, prefixed with its file, so no recursive delete
# (which follows junctions) can enter; the list missing: cannot run. The self-test runs it on fixture trees
# built in a temp dir and on the real repo. Host bash, python3 (stdlib json).
check-nsis-fork:
	scripts/ci/nsis-fork.test.sh

# T-064: the ui e2e entry point (package.json scripts.e2e) runs Playwright only in the ui image
# at the lockfile's @playwright/test version, with --network none and the caller's CI; a
# missing or mismatched image stops it before any test, naming the image. Fixture cases with a
# fake docker and fake Playwright CLIs. Host bash, python3, node.
check-e2e-entry-fixtures:
	scripts/ci/e2e-entry.test.sh

# T-177 (I5, I7, I4): scripts/ci/area-image.sh, the one reader of an area's image tag (the
# area's `image` in the teamwright config, through the kit parser), its `--present` check
# (`docker image inspect` only, missing -> exit 3 naming `make <area>-image`) and the ui tag
# pinned to the installed @playwright/test; plus no literal toolchain tag in the Makefile,
# docker/ui.Dockerfile or the contour workflow, and check-ui's presence check and --workers.
# Fixture roots with a fake docker. Host bash, python3, make.
check-area-image-fixtures:
	scripts/ci/area-image.test.sh

# T-099 (decisions #142, invariant 3): scripts/ci/whisper-cmake-cache.sh, the windows job's
# verdict on every whisper-rs-sys CMakeCache.txt under target/ (decides_on: token + literal),
# passes only when each holds GGML_NATIVE:BOOL=OFF, GGML_AVX2:BOOL=ON, GGML_AVX512:BOOL=OFF,
# GGML_AVX_VNNI:BOOL=OFF and BUILD_SHARED_LIBS:BOOL=OFF exactly once and every line naming one of those options or
# GGML_BACKEND_DL / GGML_CPU_ALL_VARIANTS is an owned line; no cache found is red. Fixture trees,
# offline. Host bash.
check-whisper-cmake-cache:
	scripts/ci/whisper-cmake-cache.test.sh

# T-117 (decisions #197): scripts/ci/opus-cmake-cache.sh, the windows job's verdict on every
# opusic-sys build under target/ (decides_on: token + literal, plus the dumpbin -directives effect
# check), passes only when each CMakeCache.txt holds OPUS_STATIC_RUNTIME:BOOL=ON and
# CMAKE_BUILD_TYPE:STRING=Release exactly once, no line names CMAKE_MSVC_RUNTIME_LIBRARY or another
# OPUS_STATIC_RUNTIME, and each out/lib/opus.lib names /DEFAULTLIB:LIBCMT and never MSVCRT; no
# cache found is red. Fixture trees and a stub dumpbin, offline. Host bash.
check-opus-cmake-cache:
	scripts/ci/opus-cmake-cache.test.sh

# T-099 (review 1 finding 2, P-010): the windows job's tiny-model cache key is derived from the
# catalog (hashFiles of crates/voicen-core/src/local_models/catalog.rs, no restore-keys), and
# ci.yml repeats no commit or SHA-256 of the catalog. Raw lines of ci.yml and catalog.rs. Host bash.
check-whisper-model-key:
	scripts/ci/whisper-model-key.test.sh

# T-099 (invariant 4, F-001 class): scripts/ci/crt-imports.sh, the windows job's verdict on the
# C runtime of every exe linking whisper.cpp (decides_on: token), refuses an exe whose dumpbin
# -dependents output holds MSVCP140 or VCRUNTIME140, a missing exe and an empty or failed dump.
# Stub dumpbin, offline. Host bash.
check-crt-imports:
	scripts/ci/crt-imports.test.sh

# check-core's image check is a prerequisite, not a recipe line: the owned lint line stays the
# first recipe line under `check-core:` (T-089, its guard pins that position). A prerequisite
# is made before the recipe starts, also under -j (T-177 I5).
check-core: core-image-present

core-image-present:
	scripts/ci/area-image.sh --present core

check-core:
	scripts/tw-run core -- 'cargo fmt --check -p voicen-core && cargo clippy -p voicen-core --all-targets -- -D warnings'
	scripts/tw-run core -- cargo test -p voicen-core

# Linux format and lint check of crates/voicen-whisper with -D warnings over every target (T-099;
# docs/decisions/ci-toolchain.md "voicen-whisper on the Linux gate"). DOCS_RS=1, scoped to this
# one invocation and never set in .cargo/config.toml, makes whisper-rs-sys skip its cmake build
# of whisper.cpp (it generates or copies the bindings first): the lint never links, so it needs
# no C++ toolchain for whisper.cpp. Building, linking and running the crate's tests stay the
# windows job's (tests/transcribe_wav.rs, the CMakeCache and dumpbin steps).
check-whisper:
	scripts/ci/area-image.sh --present core
	scripts/tw-run core -- 'cargo fmt --check -p voicen-whisper && DOCS_RS=1 cargo clippy -p voicen-whisper --all-targets -- -D warnings'

# Windows-target format and lint of the shell on Linux (T-056, decisions #63, T-166;
# docs/decisions/ci-toolchain.md). One tw-run call, three commands: rustfmt --check of the
# package, then the lib and bin with the release feature set in the dev profile (no
# dev-dependency features, tauri without custom-protocol: not what `pnpm tauri build`
# compiles, so its production context and cfg(not(dev)) / cfg(not(debug_assertions)) code
# stay the windows job's; positive cfg(dev) / cfg(debug_assertions) code is checked here),
# then with --all-targets every src-tauri/tests/*.rs, the lib (and voicen-core) built as
# their dependency with the dev-dependency features; lib and bin have test = false, so
# there are no unit-test crates (T-092). --all-targets alone would check the lib only with
# test-fakes on and pass a release-only error. Both lint runs are a full type check and
# deny every rustc and lint warning (-D warnings, as check-core and check-whisper); nothing
# links and no doctest is built: linking, starting and running the shell stay the windows
# job's. Needs the gnu target and mingw gcc baked into the core image
# (docker/rust.Dockerfile; `make core-image`). The shell links voicen-whisper (T-102) and
# the image has no mingw g++: DOCS_RS=1, scoped to these two invocations as in
# check-whisper, makes whisper-rs-sys skip its cmake build; only whisper-rs and
# whisper-rs-sys read it, so the other crates' gnu builds stay checked.
# T-201 (decisions #247, #248): the shell clock ban of the shell package lint config file
# is at deny through src-tauri/Cargo.toml [lints]. The recipe line is owned by the
# owned-lines list under scripts/ci; make check-ban-canary proves the ban bites.
check-shell-windows:
	scripts/ci/area-image.sh --present core
	scripts/tw-run core -- 'cargo fmt --check -p voicen && DOCS_RS=1 cargo clippy -p voicen --target x86_64-pc-windows-gnu -- -D warnings && DOCS_RS=1 cargo clippy -p voicen --target x86_64-pc-windows-gnu --all-targets -- -D warnings'

# Playwright workers of the gate's e2e run (T-177 I4): the owner's host limit is 2, and on CI's
# 4-vCPU runner Playwright's own default (half the logical cores) is 2 as well. Override on the
# command line: `make check-ui WORKERS=1`.
WORKERS ?= 2

# The toolchain image tags, read from the areas' `image` in the teamwright config (T-177 I7):
# no tag is written in this file. Recursive (`=`): read only by a recipe that uses them.
UI_IMAGE = $(shell scripts/ci/area-image.sh ui)
CORE_IMAGE = $(shell scripts/ci/area-image.sh core)

# The ui image must be present before the first container (T-177 I5): a `docker run` of a
# missing tag pulls implicitly and fails as a gate error (exit 125); the check stops with exit 3
# and `make ui-image`. Every recipe of `make check` that starts a toolchain container (directly
# or through its script) has this check for its area before its first container. The e2e step goes through the e2e entry (package.json scripts.e2e) on
# the host: it checks the ui image is present and re-enters it through scripts/tw-run ui with
# --network none (T-064), with an explicit worker count.
check-ui:
	scripts/ci/area-image.sh --present ui
	scripts/tw-run ui -- pnpm lint
	scripts/tw-run ui -- pnpm test
	scripts/e2e.sh --workers=$(WORKERS)

# Toolchain image of the `core` area (CI builds the same one). The first line stops on an
# unreadable tag with the reader's message.
core-image:
	scripts/ci/area-image.sh core >/dev/null
	docker build -t $(CORE_IMAGE) -f docker/rust.Dockerfile docker

# Toolchain image of the `ui` area: the official Playwright image at the lockfile's
# @playwright/test version plus pnpm (docker/ui.Dockerfile; CI builds the same one, T-064).
# The tag comes from the config; the reader has checked that its version is the installed
# @playwright/test, the base image's Playwright version (the Dockerfile's digest default pins
# that base).
ui-image:
	scripts/ci/area-image.sh ui >/dev/null
	docker build -t $(UI_IMAGE) --build-arg PLAYWRIGHT_VERSION=$(patsubst voicen-ui:%,%,$(UI_IMAGE)) --build-arg VOICEN_UI_IMAGE=$(UI_IMAGE) -f docker/ui.Dockerfile docker

# --- Licenses (T-027, decisions #9 #24) -------------------------------------------------
# One accepted list: about.toml `accepted`. Rust: cargo-about 0.9.2 (Windows target only).
# npm: the packages of the client bundle (target/licenses/npm-bundled.json, written by the
# Vite build) plus licenses/manual.json, checked by scripts/licenses/check.mjs.
# Needs crates.io access; without it the check says it cannot run (exit 3), not a license
# failure (decision #24 D3).
LICENSES_DIR := target/licenses
# The committed notices file. `:=`, not `?=`: an environment variable cannot redirect the
# check; only a command-line override can (licenses-stale-guard, on its fixtures).
NOTICES := THIRD-PARTY-NOTICES.txt

licenses-check: licenses-unit licenses-fixture licenses-stale-guard licenses-rust licenses-npm licenses-stale

# Unit tests of the npm/manual checker. The files are named: since Node 22 (the ui image has
# Node 24) `node --test <dir>` runs the directory as a module instead of finding its tests.
licenses-unit:
	scripts/ci/area-image.sh --present ui
	scripts/tw-run ui -- 'node --test scripts/licenses/*.test.mjs'

# Guard: cargo-about with the real about.toml must reject licenses/fixtures/gpl.
licenses-fixture:
	scripts/ci/area-image.sh --present core
	scripts/licenses/fixture-check.sh

# T-063 guard: the real licenses-stale recipe, run without the generation on
# licenses/fixtures/stale/<case>/ (LICENSES_DIR and NOTICES overridden), must pass equal notices
# and fail stale and missing ones naming the file. Offline, no docker. Host bash, make, diff.
licenses-stale-guard:
	scripts/licenses/stale-check.sh

# Rust crates of the workspace, Windows target, against about.toml; writes rust.txt.
licenses-rust:
	scripts/ci/area-image.sh --present core
	scripts/licenses/rust.sh

# Client build; its Vite plugin writes $(LICENSES_DIR)/npm-bundled.json. The previous list
# is deleted first, so a build that records nothing cannot pass on a stale list.
licenses-bundle:
	scripts/ci/area-image.sh --present ui
	scripts/tw-run ui -- 'rm -f $(LICENSES_DIR)/npm-bundled.json && pnpm build'

# npm packages of the client bundle and the hand-kept list, against about.toml.
# --require svelte: the bundle list must hold the Svelte runtime, or the plugin saw nothing.
licenses-npm: licenses-bundle
	scripts/ci/area-image.sh --present ui
	scripts/tw-run ui -- node scripts/licenses/check.mjs --about about.toml \
		--bundle $(LICENSES_DIR)/npm-bundled.json --manual licenses/manual.json --require svelte

# Notices generated into target/licenses (both targets below use it). cargo-about renders
# the Rust part only when every crate is accepted; npm and manual entries are listed
# whatever their license (licenses-npm judges them), so a notice is never dropped.
licenses-generate: licenses-rust licenses-bundle
	scripts/ci/area-image.sh --present ui
	scripts/tw-run ui -- node scripts/licenses/notices.mjs --rust $(LICENSES_DIR)/rust.txt \
		--bundle $(LICENSES_DIR)/npm-bundled.json --manual licenses/manual.json \
		--out $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt

# The committed notices must equal a fresh generation (guarded by licenses-stale-guard).
licenses-stale: licenses-generate
	@test -f $(NOTICES) || { echo "licenses-check: $(NOTICES) is not committed; run make licenses and commit it" >&2; exit 1; }
	@diff -u $(NOTICES) $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt || { echo "licenses-check: $(NOTICES) is stale; run make licenses and commit it" >&2; exit 1; }

# Regenerate the committed notices.
licenses: licenses-generate
	cp $(LICENSES_DIR)/THIRD-PARTY-NOTICES.txt $(NOTICES)
