// Reader of the one accepted-license list: the top-level `accepted` array of about.toml (T-027).
// Stub: the developer implements it; the tests in accepted.test.mjs define the contract.

/**
 * The top-level `accepted` array of about.toml, in file order.
 * Throws (message names `accepted`) when the key is missing, not an array, or cannot be parsed.
 * @param {string} tomlText
 * @returns {string[]}
 */
export function readAcceptedList(tomlText) {
  void tomlText;
  throw new Error("not implemented (T-027)");
}
