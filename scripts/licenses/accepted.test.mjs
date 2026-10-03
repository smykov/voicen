// T-027 red tests: the accepted list is read from about.toml, the one list (decision #24, P-010).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { readAcceptedList } from "./accepted.mjs";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));

// Decision #9 plus the five licenses #24 added. about.toml must hold exactly these.
const DECISION_24 = [
  "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0",
  "MPL-2.0", "Unlicense", "CC0-1.0", "Apache-2.0 WITH LLVM-exception", "0BSD", "BSL-1.0",
  "Unicode-DFS-2016", "CDLA-Permissive-2.0",
];

test("the committed about.toml holds exactly the list of decisions #9 + #24", () => {
  const list = readAcceptedList(readFileSync(`${ROOT}about.toml`, "utf8"));
  assert.deepEqual([...list].sort(), [...DECISION_24].sort());
  assert.equal(new Set(list).size, list.length, "no duplicates");
});

test("reads a multi-line array with comments, trailing comma and both string kinds", () => {
  const toml = [
    "# header comment",
    "accepted = [",
    '    "MIT", # inline comment',
    "    'Apache-2.0 WITH LLVM-exception',",
    "    # \"GPL-3.0-only\" was here once",
    '    "ISC",',
    "]",
    'targets = ["x86_64-pc-windows-msvc"]',
  ].join("\n");
  assert.deepEqual(readAcceptedList(toml), ["MIT", "Apache-2.0 WITH LLVM-exception", "ISC"]);
});

test("only the top-level accepted is read, not a per-crate table's", () => {
  const toml = [
    'accepted = ["MIT"]',
    "ignore-build-dependencies = true",
    "",
    "[some-crate]",
    'accepted = ["GPL-3.0-only"]',
  ].join("\n");
  assert.deepEqual(readAcceptedList(toml), ["MIT"]);
});

test("a file without a top-level accepted fails loudly", () => {
  assert.throws(() => readAcceptedList('targets = ["x86_64-pc-windows-msvc"]\n'), /accepted/);
  assert.throws(() => readAcceptedList('[some-crate]\naccepted = ["MIT"]\n'), /accepted/);
});

test("an accepted that is not an array, or is unterminated, fails loudly", () => {
  assert.throws(() => readAcceptedList('accepted = "MIT"\n'), /accepted/);
  assert.throws(() => readAcceptedList('accepted = [\n  "MIT",\n'), /accepted/);
  assert.throws(() => readAcceptedList('accepted = [ MIT ]\n'), /accepted/);
});
