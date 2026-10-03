// T-027 review round 1, finding 1: the bundle check fails closed. Every module the client
// bundle holds and every asset it emits is either project source or attributed to an npm
// package, which is then checked and listed like any other. Anything else (a virtual id no
// rule attributes, a file outside the project, a node_modules file of no package) makes the
// check fail, naming it.
//
// The tests drive the real plugin from vite.config.js ("voicen-bundled-packages") with a
// fake bundle and a fake project root in a temp dir, then run the two commands the gate runs
// on what the plugin wrote: check.mjs (judges) and notices.mjs (lists). They do not depend on
// the format of the bundle list, only on the plugin -> check/notices contract.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const VITE_CONFIG = fileURLToPath(new URL("../../vite.config.js", import.meta.url));
const CHECK = fileURLToPath(new URL("./check.mjs", import.meta.url));
const NOTICES = fileURLToPath(new URL("./notices.mjs", import.meta.url));
const PLUGIN = "voicen-bundled-packages";

/** A fresh instance of the bundle-recording plugin of vite.config.js. */
async function bundledPackagesPlugin() {
  const mod = await import(pathToFileURL(VITE_CONFIG).href);
  const config = typeof mod.default === "function"
    ? await mod.default({ command: "build", mode: "production", isSsrBuild: false, isPreview: false })
    : mod.default;
  const plugins = (await Promise.all(config.plugins ?? [])).flat(Infinity);
  const plugin = plugins.find((p) => p && p.name === PLUGIN);
  assert.ok(plugin, `vite.config.js has no plugin named ${PLUGIN}`);
  return plugin;
}

/** Calls a plugin hook given as a function or as { handler }. */
async function callHook(plugin, name, ctx, ...args) {
  const hook = plugin[name];
  const fn = typeof hook === "function" ? hook : hook?.handler;
  if (fn) await fn.call(ctx, ...args);
}

const PLUGIN_CONTEXT = {
  warn() {},
  info() {},
  debug() {},
  error(e) {
    throw e instanceof Error ? e : new Error(typeof e === "string" ? e : JSON.stringify(e));
  },
  emitFile() {
    return "";
  },
  meta: { watchMode: false },
};

/**
 * Runs the plugin on a fake bundle: one chunk holding `moduleIds`, one asset per entry of
 * `assets` (its originalFileNames, root-relative as Vite reports them). Returns the path the
 * plugin writes the bundle list to.
 */
async function runPlugin(root, { moduleIds, assets = [], ssr = false }) {
  const plugin = await bundledPackagesPlugin();
  await callHook(plugin, "configResolved", PLUGIN_CONTEXT, { root, command: "build", build: { ssr, outDir: join(root, "build") } });
  /** @type {Record<string, unknown>} */
  const bundle = {
    "_app/immutable/entry/app.js": {
      type: "chunk",
      fileName: "_app/immutable/entry/app.js",
      name: "app",
      isEntry: true,
      moduleIds,
      modules: Object.fromEntries(moduleIds.map((id) => [id, { code: "", renderedLength: 0 }])),
      code: "",
      imports: [],
      dynamicImports: [],
    },
  };
  assets.forEach((originalFileNames, i) => {
    const fileName = `_app/immutable/assets/asset-${i}.bin`;
    bundle[fileName] = { type: "asset", fileName, names: [], originalFileNames, source: "" };
  });
  await callHook(plugin, "generateBundle", PLUGIN_CONTEXT, { dir: join(root, "build") }, bundle, false);
  return join(root, "target", "licenses", "npm-bundled.json");
}

/** A package in the pnpm layout: node_modules/.pnpm/<name>@<version>/node_modules/<name>. */
function fakePackage(root, { name, version, license, extra = {} }) {
  const dir = join(root, "node_modules", ".pnpm", `${name.replace("/", "+")}@${version}`, "node_modules", ...name.split("/"));
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name, version, license, ...extra }));
  writeFileSync(join(dir, "LICENSE"), `${license} license text of ${name} (example)\n`);
  return dir;
}

function link(target, path) {
  mkdirSync(dirname(path), { recursive: true });
  symlinkSync(relative(dirname(path), target), path);
}

/**
 * A fake project root shaped like the real one (pnpm): svelte and vite are direct
 * dependencies, rolldown is reachable only through vite (as in the real tree), plus
 * `others` extra packages.
 */
function fakeProject(t, { viteLicense = "MIT", rolldownLicense = "MIT", others = [] } = {}) {
  const root = mkdtempSync(join(tmpdir(), "voicen-plugin-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, "package.json"), JSON.stringify({ name: "voicen-fixture", private: true, type: "module", devDependencies: { svelte: "5.57.1", vite: "8.3.2" } }));
  writeFileSync(join(root, "about.toml"), 'accepted = ["MIT"]\n');
  writeFileSync(join(root, "manual.json"), "[]\n");
  writeFileSync(join(root, "rust.txt"), "Rust part (example)\n");

  const svelte = fakePackage(root, { name: "svelte", version: "5.57.1", license: "MIT", extra: { exports: { ".": "./src/index-client.js", "./package.json": "./package.json" } } });
  link(svelte, join(root, "node_modules", "svelte"));
  const vite = fakePackage(root, { name: "vite", version: "8.3.2", license: viteLicense, extra: { exports: { ".": "./dist/node/index.js", "./package.json": "./package.json" } } });
  link(vite, join(root, "node_modules", "vite"));
  const rolldown = fakePackage(root, { name: "rolldown", version: "1.2.12", license: rolldownLicense, extra: { exports: { ".": "./dist/index.mjs", "./package.json": "./package.json" } } });
  link(rolldown, join(root, "node_modules", ".pnpm", "vite@8.3.2", "node_modules", "rolldown"));
  const dirs = { svelte, vite, rolldown };
  for (const o of others) dirs[o.name] = fakePackage(root, o);
  return { root, dirs };
}

/** The module ids of the real client bundle today (vite build, 2026-10-03), under `root`. */
function realShapedModuleIds(root, svelteDir) {
  return [
    `${root}/.svelte-kit/generated/client-optimized/app.js`,
    `${root}/.svelte-kit/generated/client-optimized/nodes/0.js`,
    `${root}/.svelte-kit/generated/root.svelte`,
    `${root}/src/routes/+page.svelte`,
    `${root}/src/lib/i18n.ts`,
    `${root}/i18n/en.json`,
    `${svelteDir}/src/internal/client/index.js`,
    "\0vite/preload-helper.js",
    "\0rolldown/runtime.js",
  ];
}

/** Assets of the real client bundle today: CSS from a generated node, SvelteKit's version.json. */
const REAL_SHAPED_ASSETS = [[".svelte-kit/generated/client-optimized/nodes/2.js"], []];

function check(root, list) {
  return spawnSync(process.execPath, [CHECK, "--about", join(root, "about.toml"), "--bundle", list, "--manual", join(root, "manual.json"), "--require", "svelte"], { cwd: root, encoding: "utf8" });
}

function notices(root, list) {
  const out = join(root, "NOTICES.txt");
  const r = spawnSync(process.execPath, [NOTICES, "--rust", join(root, "rust.txt"), "--bundle", list, "--manual", join(root, "manual.json"), "--out", out], { cwd: root, encoding: "utf8" });
  return { ...r, text: existsSync(out) ? readFileSync(out, "utf8") : "" };
}

// --- characterization: what must keep passing -------------------------------------------

test("characterization: the real-shaped client bundle (project sources, SvelteKit assets, Vite and rolldown helpers, accepted packages) passes the check", async (t) => {
  const { root, dirs } = fakeProject(t);
  const list = await runPlugin(root, { moduleIds: realShapedModuleIds(root, dirs.svelte), assets: REAL_SHAPED_ASSETS });
  const r = check(root, list);
  assert.equal(r.status, 0, r.stderr);
});

test("characterization: the SSR build (prerendering, does not ship) records nothing", async (t) => {
  const { root } = fakeProject(t);
  const list = await runPlugin(root, { moduleIds: ["\0virtual:example-ssr-only"], ssr: true });
  assert.equal(existsSync(list), false, "the SSR build must not write the bundle list");
});

// --- attribution: Vite's and the bundler's helpers are checked and listed ----------------

test("Vite's helper \\0vite/preload-helper.js is attributed to the vite package: a non-accepted vite license fails the check, naming vite", async (t) => {
  const { root, dirs } = fakeProject(t, { viteLicense: "GPL-3.0-only" });
  const list = await runPlugin(root, { moduleIds: [`${dirs.svelte}/src/index-client.js`, "\0vite/preload-helper.js"] });
  const r = check(root, list);
  assert.equal(r.status, 1, `expected a license failure naming vite; stderr: ${r.stderr}`);
  assert.match(r.stderr, /\bvite@8\.3\.2\b/);
  assert.match(r.stderr, /GPL-3\.0-only/);
});

test("Vite's helper is listed in the notices with the vite package's version and license text", async (t) => {
  const { root, dirs } = fakeProject(t);
  const list = await runPlugin(root, { moduleIds: [`${dirs.svelte}/src/index-client.js`, "\0vite/preload-helper.js"] });
  const r = notices(root, list);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.text, /^vite 8\.3\.2\nLicense: MIT\n/m);
  assert.ok(r.text.includes("MIT license text of vite (example)"), "vite's license text is in the notices");
});

test("the bundler runtime \\0rolldown/runtime.js is attributed to rolldown (resolved through vite): a non-accepted rolldown license fails the check, naming rolldown", async (t) => {
  const { root, dirs } = fakeProject(t, { rolldownLicense: "GPL-3.0-only" });
  const list = await runPlugin(root, { moduleIds: [`${dirs.svelte}/src/index-client.js`, "\0rolldown/runtime.js"] });
  const r = check(root, list);
  assert.equal(r.status, 1, `expected a license failure naming rolldown; stderr: ${r.stderr}`);
  assert.match(r.stderr, /\brolldown@1\.2\.12\b/);
});

// --- fail closed: what no rule attributes fails, named --------------------------------

test("a new unattributed virtual id in the client bundle fails the check, naming the id", async (t) => {
  const { root, dirs } = fakeProject(t);
  const list = await runPlugin(root, { moduleIds: [`${dirs.svelte}/src/index-client.js`, "\0virtual:example-icons/star.js"] });
  const r = check(root, list);
  assert.notEqual(r.status, 0, "an unattributed module must not pass the check");
  assert.ok(r.stderr.includes("virtual:example-icons/star.js"), `stderr names the id: ${r.stderr}`);
});

test("a virtual id without the NUL prefix and a module outside the project root fail the check, naming each", async (t) => {
  const { root, dirs } = fakeProject(t);
  const list = await runPlugin(root, {
    moduleIds: [`${dirs.svelte}/src/index-client.js`, "virtual:example-plugin/runtime", "/opt/example-elsewhere/lib/helper.js"],
  });
  const r = check(root, list);
  assert.notEqual(r.status, 0, "unattributed modules must not pass the check");
  assert.ok(r.stderr.includes("virtual:example-plugin/runtime"), `stderr names the virtual id: ${r.stderr}`);
  assert.ok(r.stderr.includes("/opt/example-elsewhere/lib/helper.js"), `stderr names the outside file: ${r.stderr}`);
});

test("an emitted asset from a node_modules package is attributed to that package: a non-accepted license fails the check, naming it", async (t) => {
  const { root, dirs } = fakeProject(t, { others: [{ name: "example-font", version: "1.0.0", license: "GPL-3.0-only" }] });
  const list = await runPlugin(root, {
    moduleIds: [`${dirs.svelte}/src/index-client.js`],
    assets: [["node_modules/.pnpm/example-font@1.0.0/node_modules/example-font/files/regular.woff2"]],
  });
  const r = check(root, list);
  assert.equal(r.status, 1, `expected a license failure naming example-font; stderr: ${r.stderr}`);
  assert.match(r.stderr, /\bexample-font@1\.0\.0\b/);
});

test("an emitted asset from node_modules that maps to no package fails the check, naming it", async (t) => {
  const { root, dirs } = fakeProject(t);
  const list = await runPlugin(root, {
    moduleIds: [`${dirs.svelte}/src/index-client.js`],
    assets: [["node_modules/.vite/example/blob.bin"]],
  });
  const r = check(root, list);
  assert.notEqual(r.status, 0, "an unattributed asset must not pass the check");
  assert.ok(r.stderr.includes("node_modules/.vite/example/blob.bin"), `stderr names the asset: ${r.stderr}`);
});
