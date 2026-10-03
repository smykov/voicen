// SPDX license-expression evaluation against the accepted list (T-027).
//
// Grammar (SPDX 2.3 annex D, operators case-sensitive):
//   or-expr  := and-expr ("OR" and-expr)*
//   and-expr := term ("AND" term)*
//   term     := "(" or-expr ")" | license-id ["WITH" exception-id]
// A term is satisfied when its text ("id" or "id WITH exception") is one of the accepted
// entries, compared as a whole: no prefix, family or "-or-later" matching.

/**
 * Never satisfied, whatever the list says: they mean "no license given". The one set of
 * such values; check.mjs reports them as reason "missing".
 */
export const NO_LICENSE = new Set(["UNLICENSED", "UNKNOWN", "NONE", "NOASSERTION"]);
const OPERATORS = new Set(["AND", "OR", "WITH"]);
const ID = /^[A-Za-z0-9][A-Za-z0-9.-]*\+?$|^(DocumentRef-[A-Za-z0-9.-]+:)?LicenseRef-[A-Za-z0-9.-]+$/;

/**
 * @param {string} text
 * @returns {string[]}
 */
function tokenize(text) {
  return text.replace(/[()]/g, " $& ").trim().split(/\s+/).filter(Boolean);
}

/** One accepted entry in the canonical form terms are compared in ("A WITH B", single spaces). */
function canonical(/** @type {string} */ entry) {
  return tokenize(entry).join(" ");
}

class ParseError extends Error {}

/**
 * Parses and evaluates `tokens` in one pass.
 * @param {string[]} tokens
 * @param {Set<string>} accepted canonical entries
 * @returns {boolean}
 */
function evaluate(tokens, accepted) {
  let pos = 0;
  const peek = () => tokens[pos];
  const take = () => {
    const t = tokens[pos++];
    if (t === undefined) throw new ParseError("unexpected end");
    return t;
  };
  const licenseId = () => {
    const t = take();
    if (t === "(" || t === ")" || OPERATORS.has(t) || !ID.test(t)) throw new ParseError(`unexpected ${t}`);
    return t;
  };

  /** @returns {boolean} */
  const term = () => {
    if (peek() === "(") {
      take();
      const value = orExpr();
      if (take() !== ")") throw new ParseError("missing )");
      return value;
    }
    let text = licenseId();
    if (peek() === "WITH") {
      take();
      text = `${text} WITH ${licenseId()}`;
    }
    return !NO_LICENSE.has(text) && accepted.has(text);
  };
  /** @returns {boolean} */
  const andExpr = () => {
    let value = term();
    while (peek() === "AND") {
      take();
      const right = term(); // parse every operand: a malformed tail must not be skipped
      value = value && right;
    }
    return value;
  };
  /** @returns {boolean} */
  const orExpr = () => {
    let value = andExpr();
    while (peek() === "OR") {
      take();
      const right = andExpr();
      value = value || right;
    }
    return value;
  };

  const value = orExpr();
  if (pos !== tokens.length) throw new ParseError(`unexpected ${tokens[pos]}`);
  return value;
}

/**
 * True when the SPDX expression `expression` is satisfied by the licenses in `accepted`
 * (AND binds tighter than OR, parentheses group, `X WITH Y` is one term matched as a whole).
 * A missing, empty or unparseable expression is never satisfied (returns false, never throws).
 * @param {string | null | undefined} expression
 * @param {string[]} accepted
 * @returns {boolean}
 */
export function satisfies(expression, accepted) {
  if (typeof expression !== "string") return false;
  const tokens = tokenize(expression);
  if (tokens.length === 0) return false;
  try {
    return evaluate(tokens, new Set(accepted.map(canonical)));
  } catch (e) {
    if (e instanceof ParseError) return false;
    throw e;
  }
}
