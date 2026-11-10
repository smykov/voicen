// T-177 (invariant I2, characterization of E1 from T-050, decisions #77): the gate's own run
// asserts, on Playwright's resolved config, that it serves a build it made itself: no reuse of
// a server that happens to listen, a strict private port, a run-private build directory. The
// opt-in scripts/ci/e2e-build-race.sh pins the same E1 by behaviour; this test pins the
// configuration in every gate run at no extra cost (no second build, no fixed port).
//
// Bite check (docs/tasks/T-177.mutants/): reuseExistingServer: true, or `--strictPort` dropped
// from the preview command, or the build directory or the port not the run's own, turns it red.
import { resolve, sep } from "node:path";
import { expect, test } from "@playwright/test";

test("the gate config serves a private build: no reuse, strict private port, run-private outDir", () => {
  const info = test.info();
  const { webServer } = info.config;
  const baseURL = info.project.use.baseURL;
  if (process.env.BASE_URL) {
    // An external server is named explicitly: then the config has no webServer at all.
    expect(webServer, "BASE_URL names the server, so Playwright starts none").toBeNull();
    return;
  }

  expect(webServer, "without BASE_URL the config starts its own server").not.toBeNull();
  const port = process.env.VOICEN_E2E_PORT;
  expect(port, "the run picked its own port").toMatch(/^\d+$/);
  expect(webServer!.reuseExistingServer, "a server that already listens is never reused").toBe(false);
  expect(webServer!.command, "the preview takes the run's port and fails if it is taken").toContain(
    `--port ${port} --strictPort`,
  );
  expect(baseURL, "the tests talk to the port the preview serves").toBeTruthy();
  expect(webServer!.url, "the server is awaited where the tests go").toBe(baseURL);
  expect(new URL(baseURL!).port, "baseURL carries the run's port").toBe(port);

  const outDir = process.env.VOICEN_E2E_OUT_DIR ?? "";
  const e2eRoot = resolve("target", "e2e") + sep;
  expect(resolve(outDir).startsWith(e2eRoot), `the build directory ${outDir} is under target/e2e`).toBe(true);
  expect(resolve(outDir).slice(e2eRoot.length), "the build directory is this run's own").toMatch(
    /^run-\d+-\d+-[^/\\]+$/,
  );
});
