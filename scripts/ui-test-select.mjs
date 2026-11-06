#!/usr/bin/env node
// The ui selector router (T-195, decision #246): `node scripts/ui-test-select.mjs <selector>...`
// sends each selector to its own runner, so the ui area's test_select_command can run an e2e/
// selector through Playwright and every other one through vitest.
//
// Invariant (I4): the router exits 0 only if every selector matches at least one test in the
// runner it is routed to and every runner it started exits 0.
//   - Routing: a selector starting with "e2e/" (after a leading "./" is stripped) goes to the
//     e2e group, run with `scripts/e2e.sh` (`pnpm e2e`); every other one to the unit group,
//     run with `scripts/ui-unit-test.mjs` (`pnpm test`).
//   - Usage (exit 2, nothing listed or run): no selector, or a selector starting with "-"
//     (an option: the router takes selectors only).
//   - Match (exit 1, nothing run): each selector is counted on its own with the runner's own
//     list (`vitest list --json`, `playwright test --list --reporter=json`); a missing file
//     counts 0. Never inferred from a run's exit code: both runners run the union of their
//     selectors and exit 0 when one of them matches.
//   - Run: the unit group, then the e2e group; the exit code is 0 only if both are 0, else
//     the first non-zero.
// `playwright test --list` loads the tests only: it runs no global setup, so the config's
// webServer (the build and preview) is not started (playwright 1.63.0 runAllTestsWithConfig,
// listMode). Inside the ui image the list run gets a placeholder BASE_URL, so the config does
// not create a private run directory for a server that never starts.
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** @typedef {"unit" | "e2e"} Runner */
/**
 * @typedef {{ ok: true, unit: string[], e2e: string[] }
 *   | { ok: false, code: 1 | 2, message: string }} Route
 */

/**
 * The router's pure step: groups the selectors by runner, or refuses them.
 * @param {string[]} selectors the arguments, as given
 * @param {(runner: Runner, selector: string) => number} list how many tests `selector`
 *   matches in `runner` (0 for a missing file); called once per selector, in order
 * @returns {Route}
 */
export function route(selectors, list) {
  if (selectors.length === 0) {
    return { ok: false, code: 2, message: "usage: ui-test-select.mjs <selector>... (no selector given)" };
  }
  const option = selectors.find((s) => s.startsWith("-"));
  if (option !== undefined) {
    return {
      ok: false,
      code: 2,
      message: `usage: ui-test-select.mjs takes selectors only, not options (got ${option})`,
    };
  }
  /** @type {string[]} */
  const unit = [];
  /** @type {string[]} */
  const e2e = [];
  /** @type {string[]} */
  const unmatched = [];
  for (const given of selectors) {
    const selector = given.replace(/^\.\//, "");
    /** @type {Runner} */
    const runner = selector.startsWith("e2e/") ? "e2e" : "unit";
    if (list(runner, selector) === 0) unmatched.push(`${selector} (${runner})`);
    (runner === "e2e" ? e2e : unit).push(selector);
  }
  if (unmatched.length > 0) {
    return { ok: false, code: 1, message: `selectors that match no test: ${unmatched.join(", ")}` };
  }
  return { ok: true, unit, e2e };
}

/** The file a selector names (a trailing :line or :line:column stripped). */
function fileOf(/** @type {string} */ selector) {
  return selector.replace(/(:\d+){1,2}$/, "");
}

/** Inside the ui image (the marker scripts/e2e.sh reads). */
function inImage() {
  return existsSync(process.env.VOICEN_E2E_DOCKER_INFO || "/ms-playwright/.docker-info");
}

/**
 * The JSON object in a runner's stdout (a wrapper may print lines around it), or null.
 * @param {string} text
 * @returns {unknown}
 */
function jsonIn(text) {
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start === -1 || end < start) return null;
  try {
    return JSON.parse(text.slice(start, end + 1));
  } catch {
    return null;
  }
}

/**
 * Specs in a Playwright JSON report (suites nest; one spec per test location).
 * @param {unknown} suites
 * @returns {number}
 */
function countSpecs(suites) {
  if (!Array.isArray(suites)) return 0;
  let n = 0;
  for (const s of suites) {
    n += Array.isArray(s?.specs) ? s.specs.length : 0;
    n += countSpecs(s?.suites);
  }
  return n;
}

/**
 * Tests a unit selector matches, by `vitest list --json`; 0 when ui-unit-test.mjs would
 * refuse it (outside src/ and scripts/, or no such file).
 * @param {string} selector
 * @returns {number}
 */
function listUnit(selector) {
  if (!existsSync(join(ROOT, fileOf(selector)))) return 0;
  const dir = mkdtempSync(join(tmpdir(), "ui-test-select-"));
  const out = join(dir, "list.json");
  try {
    const vitest = join(ROOT, "node_modules", "vitest", "vitest.mjs");
    const r = spawnSync(process.execPath, [vitest, "list", `--json=${out}`, selector], {
      cwd: ROOT,
      encoding: "utf8",
    });
    if (!existsSync(out)) {
      process.stderr.write(`ui-test-select: vitest list ${selector} wrote no list:\n${r.stdout ?? ""}${r.stderr ?? ""}`);
      return 0;
    }
    const listed = JSON.parse(readFileSync(out, "utf8"));
    return Array.isArray(listed) ? listed.length : 0;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * Tests an e2e selector matches, by `playwright test --list --reporter=json` through
 * scripts/e2e.sh (it re-enters the ui image from the host); 0 for a missing file.
 * @param {string} selector
 * @returns {number}
 */
function listE2e(selector) {
  if (!existsSync(join(ROOT, fileOf(selector)))) return 0;
  const env = { ...process.env };
  if (inImage()) env.BASE_URL = "http://localhost:9";
  const r = spawnSync(join(ROOT, "scripts", "e2e.sh"), ["--list", "--reporter=json", selector], {
    cwd: ROOT,
    encoding: "utf8",
    env,
    maxBuffer: 64 * 1024 * 1024,
  });
  const report = /** @type {{ suites?: unknown } | null} */ (jsonIn(r.stdout ?? ""));
  if (report === null) {
    process.stderr.write(`ui-test-select: playwright --list ${selector} gave no report:\n${r.stdout ?? ""}${r.stderr ?? ""}`);
    return 0;
  }
  return countSpecs(report.suites);
}

/**
 * Runs one group; its exit code (a signal or a failed start counts as 1).
 * @param {string} cmd
 * @param {string[]} args
 */
function run(cmd, args) {
  const r = spawnSync(cmd, args, { cwd: ROOT, stdio: "inherit" });
  if (r.error) {
    process.stderr.write(`ui-test-select: cannot start ${cmd}: ${r.error.message}\n`);
    return 1;
  }
  return r.status ?? 1;
}

function main() {
  const result = route(process.argv.slice(2), (runner, selector) =>
    runner === "e2e" ? listE2e(selector) : listUnit(selector),
  );
  if (!result.ok) {
    process.stderr.write(`ui-test-select: ${result.message}\n`);
    process.exit(result.code);
  }
  const codes = [];
  if (result.unit.length > 0) {
    codes.push(run(process.execPath, [join(ROOT, "scripts", "ui-unit-test.mjs"), ...result.unit]));
  }
  if (result.e2e.length > 0) {
    codes.push(run(join(ROOT, "scripts", "e2e.sh"), result.e2e));
  }
  process.exit(codes.find((c) => c !== 0) ?? 0);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
