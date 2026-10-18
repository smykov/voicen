// Reader of the one accepted-license list: the `accepted` array at the head of about.toml
// (T-027, T-094).
//
// Not a TOML reader: it reads an owned line shape (decides_on: literal). about.toml must
// begin, from byte 0, with
//
//   # ...                      zero or more header comment lines
//   accepted = [
//       # ...                  comment lines inside the list, and
//       "<id>",                one id per line, <id> = [A-Za-z0-9.+-]+ with an optional
//                              ` WITH [A-Za-z0-9.+-]+`
//   ]
//
// CRLF line ends are read as LF; a lone CR, a byte-order mark or any other line in the head
// is refused. Such a head holds no quote, backslash or multi-line string, so it has exactly
// one TOML meaning, and nothing before it can change that meaning; TOML forbids redefining
// `accepted` later (cargo-about 0.9.2 refuses a duplicate key). So the list read here is the
// list cargo-about reads, or about.toml is refused (P-020).

const ID_LINE = /^ {4}"([A-Za-z0-9.+-]+(?: WITH [A-Za-z0-9.+-]+)?)",$/;

/** A comment line of the given indent: the indent, then `#` alone or `# ` and text. */
function isComment(/** @type {string} */ line, /** @type {string} */ indent) {
  return line === `${indent}#` || line.startsWith(`${indent}# `);
}

/** @param {string} why */
function refuse(why) {
  return new Error(`about.toml: the \`accepted\` head is not in the owned shape (scripts/licenses/accepted.mjs): ${why}`);
}

/**
 * The ids of the `accepted` head of about.toml, in file order.
 * Throws (message names about.toml and `accepted`) on any file that does not begin with the
 * owned head shape.
 * @param {string} tomlText
 * @returns {string[]}
 */
export function readAcceptedList(tomlText) {
  if (tomlText.startsWith("﻿")) throw refuse("a byte-order mark before the head");
  const text = tomlText.replace(/\r\n/g, "\n");
  if (text.includes("\r")) throw refuse("a lone CR");
  const lines = text.split("\n");
  let n = 0;
  while (n < lines.length && isComment(lines[n], "")) n++;
  if (n >= lines.length || lines[n] !== "accepted = [") {
    throw refuse(`expected the line \`accepted = [\` after the header comments, found ${JSON.stringify(lines[n] ?? "")}`);
  }
  /** @type {string[]} */
  const ids = [];
  for (n++; n < lines.length; n++) {
    const line = lines[n];
    if (line === "]") return ids;
    if (isComment(line, "    ")) continue;
    const m = ID_LINE.exec(line);
    if (!m) throw refuse(`line ${n + 1} is neither \`    "<id>",\`, \`    # ...\` nor \`]\`: ${JSON.stringify(line)}`);
    ids.push(m[1]);
  }
  throw refuse("the list has no closing `]` line");
}
