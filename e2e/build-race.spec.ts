// T-050 red test (M1, decision #77): the e2e run serves only a build that it made
// itself, into a directory and on a port that no other process writes or binds.
//
// Driven by scripts/ci/e2e-build-race.sh, which runs this file through the gate's own
// Playwright config (playwright.config.ts webServer, BASE_URL unset), one scenario per
// invocation (VOICEN_E2E_BUILD_RACE=<scenario>). Without that variable both tests are
// skipped, so the gate's `pnpm e2e` does not run a second `pnpm build` in the tree.
//
// - "build": while the gate's preview serves, `pnpm build` runs in the same tree (what
//   licenses-bundle or another gate's webServer does). It is started with a plain shell
//   environment (no variable of this run), as another process in the tree would be.
//   Before, during and after that build the preview answers 200 for `/` and for the
//   entry chunk it served at the start. Before T-050 the build's rimraf of
//   .svelte-kit/output makes the preview answer 500 (cached failed import of a server
//   node) or die on ENOENT (connection refused).
// - "foreign-port": the script holds the old fixed port 4173 with a foreign HTTP server.
//   The run still serves its own build: `/` is the SvelteKit app, not the foreign page.
//   Before T-050 `reuseExistingServer: !CI` silently reuses whatever listens there.
import { spawn } from "node:child_process";
import { expect, test, type APIRequestContext } from "@playwright/test";

const scenario = process.env.VOICEN_E2E_BUILD_RACE ?? "";
const ENTRY = /\/?_app\/immutable\/entry\/start\.[\w-]+\.js/;
// The body the script's foreign server answers with (scripts/ci/e2e-build-race.sh).
const FOREIGN_MARKER = "t050-foreign-server";

interface Probe {
  path: string;
  outcome: string; // HTTP status, or the request error
}

async function probe(request: APIRequestContext, path: string): Promise<Probe> {
  try {
    const res = await request.get(path, { timeout: 10_000, failOnStatusCode: false });
    return { path, outcome: String(res.status()) };
  } catch (e) {
    return { path, outcome: `request failed: ${(e as Error).message.split("\n")[0]}` };
  }
}

// `pnpm build` in the tree, as an unrelated process would run it: only the plain shell
// variables, none of this run's (so a run-private out dir variable does not leak in).
function foreignBuild(): Promise<{ code: number | null; output: string }> {
  const env: Record<string, string> = {};
  for (const k of ["PATH", "HOME", "LANG", "TMPDIR", "USER", "SHELL", "TERM"]) {
    const v = process.env[k];
    if (v !== undefined) env[k] = v;
  }
  return new Promise((resolve, reject) => {
    const child = spawn("pnpm", ["build"], { cwd: process.cwd(), env });
    let output = "";
    child.stdout.on("data", (d) => (output += d));
    child.stderr.on("data", (d) => (output += d));
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, output }));
  });
}

test("build: a concurrent pnpm build in the tree does not change what the e2e preview serves", async ({
  request,
}) => {
  test.skip(scenario !== "build", "run by scripts/ci/e2e-build-race.sh");
  test.setTimeout(300_000);

  const index = await request.get("/");
  expect(index.status(), "the preview answers / before the race").toBe(200);
  const entry = (await index.text()).match(ENTRY)?.[0];
  expect(entry, "the served / names its entry chunk").toBeTruthy();
  const entryPath = entry!.startsWith("/") ? entry! : `/${entry}`;
  expect((await probe(request, entryPath)).outcome).toBe("200");

  const bad: Probe[] = [];
  let building = true;
  const poller = (async () => {
    while (building) {
      for (const path of ["/", entryPath]) {
        const p = await probe(request, path);
        if (p.outcome !== "200") bad.push(p);
      }
      await new Promise((r) => setTimeout(r, 50));
    }
  })();
  const build = await foreignBuild();
  building = false;
  await poller;
  expect(build.code, `the concurrent pnpm build itself must succeed:\n${build.output}`).toBe(0);

  for (const path of ["/", entryPath, "/settings"]) {
    const p = await probe(request, path);
    if (p.outcome !== "200") bad.push(p);
  }
  expect(
    { failures: bad.length, first: bad.slice(0, 8), last: bad.slice(-3) },
    "every request during and after the concurrent build answers 200",
  ).toEqual({ failures: 0, first: [], last: [] });
});

test("foreign-port: a foreign server on the old fixed port is never reused", async ({
  request,
}) => {
  test.skip(scenario !== "foreign-port", "run by scripts/ci/e2e-build-race.sh");

  const index = await request.get("/");
  const body = await index.text();
  expect(body, "the run serves its own build, not the server on port 4173").not.toContain(
    FOREIGN_MARKER,
  );
  expect(index.status()).toBe(200);
  expect(body).toMatch(ENTRY);
});
