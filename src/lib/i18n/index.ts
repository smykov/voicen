// English/Russian message catalog for the UI (FR-15, T-005).
//
// The catalog lives at the repo root (`i18n/`, alias `$i18n`) and is shared with
// `voicen_core::i18n`; `text()` here and Rust `Catalog::text` follow one rule,
// pinned by `i18n/conformance.json`.
import en from "$i18n/en.json";
import ru from "$i18n/ru.json";
import { currentLanguage, setCurrentLanguage } from "./language.svelte";

/** UI language; the same tags as `voicen_core::i18n::UiLanguage`. */
export type UiLanguage = "en" | "ru";

/** An id present in the real catalog; an unknown literal id is a type error. */
export type MessageId = keyof typeof en;

/** One language's catalog: a flat id -> text map. */
export type Messages = Readonly<Record<string, string>>;

/** Both catalogs, injected into the pure functions below. */
export interface Catalog {
  en: Messages;
  ru: Messages;
}

/** Named placeholder arguments (name -> value). */
export type MessageArgs = Readonly<Record<string, string>>;

/** One catalog invariant violation, naming the offending id. */
export interface CatalogProblem {
  id: string;
  kind: "missing" | "empty";
  lang: UiLanguage;
}

/** Every UI language, in display order; the one list of them in the UI. */
export const LANGUAGES: readonly UiLanguage[] = ["en", "ru"];

/** `{name}` with `name` matching `[a-z][a-z0-9_]*`; no escaping. */
const PLACEHOLDER = /\{([a-z][a-z0-9_]*)\}/g;

const realCatalog: Catalog = { en, ru };

/** The text of `id` in `messages` if it is a non-empty string, else undefined. */
function nonEmpty(messages: Messages, id: string): string | undefined {
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

/** Ids missing from either catalog or with an empty text there. */
export function catalogProblems(catalog: Catalog): CatalogProblem[] {
  const ids = new Set([...Object.keys(catalog.en), ...Object.keys(catalog.ru)]);
  const problems: CatalogProblem[] = [];
  for (const id of ids) {
    for (const lang of LANGUAGES) {
      if (!Object.hasOwn(catalog[lang], id)) {
        problems.push({ id, kind: "missing", lang });
      } else if (nonEmpty(catalog[lang], id) === undefined) {
        problems.push({ id, kind: "empty", lang });
      }
    }
  }
  return problems;
}

/** Set the language `t()` renders in; the UI starts with `en`. */
export function setLanguage(lang: UiLanguage): void {
  setCurrentLanguage(lang);
}

/**
 * Render a message from the real catalog in the current language: the only renderer
 * of a catalog id in the UI. An id that arrives over IPC is typed `MessageId` at the
 * wire boundary (`FormError.message`) or built by `errorMessageId` (`error.<code>`);
 * there is no `string`-typed renderer (docs/decisions/i18n.md).
 * Called in a Svelte template or effect, it re-runs when the language changes.
 */
export function t(id: MessageId, args: MessageArgs = {}): string {
  return text(realCatalog, currentLanguage(), id, args);
}
