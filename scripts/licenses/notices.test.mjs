// T-027: THIRD-PARTY-NOTICES.txt generation and the --require sanity check of check.mjs
// (added by the developer: the Vite plugin must have recorded the bundle, and the notices
// must be deterministic and free of machine paths).
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { renderNotices } from "./notices.mjs";
import { workspaceMemberNames } from "./workspace.mjs";

const CHECK = fileURLToPath(new URL("./check.mjs", import.meta.url));

function tmp(t) {
  const dir = mkdtempSync(join(tmpdir(), "voicen-notices-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function fakePackage(root, { name, version, license, files }) {
  const dir = join(root, "node_modules", ".pnpm", `${name.replace("/", "+")}@${version}`, "node_modules", ...name.split("/"));
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name, version, license, repository: { url: `git+https://example.com/${name}.git` } }));
  for (const [file, text] of Object.entries(files)) writeFileSync(join(dir, file), text);
  return { name, version, dir };
}

test("notices: name, version, license and text of every component, no machine path, LF only", (t) => {
  const root = tmp(t);
  const a = fakePackage(root, { name: "@example/a", version: "1.0.0", license: "MIT", files: { "LICENSE.md": "MIT text of a\r\n" } });
  const b = fakePackage(root, { name: "b-lib", version: "2.0.0", license: "Apache-2.0 OR MIT", files: { "LICENSE-MIT": "MIT text of b", "LICENSE.spdx": "SPDXVersion: SPDX-2.1" } });
  const text = renderNotices({
    rustText: "Rust part\r\n",
    bundle: [b, a, a],
    manual: [
      { name: "model-x", license: "MIT", note: "downloaded at run time", text: "x.txt" },
      { name: "model-y", license: "MIT", text: "x.txt" },
    ],
    readText: (p) => `text of ${p}`,
  });
  assert.ok(!text.includes(root), "no machine path");
  assert.ok(!text.includes("\r"), "LF only");
  assert.ok(text.indexOf("@example/a 1.0.0") < text.indexOf("b-lib 2.0.0"), "sorted, listed once");
  assert.equal(text.split("@example/a 1.0.0").length, 2);
  assert.match(text, /License: Apache-2\.0 OR MIT\nSource: https:\/\/example\.com\/b-lib\.git\n\nMIT text of b/);
  assert.doesNotMatch(text, /SPDXVersion/, ".spdx metadata is not a license text");
  assert.match(text, /model-x\nLicense: MIT\ndownloaded at run time\n\ntext of x\.txt/);
  assert.match(text, /model-y\nLicense: MIT\n\nLicense text: see "model-x" above\./);
  assert.match(text, /Rust part/);
});

test("notices: a bundled package without a license file cannot be listed and throws, naming it", (t) => {
  const root = tmp(t);
  const p = fakePackage(root, { name: "no-text", version: "1.0.0", license: "MIT", files: {} });
  assert.throws(() => renderNotices({ rustText: "", bundle: [p], manual: [] }), /no-text@1\.0\.0/);
});

test("check.mjs --require: a bundle list without the required package cannot check (exit 2)", (t) => {
  const root = tmp(t);
  const p = fakePackage(root, { name: "fake-ok-lib", version: "1.0.0", license: "MIT", files: { LICENSE: "x" } });
  writeFileSync(join(root, "about.toml"), 'accepted = ["MIT"]\n');
  writeFileSync(join(root, "bundle.json"), JSON.stringify([p]));
  writeFileSync(join(root, "manual.json"), "[]");
  const args = ["--about", join(root, "about.toml"), "--bundle", join(root, "bundle.json"), "--manual", join(root, "manual.json")];
  const missing = spawnSync(process.execPath, [CHECK, ...args, "--require", "svelte"], { encoding: "utf8" });
  assert.equal(missing.status, 2, missing.stderr);
  assert.match(missing.stderr, /svelte/);
  const present = spawnSync(process.execPath, [CHECK, ...args, "--require", "fake-ok-lib"], { encoding: "utf8" });
  assert.equal(present.status, 0, present.stderr);
});

// Review round 1, finding 3: the project's own crates are not third-party. They stay in the
// cargo-about check but are not listed in the committed notices. The names come from the
// workspace manifests, so a new member is covered without editing this test.
test("the committed notices do not list the project's own workspace crates", () => {
  const root = fileURLToPath(new URL("../../", import.meta.url));
  // T-063: the members parse lives in ./workspace.mjs (it throws on a glob or nameless member).
  const names = workspaceMemberNames(root);
  assert.ok(names.length >= 2, `workspace members found: ${names.join(", ")}`);
  const text = readFileSync(join(root, "THIRD-PARTY-NOTICES.txt"), "utf8");
  const listed = names.flatMap((name) => {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    // "<name> <version>" as a whole crate name: not part of a longer name such as voicen-core.
    return text.split("\n").filter((line) => new RegExp(`(^|[^\\w-])${escaped} \\d+\\.\\d+\\.\\d+`).test(line));
  });
  assert.deepEqual(listed, [], `workspace crates (${names.join(", ")}) are listed in THIRD-PARTY-NOTICES.txt`);
});

// Review round 1, finding 1 (added by the developer): what the check fails on cannot be
// listed either, so `make licenses` alone never writes notices that miss bundled code.
test("notices.mjs exits 2, naming it, when the bundle list holds an unattributed module", (t) => {
  const root = tmp(t);
  const p = fakePackage(root, { name: "fake-ok-lib", version: "1.0.0", license: "MIT", files: { LICENSE: "x" } });
  writeFileSync(join(root, "bundle.json"), JSON.stringify([p, { unattributed: "\0virtual:example-icons/star.js", kind: "module" }]));
  writeFileSync(join(root, "manual.json"), "[]");
  writeFileSync(join(root, "rust.txt"), "Rust part\n");
  const out = join(root, "NOTICES.txt");
  const NOTICES = fileURLToPath(new URL("./notices.mjs", import.meta.url));
  const r = spawnSync(process.execPath, [NOTICES, "--rust", join(root, "rust.txt"), "--bundle", join(root, "bundle.json"), "--manual", join(root, "manual.json"), "--out", out], { encoding: "utf8" });
  assert.equal(r.status, 2, r.stderr);
  assert.ok(r.stderr.includes("virtual:example-icons/star.js"), r.stderr);
});

// T-063 (a): about.hbs leaves every Cargo.lock package without `source` out of the notices
// (they are taken to be our own workspace crates). So every sourceless package must be a
// workspace member, or a vendored path crate or a [patch] path would ship unlisted. The helper
// lives in ./workspace.mjs; it is imported inside each test so a missing module fails these
// tests only, not the whole file.
const WORKSPACE = new URL("./workspace.mjs", import.meta.url);

// A Cargo.lock v4 excerpt: the two members, a registry crate, a git crate, and the planted
// path crates. Names and URLs are fake.
const LOCK_HEAD = `# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "fake-registry-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"

[[package]]
name = "fake-git-crate"
version = "0.4.0"
source = "git+https://example.com/fake-git-crate.git#0123456789abcdef0123456789abcdef01234567"

[[package]]
name = "voicen"
version = "0.1.0"
dependencies = [
 "fake-registry-crate",
 "voicen-core",
]

[[package]]
name = "voicen-core"
version = "0.1.0"
dependencies = [
 "fake-git-crate",
]
`;

test("a Cargo.lock package without source that is not a workspace member is named", async () => {
  const { pathCratesOutsideWorkspace } = await import(WORKSPACE);
  const lock = `${LOCK_HEAD}
[[package]]
name = "vendored-zlib"
version = "1.3.1"
dependencies = [
 "fake-registry-crate",
]
`;
  assert.deepEqual(pathCratesOutsideWorkspace(lock, ["voicen", "voicen-core"]), ["vendored-zlib"]);
});

test("every sourceless non-member is named: a [patch] path crate and a member-prefixed name too", async () => {
  const { pathCratesOutsideWorkspace } = await import(WORKSPACE);
  // A [patch.crates-io] entry with a path is written to Cargo.lock without `source`, like a
  // path dependency; a name that only starts with a member's name is not that member.
  const lock = `${LOCK_HEAD}
[[package]]
name = "fake-registry-crate"
version = "1.2.4"

[[package]]
name = "voicen-core-vendored"
version = "0.1.0"
`;
  assert.deepEqual(
    [...pathCratesOutsideWorkspace(lock, ["voicen", "voicen-core"])].sort(),
    ["fake-registry-crate", "voicen-core-vendored"],
  );
});

test("a Cargo.lock whose only sourceless packages are the workspace members names nothing", async () => {
  const { pathCratesOutsideWorkspace } = await import(WORKSPACE);
  assert.deepEqual(pathCratesOutsideWorkspace(LOCK_HEAD, ["voicen", "voicen-core"]), []);
  // Without the members' names the same lock names both: the members list decides, not a
  // built-in exception.
  assert.deepEqual([...pathCratesOutsideWorkspace(LOCK_HEAD, [])].sort(), ["voicen", "voicen-core"]);
});

test("the repo's Cargo.lock has no sourceless package outside the workspace members", async () => {
  const { pathCratesOutsideWorkspace, workspaceMemberNames } = await import(WORKSPACE);
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const names = workspaceMemberNames(root);
  assert.deepEqual([...names].sort(), ["voicen", "voicen-core"]);
  assert.deepEqual(pathCratesOutsideWorkspace(readFileSync(join(root, "Cargo.lock"), "utf8"), names), []);
});

test("workspaceMemberNames refuses a glob member instead of guessing", async (t) => {
  const { workspaceMemberNames } = await import(WORKSPACE);
  const root = tmp(t);
  writeFileSync(join(root, "Cargo.toml"), '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n');
  mkdirSync(join(root, "crates", "fake-member"), { recursive: true });
  writeFileSync(join(root, "crates", "fake-member", "Cargo.toml"), '[package]\nname = "fake-member"\nversion = "0.1.0"\n');
  assert.throws(() => workspaceMemberNames(root), /crates\/\*/);
});
