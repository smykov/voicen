// T-181 red test (T-094 r1#2): the byte-order mark is written as the escape \uFEFF, never as an
// invisible raw U+FEFF (EF BB BF) in a source file of the licenses scripts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const DIR = fileURLToPath(new URL("./", import.meta.url));

for (const name of ["accepted.mjs", "accepted.test.mjs", "check.test.mjs"]) {
  test(`${name} holds no raw U+FEFF byte sequence`, () => {
    const bytes = readFileSync(DIR + name);
    assert.equal(bytes.indexOf(Buffer.from([0xef, 0xbb, 0xbf])), -1, `${name} has a raw byte-order mark; write \\uFEFF`);
  });
}

test("accepted.mjs still refuses a byte-order mark (written as the escape)", () => {
  assert.match(readFileSync(DIR + "accepted.mjs", "utf8"), /\\uFEFF/);
});
