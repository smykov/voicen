// The npm packages that ship: derived from the client bundle's module ids (T-027).
// Stub: the developer implements it; the tests in bundle.test.mjs define the contract.

/**
 * The package a bundled module id belongs to, or null when it is not under node_modules.
 * Handles pnpm `.pnpm/<id>/node_modules/<name>` paths, scopes, Windows backslashes,
 * query suffixes and NUL-prefixed (virtual) ids. `dir` uses forward slashes.
 * @param {string} id
 * @returns {{ name: string, dir: string } | null}
 */
export function packageOfModuleId(id) {
  void id;
  throw new Error("not implemented (T-027)");
}

/**
 * The distinct packages of a list of module ids, sorted by name then dir.
 * @param {string[]} ids
 * @returns {{ name: string, dir: string }[]}
 */
export function packagesFromModuleIds(ids) {
  void ids;
  throw new Error("not implemented (T-027)");
}

/**
 * Reads the bundle list written by the Vite plugin: a JSON array of { name, version, dir }.
 * Throws (message names the file) when it is missing, not JSON, not an array, or empty.
 * @param {string} file
 * @returns {{ name: string, version: string, dir: string }[]}
 */
export function loadBundleList(file) {
  void file;
  throw new Error("not implemented (T-027)");
}

/**
 * name, version and license of the package in `dir`, from its package.json.
 * `license` is null when the field is absent. Throws (message names the file) when
 * package.json is missing.
 * @param {string} dir
 * @returns {{ name: string, version: string, license: string | null }}
 */
export function readPackageLicense(dir) {
  void dir;
  throw new Error("not implemented (T-027)");
}
