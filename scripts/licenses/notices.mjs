// Generator of THIRD-PARTY-NOTICES.txt from the cargo-about output, the npm bundle list
// and licenses/manual.json (T-027). Called by `make licenses` / `make licenses-check`.
// Stub: the developer implements it.
import { pathToFileURL } from "node:url";

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
