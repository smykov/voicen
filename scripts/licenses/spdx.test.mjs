// T-027 red tests: SPDX expressions evaluated against the accepted list (decisions #9, #24).
import { test } from "node:test";
import assert from "node:assert/strict";
import { satisfies } from "./spdx.mjs";

// The list of decision #9 plus the five of #24, as about.toml holds it.
const ACCEPTED = [
  "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0",
  "MPL-2.0", "Unlicense", "CC0-1.0", "Apache-2.0 WITH LLVM-exception", "0BSD", "BSL-1.0",
  "Unicode-DFS-2016", "CDLA-Permissive-2.0",
];

test("a single accepted license passes", () => {
  assert.equal(satisfies("MIT", ACCEPTED), true);
  assert.equal(satisfies("CDLA-Permissive-2.0", ACCEPTED), true);
});

test("GPL-3.0 (any form) is not accepted", () => {
  for (const id of ["GPL-3.0", "GPL-3.0-only", "GPL-3.0-or-later", "GPL-2.0+", "AGPL-3.0-only", "LGPL-2.1"]) {
    assert.equal(satisfies(id, ACCEPTED), false, id);
  }
});

test("MIT OR GPL-3.0 passes: one accepted alternative is enough", () => {
  assert.equal(satisfies("MIT OR GPL-3.0", ACCEPTED), true);
  assert.equal(satisfies("GPL-3.0 OR MIT", ACCEPTED), true);
});

test("MIT AND GPL-3.0 fails: every conjunct must be accepted", () => {
  assert.equal(satisfies("MIT AND GPL-3.0", ACCEPTED), false);
  assert.equal(satisfies("GPL-3.0 AND MIT", ACCEPTED), false);
  assert.equal(satisfies("MIT AND Apache-2.0", ACCEPTED), true);
});

test("parentheses group: (MIT OR Apache-2.0) AND Unicode-3.0 passes, (MIT OR GPL-3.0) AND GPL-2.0 fails", () => {
  assert.equal(satisfies("(MIT OR Apache-2.0) AND Unicode-3.0", ACCEPTED), true);
  assert.equal(satisfies("(MIT OR GPL-3.0) AND GPL-2.0", ACCEPTED), false);
  assert.equal(satisfies("(MIT OR Apache-2.0)", ACCEPTED), true);
});

test("AND binds tighter than OR", () => {
  // MIT OR (GPL-3.0 AND GPL-2.0) → true; a left-to-right reading gives false.
  assert.equal(satisfies("MIT OR GPL-3.0 AND GPL-2.0", ACCEPTED), true);
  // (GPL-3.0 AND MIT) OR GPL-2.0 → false; a left-to-right reading of OR-first gives true.
  assert.equal(satisfies("GPL-3.0 AND MIT OR GPL-2.0", ACCEPTED), false);
});

test("WITH: an accepted exception expression passes as a whole, another one fails", () => {
  assert.equal(satisfies("Apache-2.0 WITH LLVM-exception", ACCEPTED), true);
  assert.equal(satisfies("GPL-2.0-only WITH Classpath-exception-2.0", ACCEPTED), false);
});

test("ids are matched exactly, never by prefix or substring", () => {
  for (const id of ["MIT-0", "BSD-3-Clause-Clear", "Apache-1.1", "MPL-2.0-no-copyleft-exception", "LicenseRef-MIT"]) {
    assert.equal(satisfies(id, ACCEPTED), false, id);
  }
});

test("missing, empty, UNLICENSED and UNKNOWN are not accepted", () => {
  for (const expr of [undefined, null, "", "   ", "UNLICENSED", "UNKNOWN"]) {
    assert.equal(satisfies(expr, ACCEPTED), false, String(expr));
  }
});

test("an unparseable expression is not accepted and does not throw", () => {
  for (const expr of ["MIT OR", "(MIT", "MIT)", "AND MIT", "Apache 2.0", "SEE LICENSE IN LICENSE.md", "MIT OR OR ISC"]) {
    assert.equal(satisfies(expr, ACCEPTED), false, expr);
  }
});

test("the result depends on the given list, not a built-in one", () => {
  assert.equal(satisfies("MIT", ["Apache-2.0"]), false);
  assert.equal(satisfies("GPL-3.0-only", ["GPL-3.0-only"]), true);
  assert.equal(satisfies("MIT", []), false);
});
