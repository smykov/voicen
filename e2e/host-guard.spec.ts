// T-064 review 1 #2 (and #8): Playwright never runs on the host. Every Playwright invocation
// (`pnpm e2e`, `npx --no playwright test`, `pnpm exec playwright test`, `BASE_URL=... npx
// playwright test`) loads playwright.config.ts, so the config itself refuses to start outside
// the ui image.
//
// Contract (shared with scripts/ci/e2e-entry.test.sh, which runs the host half on the host):
// the config starts only when BOTH
//   - VOICEN_UI_IMAGE is non-empty (set by docker/ui.Dockerfile `ENV`, so present in every
//     container of the ui image, tw-run's included), and
//   - a container marker file exists (/.dockerenv from docker, /run/.containerenv from podman).
// Otherwise loading the config throws before any side effect (no target/e2e run directory, no
// webServer) with a message naming `pnpm e2e`.
//
// This file runs inside the ui image (pnpm e2e). It loads the real config in a child
// `playwright test --list` from a scratch working directory, as e2e/run-dirs.spec.ts does.
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "@playwright/test";

const CONFIG = resolve("playwright.config.ts");
const CLI = resolve("node_modules/@playwright/test/cli.js");

let scratch = "";
test.beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "t064-host-guard-"));
});
test.afterEach(() => {
  rmSync(scratch, { recursive: true, force: true });
});

// One start-up of the real config with this run's environment minus its run variables, and
// minus `drop`.
function startUp(drop: string[] = []): { status: number | null; output: string } {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v === undefined || drop.includes(k) || k === "BASE_URL" || k === "CI") continue;
    if (k.startsWith("VOICEN_E2E_") || k.startsWith("TEST_") || k.startsWith("PW_")) continue;
    if (k.startsWith("PLAYWRIGHT_")) continue;
    env[k] = v;
  }
  const r = spawnSync(process.execPath, [CLI, "test", "--list", "-c", CONFIG], {
    cwd: scratch,
    env,
    encoding: "utf8",
    timeout: 60_000,
  });
  return { status: r.status, output: `${r.stdout}\n${r.stderr}` };
}

function runDirs(): string[] {
  try {
    return readdirSync(join(scratch, "target/e2e"));
  } catch {
    return [];
  }
}

test("host guard: the ui image marks itself (VOICEN_UI_IMAGE set, container marker file present)", () => {
  expect(process.env.VOICEN_UI_IMAGE ?? "", "docker/ui.Dockerfile sets ENV VOICEN_UI_IMAGE").not.toBe("");
  expect(
    existsSync("/.dockerenv") || existsSync("/run/.containerenv"),
    "this run is in a container (/.dockerenv or /run/.containerenv)",
  ).toBe(true);
});

test("host guard: inside the ui image the real config starts", () => {
  const r = startUp();
  expect(r.status, `config start-up failed in the image:\n${r.output}`).toBe(0);
});

test("host guard: without VOICEN_UI_IMAGE the real config refuses before any run directory, naming pnpm e2e", () => {
  const r = startUp(["VOICEN_UI_IMAGE"]);
  expect(r.status, `the config started without the image marker:\n${r.output}`).not.toBe(0);
  expect(runDirs(), "no run directory is made before the refusal").toEqual([]);
  expect(r.output, "the refusal points to the entry point").toContain("pnpm e2e");
});
