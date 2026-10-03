// The npm packages that ship: derived from the client bundle's module ids (T-027).
// The Vite plugin in vite.config.js calls packagesFromModuleIds + readPackageLicense and
// writes the result; check.mjs and notices.mjs read it back with loadBundleList.
// vite.config.js imports this file, so svelte-check type-checks it; the project has no
// @types/node, hence the same expect-error as in vite.config.js.
// @ts-expect-error type error without @types/node package
import { readFileSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import { join } from "node:path";

const NODE_MODULES = "/node_modules/";

/**
 * The package a bundled module id belongs to, or null when it is not under node_modules.
 * Handles pnpm `.pnpm/<id>/node_modules/<name>` paths, scopes, Windows backslashes,
 * query suffixes and NUL-prefixed (virtual) ids. `dir` uses forward slashes.
 * @param {string} id
 * @returns {{ name: string, dir: string } | null}
 */
export function packageOfModuleId(id) {
  let path = id.startsWith("\0") ? id.slice(1) : id;
  const query = path.indexOf("?");
  if (query !== -1) path = path.slice(0, query);
  path = path.replace(/\\/g, "/");

  // The innermost node_modules segment owns the file (pnpm: .pnpm/<id>/node_modules/<name>).
  const at = path.lastIndexOf(NODE_MODULES);
  if (at === -1) return null;
  const base = path.slice(0, at + NODE_MODULES.length);
  const parts = path.slice(base.length).split("/");
  const nameLength = parts[0].startsWith("@") ? 2 : 1;
  // A file of the package, so at least one segment after the name; names never start
  // with "." (.pnpm, .vite are store and cache directories, not packages).
  if (parts.length <= nameLength || parts[0].startsWith(".") || parts.slice(0, nameLength).some((p) => p === "")) {
    return null;
  }
  const name = parts.slice(0, nameLength).join("/");
  return { name, dir: base + name };
}

/** Code-point order, the same on every machine and locale. */
function compare(/** @type {string} */ a, /** @type {string} */ b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * The distinct packages of a list of module ids, sorted by name then dir.
 * @param {string[]} ids
 * @returns {{ name: string, dir: string }[]}
 */
export function packagesFromModuleIds(ids) {
  /** @type {Map<string, { name: string, dir: string }>} */
  const byDir = new Map();
  for (const id of ids) {
    const pkg = packageOfModuleId(id);
    if (pkg && !byDir.has(pkg.dir)) byDir.set(pkg.dir, pkg);
  }
  return [...byDir.values()].sort((a, b) => compare(a.name, b.name) || compare(a.dir, b.dir));
}

/**
 * Reads the bundle list written by the Vite plugin: a JSON array of { name, version, dir }.
 * Throws (message names the file) when it is missing, not JSON, not an array, or empty.
 * @param {string} file
 * @returns {{ name: string, version: string, dir: string }[]}
 */
export function loadBundleList(file) {
  let data;
  try {
    data = JSON.parse(readFileSync(file, "utf8"));
  } catch (e) {
    throw new Error(`cannot read the bundle list ${file} (run the client build: pnpm build): ${e instanceof Error ? e.message : e}`);
  }
  if (!Array.isArray(data)) throw new Error(`bundle list ${file} is not a JSON array`);
  if (data.length === 0) {
    throw new Error(`bundle list ${file} is empty: the client build recorded no npm package, so nothing can be checked`);
  }
  for (const entry of data) {
    if (!entry || typeof entry.name !== "string" || typeof entry.version !== "string" || typeof entry.dir !== "string") {
      throw new Error(`bundle list ${file}: every entry needs string name, version and dir, got ${JSON.stringify(entry)}`);
    }
  }
  return data;
}

/**
 * name, version and license of the package in `dir`, from its package.json.
 * `license` is null when the field is absent. Throws (message names the file) when
 * package.json is missing.
 * @param {string} dir
 * @returns {{ name: string, version: string, license: string | null }}
 */
export function readPackageLicense(dir) {
  const file = join(dir, "package.json");
  let pkg;
  try {
    pkg = JSON.parse(readFileSync(file, "utf8"));
  } catch (e) {
    throw new Error(`cannot read ${file}: ${e instanceof Error ? e.message : e}`);
  }
  // npm also allows the legacy object form { "type": "MIT" }; any other shape is no license.
  const raw = pkg.license;
  const license = typeof raw === "string" ? raw : raw && typeof raw.type === "string" ? raw.type : null;
  return { name: String(pkg.name), version: String(pkg.version), license };
}
