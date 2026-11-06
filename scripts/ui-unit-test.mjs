#!/usr/bin/env node
// The ui unit-test entry: package.json `scripts.test`, so `pnpm test [selectors] [vitest options]`
// (Makefile check-ui, the ui area's test_command, scripts/ui-test-select.mjs's unit group) comes
// here (T-195, invariant I4).
//
// Invariant: `pnpm test` with a selector that names no unit test file exits non-zero. vitest
// 5.0.3 prints "Couldn't find file" for a file:line outside its include set and exits 0, so a
// selector is checked here, before vitest starts:
//   - it must lie under src/ or scripts/ (vitest.config.ts `include`: src/**/*.test.ts,
//     scripts/*.test.ts); an e2e/ selector is refused with a pointer to `pnpm e2e`;
//   - it must name an existing file or directory (a trailing :line or :line:column is
//     stripped first).
// The first failing selector decides: exit 2, nothing runs. Otherwise `vitest run` gets the
// arguments unchanged and its exit code is this script's.
//
// Arguments starting with "-" are vitest options and pass through unchecked; the value of an
// option that takes one as a separate argument (e.g. `-t <name>`) is not a selector. A lone
// "--" (pnpm forwards it) is dropped.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const UNIT_DIRS = ["src", "scripts"];
// vitest options whose value may follow as a separate argument.
const VALUE_OPTIONS = new Set([
  "-t",
  "--testNamePattern",
  "-c",
  "--config",
  "-r",
  "--root",
  "--dir",
  "--project",
  "--reporter",
  "--outputFile",
  "--shard",
  "--maxWorkers",
  "--pool",
  "--environment",
  "--mode",
  "--exclude",
  "--testTimeout",
  "--hookTimeout",
  "--bail",
  "--retry",
  "--seed",
]);

/**
 * Why `selector` is not a unit selector, or null when it is.
 * @param {string} selector
 * @returns {string | null}
 */
export function refusal(selector) {
  const path = selector.replace(/^\.\//, "").replace(/(:\d+){1,2}$/, "");
  const top = path.split("/")[0];
  if (top === "e2e") {
    return `${selector} is an e2e test: run it with \`pnpm e2e ${selector}\` (Playwright), not with vitest`;
  }
  if (!UNIT_DIRS.includes(top)) {
    return `${selector} is outside ${UNIT_DIRS.map((d) => d + "/").join(" and ")} (vitest's include set)`;
  }
  if (!existsSync(join(ROOT, path))) return `${selector}: no such file (${path})`;
  return null;
}

/**
 * The selectors among vitest's arguments.
 * @param {string[]} args
 * @returns {string[]}
 */
export function selectorsOf(args) {
  const out = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--") continue;
    if (a.startsWith("-")) {
      if (VALUE_OPTIONS.has(a)) i++;
      continue;
    }
    out.push(a);
  }
  return out;
}

function main() {
  const args = process.argv.slice(2).filter((a) => a !== "--");
  for (const selector of selectorsOf(args)) {
    const why = refusal(selector);
    if (why !== null) {
      process.stderr.write(`ui-unit-test: refused: ${why}\n`);
      process.exit(2);
    }
  }
  const vitest = join(ROOT, "node_modules", "vitest", "vitest.mjs");
  if (!existsSync(vitest)) {
    process.stderr.write("ui-unit-test: node_modules/vitest is not installed (pnpm install)\n");
    process.exit(3);
  }
  const r = spawnSync(process.execPath, [vitest, "run", ...args], { cwd: ROOT, stdio: "inherit" });
  if (r.error) {
    process.stderr.write(`ui-unit-test: cannot start vitest: ${r.error.message}\n`);
    process.exit(3);
  }
  if (r.signal) process.kill(process.pid, r.signal);
  process.exit(r.status ?? 1);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
