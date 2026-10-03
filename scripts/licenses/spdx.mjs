// SPDX license-expression evaluation against the accepted list (T-027).
// Stub: the developer implements it; the tests in spdx.test.mjs define the contract.

/**
 * True when the SPDX expression `expression` is satisfied by the licenses in `accepted`
 * (AND binds tighter than OR, parentheses group, `X WITH Y` is one term matched as a whole).
 * A missing, empty or unparseable expression is never satisfied (returns false, never throws).
 * @param {string | null | undefined} expression
 * @param {string[]} accepted
 * @returns {boolean}
 */
export function satisfies(expression, accepted) {
  void expression;
  void accepted;
  throw new Error("not implemented (T-027)");
}
