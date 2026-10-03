// Reader of the one accepted-license list: the top-level `accepted` array of about.toml (T-027).
//
// Not a TOML parser: it reads the top-level key/value lines before the first table header
// and understands only what about.toml uses for `accepted` (an array of basic or literal
// strings, comments, trailing comma). Anything else in that value fails loudly, so a
// format the reader does not understand can never shrink or widen the list silently.

/**
 * Reads one string starting at text[i] (a quote). Returns [value, next index].
 * @param {string} text
 * @param {number} i
 * @returns {[string, number]}
 */
function readString(text, i) {
  const quote = text[i];
  let value = "";
  for (let j = i + 1; j < text.length; j++) {
    const c = text[j];
    if (c === "\n") break;
    if (c === quote) return [value, j + 1];
    if (c === "\\" && quote === '"') {
      const next = text[j + 1];
      const escapes = { '"': '"', "\\": "\\", n: "\n", t: "\t" };
      if (next === undefined || !(next in escapes)) break;
      value += escapes[/** @type {keyof typeof escapes} */ (next)];
      j++;
      continue;
    }
    value += c;
  }
  throw new Error("about.toml: unterminated string (while reading `accepted`)");
}

/** Index after the comment or whitespace run at i (comments run to end of line). */
function skipBlank(/** @type {string} */ text, /** @type {number} */ i) {
  while (i < text.length) {
    if (/\s/.test(text[i])) i++;
    else if (text[i] === "#") {
      while (i < text.length && text[i] !== "\n") i++;
    } else break;
  }
  return i;
}

/**
 * Parses the array value starting at text[i] ("[").
 * @param {string} text
 * @param {number} i
 * @returns {string[]}
 */
function readStringArray(text, i) {
  if (text[i] !== "[") throw new Error("about.toml: `accepted` must be an array of strings");
  i++;
  /** @type {string[]} */
  const items = [];
  for (;;) {
    i = skipBlank(text, i);
    if (i >= text.length) throw new Error("about.toml: unterminated `accepted` array");
    if (text[i] === "]") return items;
    if (text[i] !== '"' && text[i] !== "'") {
      throw new Error(`about.toml: \`accepted\` holds a non-string value near ${JSON.stringify(text.slice(i, i + 20))}`);
    }
    const [value, next] = readString(text, i);
    items.push(value);
    i = skipBlank(text, next);
    if (text[i] === ",") i++;
    else if (text[i] !== "]") {
      throw new Error("about.toml: `accepted` entries must be separated by commas");
    }
  }
}

/**
 * Index just past the value that starts at text[i]: a bracketed array (strings and comments
 * respected) or the rest of the line.
 */
function skipValue(/** @type {string} */ text, /** @type {number} */ i) {
  if (text[i] !== "[" && text[i] !== "{") {
    while (i < text.length && text[i] !== "\n") i++;
    return i;
  }
  let depth = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === '"' || c === "'") {
      i = readString(text, i)[1];
      continue;
    }
    if (c === "#") {
      while (i < text.length && text[i] !== "\n") i++;
      continue;
    }
    if (c === "[" || c === "{") depth++;
    if (c === "]" || c === "}") depth--;
    i++;
    if (depth === 0) return i;
  }
  return i;
}

/**
 * The top-level `accepted` array of about.toml, in file order.
 * Throws (message names `accepted`) when the key is missing, not an array, or cannot be parsed.
 * @param {string} tomlText
 * @returns {string[]}
 */
export function readAcceptedList(tomlText) {
  const text = tomlText.replace(/\r\n/g, "\n");
  let i = 0;
  for (;;) {
    i = skipBlank(text, i);
    if (i >= text.length || text[i] === "[") break; // end of file or first table header
    const eq = text.indexOf("=", i);
    const eol = text.indexOf("\n", i);
    if (eq === -1 || (eol !== -1 && eol < eq)) {
      throw new Error(`about.toml: cannot read the line before \`accepted\`: ${JSON.stringify(text.slice(i, eol === -1 ? undefined : eol))}`);
    }
    const key = text.slice(i, eq).trim();
    const valueStart = skipBlank(text, eq + 1);
    if (key === "accepted") return readStringArray(text, valueStart);
    i = skipValue(text, valueStart);
  }
  throw new Error("about.toml: no top-level `accepted` list");
}
