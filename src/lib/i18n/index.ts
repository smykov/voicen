// English/Russian message catalog for the UI (FR-15, T-005).
//
// The catalog lives at the repo root (`i18n/`, alias `$i18n`) and is shared with
// `voicen_core::i18n`; `text()` (`./render.ts`) and Rust `Catalog::text` follow one
// rule, pinned by `i18n/conformance.json`. `text` takes any string id, so it is not
// exported from here: `t(id: MessageId)` is the module's only public renderer.
import en from "$i18n/en.json";
import ru from "$i18n/ru.json";
import { currentLanguage, setCurrentLanguage } from "./language.svelte";
import { nonEmpty, text } from "./render";

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

const realCatalog: Catalog = { en, ru };

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
 * wire boundary (`FormError.message`) or built by `errorMessageId` (`error.<code>`) or
 * `fieldLabelId` (`settings.field_label.<FieldId>`); there is no `string`-typed
 * renderer (docs/decisions/i18n.md).
 * Called in a Svelte template or effect, it re-runs when the language changes.
 */
export function t(id: MessageId, args: MessageArgs = {}): string {
  return text(realCatalog, currentLanguage(), id, args);
}
