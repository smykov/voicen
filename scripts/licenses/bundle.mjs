// The npm packages that ship: derived from the client bundle (T-027).
// The Vite plugin in vite.config.js calls classifyBundle + readPackageLicense and writes the
// result; check.mjs and notices.mjs read it back with loadBundleList + splitBundleList.
// Fail closed (review round 1): every bundled module and emitted asset is project source or
// attributed to a package; anything else is written to the list as "unattributed" and fails
// the check, naming it.
// vite.config.js imports this file, so svelte-check type-checks it; the project has no
// @types/node, hence the same expect-error as in vite.config.js.
// @ts-expect-error type error without @types/node package
import { existsSync, readFileSync, realpathSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import { join, resolve } from "node:path";

const NODE_MODULES = "/node_modules/";

/**
 * The package a bundled module id belongs to, or null when it is not under node_modules.
 * Handles pnpm `.pnpm/<id>/node_modules/<name>` paths, scopes, Windows backslashes,
 * query suffixes and NUL-prefixed (virtual) ids. `dir` uses forward slashes.
 * @param {string} id
 * @returns {{ name: string, dir: string } | null}
 */
export function packageOfModuleId(id) {
  const path = normalizePath(id);

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

/** Forward slashes, no NUL prefix, no query suffix, no trailing slash. */
function normalizePath(/** @type {string} */ id) {
  let path = id.startsWith("\0") ? id.slice(1) : id;
  const query = path.indexOf("?");
  if (query !== -1) path = path.slice(0, query);
  return path.replace(/\\/g, "/").replace(/(.)\/+$/, "$1");
}

/**
 * The real directory of package `name` as Node resolves it from `fromDir` (each ancestor's
 * node_modules, nearest first; package `exports` are not consulted), or null.
 * @param {string} name
 * @param {string} fromDir
 * @returns {string | null}
 */
export function findPackageDir(name, fromDir) {
  let dir = normalizePath(fromDir);
  while (dir !== "") {
    if (!dir.endsWith("/node_modules")) {
      const candidate = `${dir}/node_modules/${name}`;
      if (existsSync(`${candidate}/package.json`)) return normalizePath(realpathSync(candidate));
    }
    const slash = dir.lastIndexOf("/");
    dir = slash > 0 ? dir.slice(0, slash) : "";
  }
  return null;
}

/**
 * Virtual modules of the build tool itself, attributed to the package that generates them.
 * Each rule names a package by its own id namespace; any other virtual id is unattributed.
 * Vite: `\0vite/preload-helper.js`, `\0vite/modulepreload-polyfill.js`, ... (vite 8.3.2).
 * Rolldown (Vite 8's bundler): `\0rolldown/runtime.js`; resolved through vite's directory,
 * because under pnpm rolldown is a dependency of vite, not of the project.
 * @type {{ prefix: string, resolve: (root: string) => string | null }[]}
 */
const BUILD_TOOL_MODULES = [
  { prefix: "\0vite/", resolve: (root) => findPackageDir("vite", root) },
  {
    prefix: "\0rolldown/",
    resolve: (root) => {
      const vite = findPackageDir("vite", root);
      return vite === null ? null : findPackageDir("rolldown", vite);
    },
  },
];

/**
 * Sorts every bundled module id and emitted asset file into npm packages and unattributed
 * entries. Project sources (files under `root`, outside node_modules) are dropped: they are
 * ours. A module or asset is attributed to a package when its path lies in the package's
 * directory under node_modules, or when it is a build-tool virtual module (BUILD_TOOL_MODULES).
 * Everything else is unattributed: a virtual id no rule covers (with or without the `\0`
 * prefix), a file outside `root`, a node_modules file of no package. Assets are given by
 * their original file names (absolute or relative to `root`); an asset with no original file
 * is generated by a plugin (SvelteKit's version.json) and is not passed here.
 * @param {{ root: string, moduleIds: string[], assetFiles?: string[] }} input
 * @returns {{ packages: { name: string, dir: string }[], unattributed: { kind: "module" | "asset", id: string }[] }}
 */
export function classifyBundle({ root, moduleIds, assetFiles = [] }) {
  const base = normalizePath(root);
  // Without an absolute root every path would count as project source: refuse instead.
  if (!/^(\/|[A-Za-z]:\/)/.test(base)) throw new Error(`classifyBundle: the project root must be an absolute path, got ${JSON.stringify(root)}`);
  /** @type {string[]} */
  const attributed = [];
  /** @type {{ kind: "module" | "asset", id: string }[]} */
  const unattributed = [];

  /** @returns {boolean} true when `path` (normalized, absolute) is a package file or ours */
  const place = (/** @type {string} */ path) => {
    if (packageOfModuleId(path)) {
      attributed.push(path);
      return true;
    }
    return !`${path}/`.includes("/node_modules/") && path.startsWith(`${base}/`);
  };

  for (const id of moduleIds) {
    const tool = BUILD_TOOL_MODULES.find((rule) => id.startsWith(rule.prefix));
    if (tool) {
      // Attributed only when the package resolves to a directory under node_modules.
      const dir = tool.resolve(base);
      const file = dir === null ? null : `${dir}/package.json`;
      if (file !== null && packageOfModuleId(file)) attributed.push(file);
      else unattributed.push({ kind: "module", id });
      continue;
    }
    if (!place(normalizePath(id))) unattributed.push({ kind: "module", id });
  }
  for (const file of assetFiles) {
    if (!place(normalizePath(resolve(base, file)))) unattributed.push({ kind: "asset", id: file });
  }

  /** @type {Map<string, { kind: "module" | "asset", id: string }>} */
  const unique = new Map(unattributed.map((u) => [`${u.kind}:${u.id}`, u]));
  return {
    packages: packagesFromModuleIds(attributed),
    unattributed: [...unique.values()].sort((a, b) => compare(a.kind, b.kind) || compare(a.id, b.id)),
  };
}

/**
 * @typedef {{ name: string, version: string, dir: string }} BundledPackage
 * @typedef {{ unattributed: string, kind: "module" | "asset" }} UnattributedEntry
 */

/**
 * Reads the bundle list written by the Vite plugin: a JSON array of packages
 * { name, version, dir } and unattributed entries { unattributed, kind }.
 * Throws (message names the file) when it is missing, not JSON, not an array, empty, or an
 * entry is neither.
 * @param {string} file
 * @returns {(BundledPackage | UnattributedEntry)[]}
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
    const pkg = entry && typeof entry.name === "string" && typeof entry.version === "string" && typeof entry.dir === "string";
    const other = entry && typeof entry.unattributed === "string" && (entry.kind === "module" || entry.kind === "asset");
    if (!pkg && !other) {
      throw new Error(`bundle list ${file}: every entry needs string name, version and dir (a package) or unattributed and kind (module | asset), got ${JSON.stringify(entry)}`);
    }
  }
  return data;
}

/**
 * The packages and the unattributed entries of a bundle list.
 * @param {(BundledPackage | UnattributedEntry)[]} list
 * @returns {{ packages: BundledPackage[], unattributed: UnattributedEntry[] }}
 */
export function splitBundleList(list) {
  /** @type {BundledPackage[]} */
  const packages = [];
  /** @type {UnattributedEntry[]} */
  const unattributed = [];
  for (const entry of list) {
    if ("unattributed" in entry) unattributed.push(entry);
    else packages.push(entry);
  }
  return { packages, unattributed };
}

/** How an unattributed entry is named in errors: the id as JSON, so a NUL prefix shows. */
export function describeUnattributed(/** @type {UnattributedEntry} */ entry) {
  return `${entry.kind} ${JSON.stringify(entry.unattributed)}`;
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
