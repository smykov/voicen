// T-027 red tests: the shipped npm set comes from the client bundle's module ids.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { packageOfModuleId, packagesFromModuleIds, loadBundleList, readPackageLicense } from "./bundle.mjs";

const NM = "/work/voicen/node_modules";

test("pnpm path maps to the package under the inner node_modules", () => {
  assert.deepEqual(
    packageOfModuleId(`${NM}/.pnpm/svelte@5.57.1/node_modules/svelte/src/internal/client/index.js`),
    { name: "svelte", dir: `${NM}/.pnpm/svelte@5.57.1/node_modules/svelte` },
  );
});

test("scoped pnpm path with a peer suffix maps to @scope/name", () => {
  const dir = `${NM}/.pnpm/@sveltejs+kit@2.70.3_@sveltejs+vite-plugin-svelte@7.1.2_svelte@5.57.1_vite@8.0.16/node_modules/@sveltejs/kit`;
  assert.deepEqual(packageOfModuleId(`${dir}/src/runtime/client/entry.js`), { name: "@sveltejs/kit", dir });
});

test("Windows backslash ids map to the package, dir with forward slashes", () => {
  const id = "C:\\work\\voicen\\node_modules\\.pnpm\\@tauri-apps+api@2.12.1\\node_modules\\@tauri-apps\\api\\core.js";
  assert.deepEqual(packageOfModuleId(id), {
    name: "@tauri-apps/api",
    dir: "C:/work/voicen/node_modules/.pnpm/@tauri-apps+api@2.12.1/node_modules/@tauri-apps/api",
  });
});

test("a nested node_modules maps to the innermost package", () => {
  assert.deepEqual(
    packageOfModuleId(`${NM}/.pnpm/a@1.0.0/node_modules/a/node_modules/@b/c/index.js`),
    { name: "@b/c", dir: `${NM}/.pnpm/a@1.0.0/node_modules/a/node_modules/@b/c` },
  );
});

test("a flat (npm-style) node_modules path maps to the package", () => {
  assert.deepEqual(packageOfModuleId(`${NM}/clsx/dist/clsx.mjs`), { name: "clsx", dir: `${NM}/clsx` });
});

test("a query suffix is ignored", () => {
  assert.deepEqual(
    packageOfModuleId(`${NM}/.pnpm/devalue@5.1.1/node_modules/devalue/index.js?v=4f2a91c0`),
    { name: "devalue", dir: `${NM}/.pnpm/devalue@5.1.1/node_modules/devalue` },
  );
});

test("a NUL-prefixed virtual id wrapping a node_modules file maps to that package", () => {
  assert.deepEqual(
    packageOfModuleId(`\0${NM}/.pnpm/esm-env@1.2.2/node_modules/esm-env/index.js?commonjs-proxy`),
    { name: "esm-env", dir: `${NM}/.pnpm/esm-env@1.2.2/node_modules/esm-env` },
  );
});

// Review round 1, finding 1: virtual ids no longer "map to no package" silently. Whether
// one is attributed (Vite's helpers -> vite) or fails the check is pinned in plugin.test.mjs.
test("project sources map to no package", () => {
  for (const id of [
    "/work/voicen/src/routes/+page.svelte",
    "/work/voicen/.svelte-kit/generated/client/app.js",
    "/work/voicen/src/lib/node_modules_helper.ts",
    "C:\\work\\voicen\\src\\lib\\i18n.ts",
  ]) {
    assert.equal(packageOfModuleId(id), null, JSON.stringify(id));
  }
});

test("packagesFromModuleIds: one entry per package dir, project sources dropped, sorted by name", () => {
  const svelte = `${NM}/.pnpm/svelte@5.57.1/node_modules/svelte`;
  const kit = `${NM}/.pnpm/@sveltejs+kit@2.70.3/node_modules/@sveltejs/kit`;
  const clsxA = `${NM}/.pnpm/clsx@2.1.1/node_modules/clsx`;
  const clsxB = `${NM}/.pnpm/clsx@1.2.1/node_modules/clsx`;
  const ids = [
    `${svelte}/src/index-client.js`,
    `${kit}/src/runtime/client/entry.js`,
    `${svelte}/src/internal/client/index.js`,
    "/work/voicen/src/routes/+page.svelte",
    `${clsxA}/dist/clsx.mjs`,
    `${clsxB}/dist/clsx.mjs`,
    `${kit}/src/runtime/app/state/index.js`,
  ];
  assert.deepEqual(packagesFromModuleIds(ids), [
    { name: "@sveltejs/kit", dir: kit },
    { name: "clsx", dir: clsxB },
    { name: "clsx", dir: clsxA },
    { name: "svelte", dir: svelte },
  ]);
});

function tmp() {
  return mkdtempSync(join(tmpdir(), "voicen-licenses-"));
}

test("loadBundleList: a missing bundle list fails, naming the file", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "npm-bundled.json");
  assert.throws(() => loadBundleList(file), (e) => e instanceof Error && e.message.includes(file));
});

test("loadBundleList: an empty bundle list fails instead of passing vacuously", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "npm-bundled.json");
  writeFileSync(file, "[]\n");
  assert.throws(() => loadBundleList(file), (e) => e instanceof Error && e.message.includes(file) && /empty/i.test(e.message));
});

test("loadBundleList: invalid JSON or a non-array fails, naming the file", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "npm-bundled.json");
  writeFileSync(file, "{ not json");
  assert.throws(() => loadBundleList(file), (e) => e instanceof Error && e.message.includes(file));
  writeFileSync(file, '{"packages": []}');
  assert.throws(() => loadBundleList(file), (e) => e instanceof Error && e.message.includes(file));
});

test("loadBundleList: returns the listed packages", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "npm-bundled.json");
  const entries = [{ name: "svelte", version: "5.57.1", dir: `${NM}/.pnpm/svelte@5.57.1/node_modules/svelte` }];
  writeFileSync(file, JSON.stringify(entries));
  assert.deepEqual(loadBundleList(file), entries);
});

test("readPackageLicense: reads name, version and license from package.json", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const pkg = join(dir, "node_modules", "@example", "fake-lib");
  mkdirSync(pkg, { recursive: true });
  writeFileSync(join(pkg, "package.json"), JSON.stringify({ name: "@example/fake-lib", version: "1.2.3", license: "GPL-3.0-only" }));
  assert.deepEqual(readPackageLicense(pkg), { name: "@example/fake-lib", version: "1.2.3", license: "GPL-3.0-only" });
});

test("readPackageLicense: an absent license field is null, not a default", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "no-license-lib", version: "0.0.1" }));
  assert.deepEqual(readPackageLicense(dir), { name: "no-license-lib", version: "0.0.1", license: null });
});

test("readPackageLicense: a missing package.json fails, naming the file", (t) => {
  const dir = tmp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  assert.throws(() => readPackageLicense(dir), (e) => e instanceof Error && e.message.includes(join(dir, "package.json")));
});
