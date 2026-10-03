// English/Russian message catalog for the UI (FR-15, T-005).
//
// STUB written by the test writer: the public surface below exists only so the
// tests type-check. Every body throws; the implementation replaces them and
// removes the eslint-disable below (it exists only for the stub's unused params).
/* eslint-disable @typescript-eslint/no-unused-vars -- T-005 stub, removed by the implementation */
import en from "$i18n/en.json";

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

/**
 * The single lookup + render rule, the same as Rust `Catalog::text`
 * (pinned by `i18n/conformance.json`).
 */
export function text(_catalog: Catalog, _lang: UiLanguage, _id: string, _args: MessageArgs = {}): string {
  throw new Error("T-005: not implemented");
}

/** Ids missing from either catalog or with an empty text there. */
export function catalogProblems(_catalog: Catalog): CatalogProblem[] {
  throw new Error("T-005: not implemented");
}

/** Set the language `t()` renders in; the UI starts with `en`. */
export function setLanguage(_lang: UiLanguage): void {
  throw new Error("T-005: not implemented");
}

/** Render a message from the real catalog in the current language. */
export function t(_id: MessageId, _args: MessageArgs = {}): string {
  throw new Error("T-005: not implemented");
}
