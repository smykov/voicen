import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { defineConfig, devices } from "@playwright/test";

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
const E2E_ROOT = resolve("target/e2e");

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return (e as { code?: string }).code === "EPERM";
  }
}

// Directories of earlier runs whose runner process is gone (killed before its teardown).
function sweepStaleRunDirs(): void {
  let entries: string[];
  try {
    entries = readdirSync(E2E_ROOT);
  } catch {
    return;
  }
  for (const name of entries) {
    const pid = Number(/^run-(\d+)-/.exec(name)?.[1]);
    if (pid && !isAlive(pid)) rmSync(join(E2E_ROOT, name), { recursive: true, force: true });
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
    sweepStaleRunDirs();
    mkdirSync(E2E_ROOT, { recursive: true });
    process.env.VOICEN_E2E_OUT_DIR = mkdtempSync(join(E2E_ROOT, `run-${process.pid}-`));
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
