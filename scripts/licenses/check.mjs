// License check for the npm bundle and the hand-kept list (T-027).
//
//   node scripts/licenses/check.mjs --about about.toml \
//     --bundle target/licenses/npm-bundled.json --manual licenses/manual.json \
//     [--require <package>]...
//
// Exit 0: every component accepted. Exit 1: license failures, each named on stderr,
// including bundle modules or assets attributed to no package (fail closed, bundle.mjs).
// Exit 2: cannot check (missing or empty bundle list, unreadable about.toml or manual list,
// a bundled package without package.json, a --require'd package absent from the bundle).
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { readAcceptedList } from "./accepted.mjs";
import { describeUnattributed, loadBundleList, readPackageLicense, splitBundleList } from "./bundle.mjs";
import { NO_LICENSE, satisfies } from "./spdx.mjs";

/**
 * Every component whose license does not satisfy `accepted`, in input order.
 * reason: "missing" (absent, empty, or a NO_LICENSE value of spdx.mjs) | "not-accepted".
 * @param {{ name: string, version?: string, license?: string | null }[]} components
 * @param {string[]} accepted
 * @returns {{ name: string, version?: string, license: string | null, reason: "missing" | "not-accepted" }[]}
 */
export function checkComponents(components, accepted) {
  /** @type {{ name: string, version?: string, license: string | null, reason: "missing" | "not-accepted" }[]} */
  const failures = [];
  for (const c of components) {
    const license = typeof c.license === "string" ? c.license : null;
    /** @type {"missing" | "not-accepted" | null} */
    let reason = null;
    if (license === null || license.trim() === "" || NO_LICENSE.has(license.trim())) reason = "missing";
    else if (!satisfies(license, accepted)) reason = "not-accepted";
    if (reason === null) continue;
    failures.push({
      name: c.name,
      ...(c.version !== undefined ? { version: c.version } : {}),
      license,
      reason,
    });
  }
  return failures;
}

/**
 * Reads licenses/manual.json: a JSON array of { name, version?, license, url?, note? }.
 * Throws (message names the file) when it is missing, not an array, or an entry has no name.
 * @param {string} file
 * @returns {{ name: string, version?: string, license: string | null }[]}
 */
export function loadManualList(file) {
  let data;
  try {
    data = JSON.parse(readFileSync(file, "utf8"));
  } catch (e) {
    throw new Error(`cannot read the manual list ${file}: ${e instanceof Error ? e.message : e}`);
  }
  if (!Array.isArray(data)) throw new Error(`manual list ${file} is not a JSON array`);
  for (const entry of data) {
    if (!entry || typeof entry.name !== "string" || entry.name.trim() === "") {
      throw new Error(`manual list ${file}: an entry has no name: ${JSON.stringify(entry)}`);
    }
  }
  return data;
}

/**
 * Parses `--key value` pairs; repeated keys collect into an array.
 * @param {string[]} argv
 * @param {string[]} known
 * @returns {Record<string, string[]>}
 */
export function parseArgs(argv, known) {
  /** @type {Record<string, string[]>} */
  const args = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    const value = argv[i + 1];
    if (!argv[i]?.startsWith("--") || !known.includes(key) || value === undefined) {
      throw new Error(`bad argument ${JSON.stringify(argv[i])}; expected ${known.map((k) => `--${k} <value>`).join(" ")}`);
    }
    (args[key] ??= []).push(value);
  }
  return args;
}

/** One line per failure, naming the component, its version and what is wrong. */
function describe(/** @type {string} */ kind, /** @type {ReturnType<typeof checkComponents>[number]} */ f) {
  const what = f.version ? `${f.name}@${f.version}` : f.name;
  return f.reason === "missing"
    ? `licenses-check: FAIL ${kind} ${what}: no license (${f.license === null ? "none given" : JSON.stringify(f.license)})`
    : `licenses-check: FAIL ${kind} ${what}: license ${f.license} is not in the accepted list (about.toml)`;
}

/**
 * @param {string[]} argv
 * @returns {number} exit code
 */
export function main(argv) {
  let accepted, bundled, unattributed, manual;
  try {
    const args = parseArgs(argv, ["about", "bundle", "manual", "require"]);
    for (const k of ["about", "bundle", "manual"]) {
      if (!args[k]) throw new Error(`missing --${k}`);
    }
    accepted = readAcceptedList(readFileSync(args.about[0], "utf8"));
    const list = splitBundleList(loadBundleList(args.bundle[0]));
    for (const want of args.require ?? []) {
      if (!list.packages.some((p) => p.name === want)) {
        throw new Error(`bundle list ${args.bundle[0]} does not contain ${want}: the client build did not record the bundled packages`);
      }
    }
    bundled = list.packages.map((p) => readPackageLicense(p.dir));
    unattributed = list.unattributed;
    manual = loadManualList(args.manual[0]);
  } catch (e) {
    console.error(`licenses-check: cannot check npm and manual licenses: ${e instanceof Error ? e.message : e}`);
    return 2;
  }

  const failures = [
    ...unattributed.map(
      (u) => `licenses-check: FAIL bundle ${describeUnattributed(u)} is in the client bundle but is neither project source nor a file of an npm package, so its license cannot be checked; attribute it in scripts/licenses/bundle.mjs or keep it out of the bundle`,
    ),
    ...checkComponents(bundled, accepted).map((f) => describe("npm", f)),
    ...checkComponents(manual, accepted).map((f) => describe("manual", f)),
  ];
  for (const line of failures) console.error(line);
  if (failures.length > 0) {
    console.error(`licenses-check: ${failures.length} component(s) outside the accepted list; an exception is an owner decision (docs/decisions.md), never an about.toml edit alone`);
    return 1;
  }
  console.log(`licenses-check: ok: ${bundled.length} bundled npm package(s) and ${manual.length} manual entr(y/ies) accepted`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
