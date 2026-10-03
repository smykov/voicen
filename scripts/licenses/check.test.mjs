// T-027 red tests: the npm bundle and the hand-kept list are checked against about.toml,
// and every failing component is named.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkComponents, loadManualList } from "./check.mjs";
import { readAcceptedList } from "./accepted.mjs";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));
const CHECK = fileURLToPath(new URL("./check.mjs", import.meta.url));
const ACCEPTED = ["MIT", "Apache-2.0", "ISC", "Apache-2.0 WITH LLVM-exception"];

// --- checkComponents --------------------------------------------------------------

test("all accepted components produce no failure", () => {
  assert.deepEqual(
    checkComponents([
      { name: "svelte", version: "5.57.1", license: "MIT" },
      { name: "@tauri-apps/api", version: "2.12.1", license: "Apache-2.0 OR MIT" },
    ], ACCEPTED),
    [],
  );
});

test("a GPL-3.0 package fails and is named", () => {
  assert.deepEqual(
    checkComponents([
      { name: "svelte", version: "5.57.1", license: "MIT" },
      { name: "fake-gpl-lib", version: "1.0.0", license: "GPL-3.0" },
    ], ACCEPTED),
    [{ name: "fake-gpl-lib", version: "1.0.0", license: "GPL-3.0", reason: "not-accepted" }],
  );
});

test("a missing or UNLICENSED license fails with reason missing", () => {
  const failures = checkComponents([
    { name: "no-field", version: "1.0.0" },
    { name: "null-field", version: "1.0.0", license: null },
    { name: "empty-field", version: "1.0.0", license: "" },
    { name: "proprietary", version: "1.0.0", license: "UNLICENSED" },
    { name: "unknown", version: "1.0.0", license: "UNKNOWN" },
  ], ACCEPTED);
  assert.deepEqual(failures.map((f) => [f.name, f.reason]), [
    ["no-field", "missing"],
    ["null-field", "missing"],
    ["empty-field", "missing"],
    ["proprietary", "missing"],
    ["unknown", "missing"],
  ]);
});

// Review round 1, finding 4: one "no license" set (spdx.mjs) for the evaluator and the
// report, so NONE and NOASSERTION are reported as "no license", not as "not accepted".
test("NONE and NOASSERTION are reported as no license (reason missing), like UNLICENSED", () => {
  const failures = checkComponents([
    { name: "spdx-none", version: "1.0.0", license: "NONE" },
    { name: "spdx-noassertion", version: "1.0.0", license: "NOASSERTION" },
  ], ACCEPTED);
  assert.deepEqual(failures.map((f) => [f.name, f.reason]), [
    ["spdx-none", "missing"],
    ["spdx-noassertion", "missing"],
  ]);
});

test("MIT OR GPL-3.0 passes, MIT AND GPL-3.0 fails", () => {
  const failures = checkComponents([
    { name: "either", version: "1.0.0", license: "MIT OR GPL-3.0" },
    { name: "both", version: "1.0.0", license: "MIT AND GPL-3.0" },
  ], ACCEPTED);
  assert.deepEqual(failures.map((f) => f.name), ["both"]);
});

test("every failing component is reported, not only the first", () => {
  const failures = checkComponents([
    { name: "a-gpl", version: "1.0.0", license: "GPL-3.0-only" },
    { name: "b-ok", version: "1.0.0", license: "MIT" },
    { name: "c-agpl", version: "2.0.0", license: "AGPL-3.0-only" },
  ], ACCEPTED);
  assert.deepEqual(failures.map((f) => f.name), ["a-gpl", "c-agpl"]);
});

// --- manual list --------------------------------------------------------------------

function tmp(t) {
  const dir = mkdtempSync(join(tmpdir(), "voicen-licenses-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("loadManualList: a missing manual list fails, naming the file", (t) => {
  const file = join(tmp(t), "manual.json");
  assert.throws(() => loadManualList(file), (e) => e instanceof Error && e.message.includes(file));
});

test("loadManualList: a non-array or an entry without a name fails, naming the file", (t) => {
  const file = join(tmp(t), "manual.json");
  writeFileSync(file, JSON.stringify({ name: "whisper.cpp", license: "MIT" }));
  assert.throws(() => loadManualList(file), (e) => e instanceof Error && e.message.includes(file));
  writeFileSync(file, JSON.stringify([{ license: "MIT" }]));
  assert.throws(() => loadManualList(file), (e) => e instanceof Error && e.message.includes(file));
});

test("manual entries are checked like packages: a GPL entry is named", (t) => {
  const file = join(tmp(t), "manual.json");
  const entries = [
    { name: "fake-model", version: "1", license: "MIT", note: "downloaded at run time" },
    { name: "fake-gpl-tool", version: "2", license: "GPL-3.0-only" },
  ];
  writeFileSync(file, JSON.stringify(entries));
  const failures = checkComponents(loadManualList(file), ACCEPTED);
  assert.deepEqual(failures.map((f) => [f.name, f.reason]), [["fake-gpl-tool", "not-accepted"]]);
});

test("the committed licenses/manual.json lists whisper.cpp, Silero VAD, the whisper models and NSIS, all accepted", () => {
  const entries = loadManualList(`${ROOT}licenses/manual.json`);
  const names = entries.map((e) => e.name).join("\n");
  for (const want of [/whisper\.cpp/i, /silero/i, /whisper.*model|ggml.*model/i, /nsis/i]) {
    assert.match(names, want);
  }
  const accepted = readAcceptedList(readFileSync(`${ROOT}about.toml`, "utf8"));
  assert.deepEqual(checkComponents(entries, accepted), []);
});

// --- the command the gate runs ---------------------------------------------------------

/** A throwaway project: about.toml, node_modules with fake packages, bundle list, manual list. */
function project(t, { accepted = ["MIT", "Apache-2.0"], packages = [], manual = [] } = {}) {
  const dir = tmp(t);
  writeFileSync(join(dir, "about.toml"), `accepted = [${accepted.map((a) => JSON.stringify(a)).join(", ")}]\n`);
  const bundled = packages.map(({ name, version, license }) => {
    const pkgDir = join(dir, "node_modules", ".pnpm", `${name.replace("/", "+")}@${version}`, "node_modules", ...name.split("/"));
    mkdirSync(pkgDir, { recursive: true });
    const json = { name, version };
    if (license !== undefined) json.license = license;
    writeFileSync(join(pkgDir, "package.json"), JSON.stringify(json));
    return { name, version, dir: pkgDir };
  });
  mkdirSync(join(dir, "target", "licenses"), { recursive: true });
  const bundle = join(dir, "target", "licenses", "npm-bundled.json");
  writeFileSync(bundle, JSON.stringify(bundled));
  const manualFile = join(dir, "manual.json");
  writeFileSync(manualFile, JSON.stringify(manual));
  return { dir, about: join(dir, "about.toml"), bundle, manual: manualFile };
}

function run(p, overrides = {}) {
  const args = ["--about", p.about, "--bundle", overrides.bundle ?? p.bundle, "--manual", p.manual];
  return spawnSync(process.execPath, [CHECK, ...args], { encoding: "utf8" });
}

const OK_PKG = { name: "fake-ok-lib", version: "1.0.0", license: "MIT" };
const OK_MANUAL = { name: "fake-model", version: "1", license: "MIT" };

test("check.mjs exits 0 when every bundled package and manual entry is accepted", (t) => {
  const p = project(t, { packages: [OK_PKG, { name: "@example/scoped", version: "2.0.0", license: "Apache-2.0 OR MIT" }], manual: [OK_MANUAL] });
  const r = run(p);
  assert.equal(r.status, 0, r.stderr);
});

test("check.mjs exits 1 and names a bundled GPL-3.0 package with its license", (t) => {
  const p = project(t, { packages: [OK_PKG, { name: "fake-gpl-lib", version: "3.1.4", license: "GPL-3.0" }], manual: [OK_MANUAL] });
  const r = run(p);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /fake-gpl-lib/);
  assert.match(r.stderr, /GPL-3\.0/);
  assert.doesNotMatch(r.stderr, /fake-ok-lib/);
});

test("check.mjs exits 1 and names a bundled package without a license", (t) => {
  const p = project(t, { packages: [OK_PKG, { name: "fake-nolicense-lib", version: "0.1.0" }], manual: [OK_MANUAL] });
  const r = run(p);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /fake-nolicense-lib/);
});

test("check.mjs exits 1 and names a GPL entry of the manual list", (t) => {
  const p = project(t, { packages: [OK_PKG], manual: [OK_MANUAL, { name: "fake-gpl-tool", version: "9", license: "GPL-3.0-only" }] });
  const r = run(p);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /fake-gpl-tool/);
});

test("check.mjs reads the accepted list from the given about.toml for both npm and manual entries", (t) => {
  const p = project(t, { accepted: ["Apache-2.0"], packages: [OK_PKG], manual: [OK_MANUAL] });
  const r = run(p);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /fake-ok-lib/);
  assert.match(r.stderr, /fake-model/);
});

test("check.mjs exits 2 (cannot check), naming the file, when the bundle list is missing", (t) => {
  const p = project(t, { packages: [OK_PKG], manual: [OK_MANUAL] });
  const missing = join(p.dir, "target", "licenses", "absent.json");
  const r = run(p, { bundle: missing });
  assert.equal(r.status, 2, r.stderr);
  assert.ok(r.stderr.includes(missing), r.stderr);
});

test("check.mjs exits 2 (cannot check) when the bundle list is empty", (t) => {
  const p = project(t, { packages: [], manual: [OK_MANUAL] });
  const r = run(p);
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /empty/i);
});
