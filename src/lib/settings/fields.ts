// DOM ids of a settings field's control and of its error text, derived from the
// FieldId (contracts/ipc.md), so a label, a control and its error always match.

/** The id of the form control of `field` (`engine.api.base_url` -> `field-engine-api-base_url`). */
export function controlId(field: string): string {
  return `field-${field.replaceAll(".", "-")}`;
}

/** The id of the element that shows the error of `field`. */
export function errorId(field: string): string {
  return `${controlId(field)}-error`;
}
