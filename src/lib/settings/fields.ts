// DOM ids of a settings field's control and of its error and warning texts, derived
// from the FieldId (contracts/ipc.md), so a label, a control and its messages always
// match.
import type { MessageId } from "../i18n";

/** The id of the form control of `field` (`engine.api.base_url` -> `field-engine-api-base_url`). */
export function controlId(field: string): string {
  return `field-${field.replaceAll(".", "-")}`;
}

/** The id of the element that shows the error of `field`. */
export function errorId(field: string): string {
  return `${controlId(field)}-error`;
}

/** The id of the element that shows the warning of `field` (T-015). */
export function warningId(field: string): string {
  return `${controlId(field)}-warning`;
}

/**
 * The control's `aria-describedby`: the ids of the messages `FieldMessage` renders for
 * `field` (its error, its warning, both, or none -> undefined). The one rule for every
 * control, so a rendered message is always in the control's description.
 */
export function describedBy(
  field: string,
  errors: Record<string, MessageId>,
  warnings: Record<string, MessageId>,
): string | undefined {
  const ids: string[] = [];
  if (errors[field]) ids.push(errorId(field));
  if (warnings[field]) ids.push(warningId(field));
  return ids.length > 0 ? ids.join(" ") : undefined;
}
