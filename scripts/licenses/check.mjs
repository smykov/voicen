// License check for the npm bundle and the hand-kept list (T-027).
//
//   node scripts/licenses/check.mjs --about about.toml \
//     --bundle target/licenses/npm-bundled.json --manual licenses/manual.json
//
// Exit 0: every component accepted. Exit 1: license failures, each named on stderr.
// Exit 2: cannot check (missing or empty bundle list, unreadable about.toml or manual list).
// Stub: the developer implements it; the tests in check.test.mjs define the contract.
import { pathToFileURL } from "node:url";

/**
 * Every component whose license does not satisfy `accepted`, in input order.
 * reason: "missing" (absent, empty, UNLICENSED, UNKNOWN) | "not-accepted".
 * @param {{ name: string, version?: string, license?: string | null }[]} components
 * @param {string[]} accepted
 * @returns {{ name: string, version?: string, license: string | null, reason: "missing" | "not-accepted" }[]}
 */
export function checkComponents(components, accepted) {
  void components;
  void accepted;
  throw new Error("not implemented (T-027)");
}

/**
 * Reads licenses/manual.json: a JSON array of { name, version?, license, url?, note? }.
 * Throws (message names the file) when it is missing, not an array, or an entry has no name.
 * @param {string} file
 * @returns {{ name: string, version?: string, license: string | null }[]}
 */
export function loadManualList(file) {
  void file;
  throw new Error("not implemented (T-027)");
}

/**
 * @param {string[]} argv
 * @returns {number} exit code
 */
export function main(argv) {
  void argv;
  throw new Error("not implemented (T-027)");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
