import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readlinkSync, rmSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { defineConfig, devices } from "@playwright/test";

// Playwright never runs on the host (T-064 review 1 #2, #8; docs/decisions/ui-e2e.md E3): every
// invocation (`npx playwright test`, `pnpm exec playwright test`, `BASE_URL=... npx playwright
// test`) loads this file, so it refuses to start outside the ui image before any side effect
// (no run directory, no webServer). The ui image is recognised by BOTH its own variable
// (docker/ui.Dockerfile ENV VOICEN_UI_IMAGE) and a container marker file; neither alone counts.
if (
  !process.env.VOICEN_UI_IMAGE ||
  !(existsSync("/.dockerenv") || existsSync("/run/.containerenv"))
) {
  throw new Error(
    "e2e runs only inside the ui image (VOICEN_UI_IMAGE set in a container); run `pnpm e2e`, " +
      "which starts it in the image with a private network namespace (docs/decisions/ui-e2e.md E3)",
  );
}

// The e2e run serves only a build it made itself, from a directory and on a port no other
// process writes or binds (T-050, decisions #77):
// - the webServer builds into a run-private directory under target/e2e/ (VOICEN_E2E_OUT_DIR,
//   read by svelte.config.js), so a concurrent `pnpm build` in the tree (licenses-bundle,
//   another gate) cannot rewrite what the preview serves;
// - the preview listens on a free port picked for this run, with --strictPort, so a port
//   taken in between fails loudly instead of serving something else;
// - an existing server is reused only when BASE_URL names it explicitly (then there is no
//   webServer at all).
// The config is loaded by the runner and again by every worker: the runner picks the dir and
// port and puts them in the environment, which the workers (and the webServer) inherit.
// The runner removes its dir in global teardown; dirs of runs that died are swept here.
// E1 across pid namespaces (T-064): runs in other containers of the ui image, or on the host,
// share target/e2e through the mounted tree, and a pid means something only in its own pid
// namespace. So a run dir is named run-<pid namespace inode>-<runner pid>-XXXXXX; the sweep
// judges a dir of this namespace by its runner's liveness, and a dir of any other namespace
// (or of the older run-<pid>-XXXXXX form, namespace unknown) only by age.
const E2E_ROOT = resolve("target/e2e");
// No e2e run lives this long; a younger dir of another pid namespace may be a live run.
const FOREIGN_MAX_AGE_MS = 24 * 3600 * 1000;

// The inode of this process's pid namespace ("pid:[4026531836]" -> "4026531836"); "0" when
// it cannot be read (then every dir counts as another namespace's and is swept by age only).
function pidNamespace(): string {
  try {
    return /^pid:\[(\d+)\]$/.exec(readlinkSync("/proc/self/ns/pid"))?.[1] ?? "0";
  } catch {
    return "0";
  }
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as { code?: string }).code === "EPERM";
  }
}

// Directories of earlier runs whose runner is gone (killed before its teardown).
function sweepStaleRunDirs(ns: string): void {
  let entries: string[];
  try {
    entries = readdirSync(E2E_ROOT);
  } catch {
    return;
  }
  const now = Date.now();
  for (const name of entries) {
    if (!name.startsWith("run-")) continue;
    const dir = join(E2E_ROOT, name);
    const m = /^run-(\d+)-(\d+)-[^-]+$/.exec(name);
    let stale: boolean;
    if (m && m[1] === ns && ns !== "0") {
      stale = !isAlive(Number(m[2]));
    } else {
      try {
        stale = now - statSync(dir).mtimeMs > FOREIGN_MAX_AGE_MS;
      } catch {
        continue;
      }
    }
    if (stale) rmSync(dir, { recursive: true, force: true });
  }
}

// A port that is free right now on localhost (the host vite preview binds by default).
function freePort(): number {
  const out = execFileSync(process.execPath, [
    "-e",
    `const s = require("node:net").createServer();
     s.listen(0, "localhost", () => { process.stdout.write(String(s.address().port)); s.close(); });`,
  ]);
  return Number(out.toString());
}

function privateServer(): { port: number } {
  if (!process.env.VOICEN_E2E_OUT_DIR || !process.env.VOICEN_E2E_PORT) {
    const ns = pidNamespace();
    sweepStaleRunDirs(ns);
    mkdirSync(E2E_ROOT, { recursive: true });
    process.env.VOICEN_E2E_OUT_DIR = mkdtempSync(join(E2E_ROOT, `run-${ns}-${process.pid}-`));
    process.env.VOICEN_E2E_OUT_DIR_OWNER = String(process.pid);
    process.env.VOICEN_E2E_PORT = String(freePort());
  }
  return { port: Number(process.env.VOICEN_E2E_PORT) };
}

const external = process.env.BASE_URL;
const server = external ? undefined : privateServer();
const baseURL = external ?? `http://localhost:${server!.port}`;

export default defineConfig({
  testDir: "e2e",
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"]],
  globalTeardown: "./e2e/support/global-teardown.ts",
  use: {
    baseURL,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: server
    ? {
        command: `pnpm build && pnpm preview --port ${server.port} --strictPort`,
        url: baseURL,
        reuseExistingServer: false,
      }
    : undefined,
});
