// T-064 red test, E1 across pid namespaces (docs/decisions/ui-e2e.md E1; T-064 analysis, seam
// "second path"): the sweep of stale run directories in playwright.config.ts must never delete
// the directory of a live run, also when that run is in another pid namespace (another
// container of the ui image, or the host, sharing target/e2e through the mounted tree).
// In a container the runner is pid 1-ish in its own namespace; `process.kill(pid, 0)` there
// says nothing about a `run-<pid>-` directory made in another namespace.
//
// Contract pinned here (black box: the tests load the real playwright.config.ts in a child
// `playwright test --list`, which runs the config's start-up exactly as every e2e run does, in
// a scratch working directory so target/e2e is private to the test):
//   1. a run directory records the pid namespace of the runner that made it (readlink
//      /proc/self/ns/pid, its decimal inode) and the runner's pid, in its name or in a file at
//      its top level;
//   2. at start-up, a directory of THIS pid namespace whose runner pid is gone is swept
//      (today's behaviour, kept);
//   3. a directory of this pid namespace whose runner is alive is kept (today's behaviour);
//   4. a directory of ANOTHER pid namespace is judged by age only: kept while younger than
//      the sweep's age limit (a live run in another container) ...
//   5. ... and swept once older than 48 hours (no run lives that long), so dead runs of other
//      namespaces do not pile up.
// The "another namespace" directory is made from a real one by replacing this namespace's
// inode with another number in its name and in its top-level files; the "old" one by setting
// the mtime of the directory and its top-level entries 48 hours back.
import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "@playwright/test";

const CONFIG = resolve("playwright.config.ts");
const CLI = resolve("node_modules/@playwright/test/cli.js");
const PID_NS = /^pid:\[(\d+)\]$/.exec(readlinkSync("/proc/self/ns/pid"))?.[1] ?? "";
const FOREIGN_NS = PID_NS === "4026500001" ? "4026500002" : "4026500001";
const HOURS_48 = 48 * 3600;

// A decimal number as a whole token (not part of a longer number).
function token(n: string): RegExp {
  return new RegExp(`(?<![0-9])${n}(?![0-9])`, "g");
}

let scratch = "";
test.beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "t064-run-dirs-"));
});
test.afterEach(() => {
  rmSync(scratch, { recursive: true, force: true });
});

function runDirs(): string[] {
  try {
    return readdirSync(join(scratch, "target/e2e")).sort();
  } catch {
    return [];
  }
}

// One start-up of the real config, as a fresh e2e run (no run variables inherited from this
// run), in the scratch dir. Returns the pid of that runner (gone when this returns).
function startUp(): number {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v === undefined || k === "BASE_URL" || k === "CI" || k.startsWith("VOICEN_E2E_")) continue;
    if (k.startsWith("TEST_") || k.startsWith("PW_") || k.startsWith("PLAYWRIGHT_")) continue;
    env[k] = v;
  }
  const r = spawnSync(process.execPath, [CLI, "test", "--list", "-c", CONFIG], {
    cwd: scratch,
    env,
    encoding: "utf8",
    timeout: 60_000,
  });
  expect(r.status, `config start-up failed:\n${r.stdout}\n${r.stderr}`).toBe(0);
  return r.pid;
}

// The run directory a start-up made: exactly one new entry.
function madeBy(before: string[]): string {
  const made = runDirs().filter((d) => !before.includes(d));
  expect(made, "one start-up of playwright.config.ts makes one run directory").toHaveLength(1);
  return made[0];
}

// Rewrites a run directory (name and top-level files): every whole-token `from` becomes `to`.
function rewrite(dir: string, from: string, to: string): string {
  const root = join(scratch, "target/e2e");
  for (const f of readdirSync(join(root, dir))) {
    const p = join(root, dir, f);
    if (statSync(p).isFile()) writeFileSync(p, readFileSync(p, "utf8").replace(token(from), to));
  }
  const name = dir.replace(token(from), to);
  if (name !== dir) renameSync(join(root, dir), join(root, name));
  return name;
}

function age(dir: string, seconds: number): void {
  const root = join(scratch, "target/e2e");
  const t = Date.now() / 1000 - seconds;
  for (const f of readdirSync(join(root, dir))) utimesSync(join(root, dir, f), t, t);
  utimesSync(join(root, dir), t, t);
}

// Everything at the top level of a run directory, name included, as one text.
function recorded(dir: string): string {
  const root = join(scratch, "target/e2e");
  const files = readdirSync(join(root, dir))
    .map((f) => join(root, dir, f))
    .filter((p) => statSync(p).isFile())
    .map((p) => readFileSync(p, "utf8"));
  return [dir, ...files].join("\n");
}

test("E1: a run directory records its runner's pid namespace and pid", () => {
  expect(PID_NS, "this test reads its own pid namespace").toMatch(/^\d+$/);
  const pid = startUp();
  const dir = madeBy([]);
  const text = recorded(dir);
  expect(text, `run directory ${dir} records the runner pid ${pid}`).toMatch(token(String(pid)));
  expect(text, `run directory ${dir} records the pid namespace ${PID_NS}`).toMatch(token(PID_NS));
});

test("E1: a dead run's directory of this pid namespace is swept at the next start", () => {
  startUp();
  const dead = madeBy([]);
  startUp();
  expect(runDirs(), "the dead run's directory is swept").not.toContain(dead);
});

test("E1: a directory of this pid namespace whose runner is alive is kept", () => {
  const pid = startUp();
  const live = rewrite(madeBy([]), String(pid), String(process.pid));
  startUp();
  expect(runDirs(), `run directory of the live pid ${process.pid} is kept`).toContain(live);
});

test("E1: a fresh directory of another pid namespace is kept although its pid is not alive here", () => {
  startUp();
  const foreign = rewrite(madeBy([]), PID_NS, FOREIGN_NS);
  startUp();
  expect(
    runDirs(),
    `run directory ${foreign} of pid namespace ${FOREIGN_NS} (a live run in another container) is kept`,
  ).toContain(foreign);
});

test("E1: a directory of another pid namespace older than 48 hours is swept", () => {
  startUp();
  const foreign = rewrite(madeBy([]), PID_NS, FOREIGN_NS);
  age(foreign, HOURS_48);
  startUp();
  expect(runDirs(), `stale run directory ${foreign} of another pid namespace is swept`).not.toContain(foreign);
});
