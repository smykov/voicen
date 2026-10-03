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
  const workspace = readFileSync(join(root, "Cargo.toml"), "utf8");
  const members = workspace.match(/^members\s*=\s*\[([^\]]*)\]/m);
  assert.ok(members, "Cargo.toml has a [workspace] members list");
  const names = [...members[1].matchAll(/"([^"]+)"/g)].map(([, dir]) => {
    const manifest = readFileSync(join(root, dir, "Cargo.toml"), "utf8");
    const pkg = manifest.slice(manifest.indexOf("[package]")).match(/^name\s*=\s*"([^"]+)"/m);
    assert.ok(manifest.includes("[package]") && pkg, `${dir}/Cargo.toml has a [package] name`);
    return pkg[1];
  });
  assert.ok(names.length >= 2, `workspace members found: ${names.join(", ")}`);
  const text = readFileSync(join(root, "THIRD-PARTY-NOTICES.txt"), "utf8");
  const listed = names.flatMap((name) => {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    // "<name> <version>" as a whole crate name: not part of a longer name such as voicen-core.
    return text.split("\n").filter((line) => new RegExp(`(^|[^\\w-])${escaped} \\d+\\.\\d+\\.\\d+`).test(line));
  });
  assert.deepEqual(listed, [], `workspace crates (${names.join(", ")}) are listed in THIRD-PARTY-NOTICES.txt`);
});
