// T-027 red tests: the accepted list is read from about.toml, the one list (decision #24, P-010).
// T-094 red tests: the reader accepts only an owned head shape of about.toml (from byte 0:
// `# ...` lines, `accepted = [`, lines `    # ...` or `    "<id>",`, `]`) and refuses every
// other file, so it cannot read a list that cargo-about does not (P-020, guard-model).
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

/** Lines joined with LF, ending with a newline. */
const file = (/** @type {string[]} */ lines) => `${lines.join("\n")}\n`;

/** A valid head: header comment, `accepted = [`, the ids, `]`. */
const HEAD = ["# header comment", "accepted = [", '    "MIT",', '    "ISC",', "]"];

/** Every refusal names about.toml (the reader's own error, not a TypeError). */
const REFUSED = /about\.toml/;

test("the committed about.toml holds exactly the list of decisions #9 + #24", () => {
  const list = readAcceptedList(readFileSync(`${ROOT}about.toml`, "utf8"));
  assert.deepEqual(list, DECISION_24, "file order, exactly the decided ids");
  assert.equal(new Set(list).size, list.length, "no duplicates");
});

test("reads the owned head shape: comments, ids with WITH, in file order", () => {
  const toml = file([
    "# header comment",
    "# second header line",
    "accepted = [",
    "    # decision #9",
    '    "MIT",',
    '    "Apache-2.0 WITH LLVM-exception",',
    '    # "GPL-3.0-only" was here once',
    '    "BSD-3-Clause",',
    '    "CC0-1.0",',
    '    "0BSD",',
    "]",
    "",
    'targets = ["x86_64-pc-windows-msvc"]',
  ]);
  assert.deepEqual(readAcceptedList(toml), [
    "MIT", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause", "CC0-1.0", "0BSD",
  ]);
});

test("the head may start directly with accepted, and CRLF line ends are read like LF", () => {
  assert.deepEqual(readAcceptedList(file(HEAD.slice(1))), ["MIT", "ISC"]);
  assert.deepEqual(readAcceptedList(file(HEAD).replace(/\n/g, "\r\n")), ["MIT", "ISC"]);
});

test("only the head list is read: a later accepted, top-level or per-crate, does not change it", () => {
  // cargo-about refuses a second top-level `accepted` (duplicate key, probe p40); a per-crate
  // table's `accepted` is a crate exception, never the npm list.
  const later = file([...HEAD, 'accepted = ["GPL-3.0-only"]']);
  assert.deepEqual(readAcceptedList(later), ["MIT", "ISC"]);
  const perCrate = file([...HEAD, "ignore-build-dependencies = true", "", "[some-crate]", 'accepted = ["GPL-3.0-only"]']);
  assert.deepEqual(readAcceptedList(perCrate), ["MIT", "ISC"]);
  // Whatever follows the head (even a multi-line string) is not read.
  const tail = file([...HEAD, 'targets = ["""x', 'accepted = ["GPL-3.0-only"]', '"""]']);
  assert.deepEqual(readAcceptedList(tail), ["MIT", "ISC"]);
});

// --- divergence probes (T-094 ## Investigation): the old reader returned ["GPL-3.0-only"] ---

const PROBES = {
  // cargo-about 0.9.2 reads [MIT] and exits 0: a silent widening of the npm list.
  "q01 targets, basic multi-line string": ['targets = ["""x86_64-pc-windows-msvc" ]', 'accepted = ["GPL-3.0-only"]', '"""]', 'accepted = ["MIT"]'],
  "q02 targets, literal multi-line string": ["targets = ['''x86_64-pc-windows-msvc' ]", 'accepted = ["GPL-3.0-only"]', "''']", 'accepted = ["MIT"]'],
  "q03 workarounds, basic multi-line string": ['workarounds = ["""ring" ]', 'accepted = ["GPL-3.0-only"]', '"""]', 'accepted = ["MIT"]'],
  // cargo-about refuses these, the reader must too.
  "p10 note, basic multi-line string": ['note = """', 'accepted = ["GPL-3.0-only"]', '"""', 'accepted = ["MIT"]'],
  "p12 note, literal multi-line string": ["note = '''", 'accepted = ["GPL-3.0-only"]', "'''", 'accepted = ["MIT"]'],
};

for (const [name, lines] of Object.entries(PROBES)) {
  test(`divergence probe ${name} is refused`, () => {
    assert.throws(() => readAcceptedList(file(lines)), REFUSED);
  });
}

// --- refusals: anything outside the owned head shape ---

const REFUSALS = {
  "a single-line array": ['accepted = ["MIT"]'],
  "a single-line array with spaces": ['accepted = [ "MIT", "ISC" ]'],
  "a literal-string id": ["accepted = [", "    'MIT',", "]"],
  "an inline comment after an id": ["accepted = [", '    "MIT", # inline', "]"],
  "a key before accepted": ['targets = ["x86_64-pc-windows-msvc"]', ...HEAD.slice(1)],
  "a key between the header comments and accepted": ["# header", "ignore-dev-dependencies = true", ...HEAD.slice(1)],
  "a blank line before accepted": ["# header", "", ...HEAD.slice(1)],
  "a blank first line": ["", ...HEAD],
  "an id holding a quote": ["accepted = [", '    "MI\\"T",', "]"],
  "an id holding a backslash": ["accepted = [", '    "MI\\\\T",', "]"],
  "an id holding a space without WITH": ["accepted = [", '    "MIT OR GPL-3.0-only",', "]"],
  "an id with a lowercase with": ["accepted = [", '    "Apache-2.0 with LLVM-exception",', "]"],
  "an id that is empty": ["accepted = [", '    "",', "]"],
  "an id without a trailing comma": ["accepted = [", '    "MIT"', "]"],
  "two ids on one line": ["accepted = [", '    "MIT", "ISC",', "]"],
  "an id indented with two spaces": ["accepted = [", '  "MIT",', "]"],
  "an id indented with five spaces": ["accepted = [", '     "MIT",', "]"],
  "an id indented with a tab": ["accepted = [", '\t"MIT",', "]"],
  "text before an indented id": ["accepted = [", 'x    "GPL-3.0-only",', "]"],
  "a blank line inside the list": ["accepted = [", '    "MIT",', "", '    "ISC",', "]"],
  "a multi-line string inside the list": ["accepted = [", '    """MIT""",', "]"],
  "accepted with extra spacing": ["accepted  = [", '    "MIT",', "]"],
  "a quoted accepted key": ['"accepted" = [', '    "MIT",', "]"],
  "trailing text after the closing bracket": ["accepted = [", '    "MIT",', "] # end"],
  "an indented closing bracket": ["accepted = [", '    "MIT",', "    ]"],
  "an unterminated list": ["accepted = [", '    "MIT",'],
  // T-094 round 2: every exact-line comparison of the head refuses a prefix, trim or trimEnd
  // match. Under a prefix match of `accepted = [` the first file reads ["MIT"], TOML reads
  // ["GPL-3.0-only", "MIT"] (validation 1, M1).
  "an id after the opening bracket": ['accepted = [ "GPL-3.0-only",', '    "MIT",', "]"],
  "a comment after the opening bracket": ["accepted = [ # c", '    "MIT",', "]"],
  "trailing whitespace after the opening bracket": ["accepted = [ ", '    "MIT",', "]"],
  "leading whitespace before accepted": ["  accepted = [", '    "MIT",', "]"],
  "trailing whitespace after the closing bracket": ["accepted = [", '    "MIT",', "] "],
  "an indented header comment": ["  # header", ...HEAD.slice(1)],
  "a header comment without a space after #": ["#header", ...HEAD.slice(1)],
  "a list comment without a space after #": ["accepted = [", "    #comment", '    "MIT",', "]"],
  "a list comment indented with two spaces": ["accepted = [", "  # comment", '    "MIT",', "]"],
  "a list comment indented with a tab": ["accepted = [", "\t# comment", '    "MIT",', "]"],
};

for (const [name, lines] of Object.entries(REFUSALS)) {
  test(`refused: ${name}`, () => {
    assert.throws(() => readAcceptedList(file(lines)), REFUSED);
  });
}

test("refused: a file that ends inside the head, without a closing bracket", () => {
  assert.throws(() => readAcceptedList('accepted = [\n    "MIT",'), REFUSED);
  assert.throws(() => readAcceptedList('accepted = [\n    "MIT",\n    # more to come'), REFUSED);
  assert.throws(() => readAcceptedList("# header\naccepted = ["), REFUSED);
});

test("refused: a byte-order mark before the head", () => {
  assert.throws(() => readAcceptedList(`﻿${file(HEAD)}`), REFUSED);
});

test("refused: a lone CR anywhere in the head", () => {
  assert.throws(() => readAcceptedList(file(HEAD).replace('"MIT",\n', '"MIT",\r')), REFUSED);
  assert.throws(() => readAcceptedList(file(HEAD).replace("# header comment\n", "# header\rcomment\n")), REFUSED);
});

test("a file without a top-level accepted fails loudly", () => {
  assert.throws(() => readAcceptedList('targets = ["x86_64-pc-windows-msvc"]\n'), /accepted/);
  assert.throws(() => readAcceptedList('[some-crate]\naccepted = ["MIT"]\n'), /accepted/);
  assert.throws(() => readAcceptedList(""), /accepted/);
});

test("an accepted that is not an array, or is unterminated, fails loudly", () => {
  assert.throws(() => readAcceptedList('accepted = "MIT"\n'), /accepted/);
  assert.throws(() => readAcceptedList('accepted = [\n  "MIT",\n'), /accepted/);
  assert.throws(() => readAcceptedList('accepted = [ MIT ]\n'), /accepted/);
});
