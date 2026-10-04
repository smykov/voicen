// The lookup + render rule of the UI catalog, internal to `src/lib/i18n` (T-039).
//
// `text` takes any string id, so it is not exported from `index.ts`: the module's only
// public renderer is `t(id: MessageId)` (docs/decisions/i18n.md). Callers of `text`:
// `t` in `index.ts` and the conformance test (`i18n.test.ts`); `catalogProblems` uses
// `nonEmpty`.
import type { Catalog, MessageArgs, Messages, UiLanguage } from "./index";

/** `{name}` with `name` matching `[a-z][a-z0-9_]*`; no escaping. */
const PLACEHOLDER = /\{([a-z][a-z0-9_]*)\}/g;

/** The text of `id` in `messages` if it is a non-empty string, else undefined. */
export function nonEmpty(messages: Messages, id: string): string | undefined {
  if (!Object.hasOwn(messages, id)) return undefined;
  const value = messages[id];
  return typeof value === "string" && value !== "" ? value : undefined;
}

/**
 * The single lookup + render rule, the same as Rust `Catalog::text`
 * (pinned by `i18n/conformance.json`).
 *
 * Lookup: the `lang` text if non-empty, else the `en` text if non-empty, else `id`.
 * Render: one left-to-right pass over the template; each placeholder with an
 * argument is replaced by the value inserted literally (the replacer function's
 * return value is never read as a `$&`-style pattern) and never re-expanded; a
 * placeholder without an argument stays verbatim; extra arguments are ignored.
 */
export function text(catalog: Catalog, lang: UiLanguage, id: string, args: MessageArgs = {}): string {
  const template = nonEmpty(catalog[lang], id) ?? nonEmpty(catalog.en, id);
  if (template === undefined) return id;
  return template.replace(PLACEHOLDER, (placeholder: string, name: string) =>
    Object.hasOwn(args, name) ? args[name] : placeholder,
  );
}
