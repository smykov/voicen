# UI end-to-end runs (Playwright)

**Code:** `playwright.config.ts`, `e2e/support/{boot,global-teardown}.ts`, the env-driven `kit.outDir` and adapter-static `pages` / `assets` in `svelte.config.js` · **Tests that pin it:** `e2e/boot-failure.spec.ts` (the boot fixture's contract and the structural check that every spec using `page` imports `test` / `expect` from `./support/boot`), `e2e/build-race.spec.ts`, `scripts/ci/e2e-build-race.sh` (reproduction, scenarios `build` and `foreign-port`; opt-in via `VOICEN_E2E_BUILD_RACE`)

Tasks: T-050. Decisions: #77. Open questions: OQ-13 (answered).

## Invariants

### E1 — An e2e run serves only a build it made itself, from a directory and a port nobody else writes or binds

- **Defect that produced it:** none in the failure log (T-050 analysis, class `e2e-boot-flake`). `vite preview` serves `.svelte-kit/output`, not `build/`, and every `pnpm build` in the tree starts by removing that directory. A preview under a concurrent build answered 500 for the rest of its life (a rejected server-node import is cached) or died on a missing chunk. Fixed port 4173 with `reuseExistingServer: !CI` also let a gate reuse, and lose, another tree's preview.
- **What breaks if you violate it:** local gates fail at random in unrelated tests when a second `make check` or `pnpm build` runs in the same tree, or when a gate in another worktree reuses a foreign preview that holds different code.
- **Where it is enforced:** `playwright.config.ts` makes a run-private directory `target/e2e/run-<pid>-XXXXXX` (`mkdtemp`) and a free port, and passes them as `VOICEN_E2E_OUT_DIR`, `VOICEN_E2E_OUT_DIR_OWNER`, `VOICEN_E2E_PORT` (the runner picks them, workers and the webServer inherit). The webServer is `pnpm build && pnpm preview --port <port> --strictPort`, `reuseExistingServer: false`. `svelte.config.js` moves `kit.outDir` and the adapter's `pages` / `assets` under that directory only when `VOICEN_E2E_OUT_DIR` is set; every other build (lint, unit tests, `tauri build`, licenses-bundle) keeps the defaults. `BASE_URL` is the only way to reuse a server, and then there is no webServer. The runner removes its directory in `globalTeardown`; directories of runs whose pid is gone are swept at the next start.
- **Don't:** add a fixed port or `reuseExistingServer: true`, point the e2e preview at `.svelte-kit` or `build/`, set `VOICEN_E2E_OUT_DIR` for any other build, or share one run directory between runs.

### E2 — A page that fails to boot fails its test with the cause

- **Defect that produced it:** same as E1, and the host-network cause M2 (below): a failed chunk fetch made SvelteKit render its own "500 Internal Error" page and the test failed on an unrelated assertion or timeout.
- **What breaks if you violate it:** a boot failure is read as a product bug in whatever the test asserted next.
- **Where it is enforced:** `e2e/support/boot.ts` extends `page`: `requestfailed` (method, URL, errorText), a response with status >= 500, and a renderer crash are recorded, and the test fails in teardown with `page did not boot cleanly` and the list, even if its own assertions passed. Every spec using `page` imports `test` and `expect` from `./support/boot`; `e2e/boot-failure.spec.ts` fails when one does not.
- **Don't:** import `test` from `@playwright/test` in a spec that uses `page`, add retries (the cause is the report), or swallow failed requests in the fixture.

## Rejected approaches

| Approach | Why rejected | Ref |
|---|---|---|
| Blanket retries of failed tests | hide real failures | T-050 analysis |
| Building `licenses-bundle` elsewhere only | does not stop another gate's Playwright build or a foreign preview on the shared port | T-050 analysis |

## Open

- M2, the host-network cause of every trace so far (Chromium in the host netns gets `net::ERR_NETWORK_CHANGED` from other jobs' Docker network churn; 9/300 failures in the host netns, 0/200 in a private one), is not fixed by E1. T-064 runs the ui e2e in the Playwright container (decisions #77). Until then the boot fixture names it, it does not prevent it.
- Out of scope of T-050: the e2e build still writes the shared `target/licenses/npm-bundled.json` through the licenses Vite plugin, so two concurrent e2e builds in one tree write the same file. Not a served asset, so it cannot make a preview answer 500; not moved into the run directory.
