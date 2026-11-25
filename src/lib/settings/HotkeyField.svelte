<script lang="ts">
  // A hotkey input: shows the hotkey text and captures a new one from a key press
  // (hotkey.ts, no allowlist); core decides whether it is valid and available. One
  // component for every hotkey FieldId (T-155): `field` names the control, `label` its
  // catalog label and `value` the bound draft value (recording.hotkey on Recording,
  // calls.hotkey on Calls).
  import { t, type MessageId } from "$lib/i18n";
  import { controlId, describedBy, hintId } from "./fields";
  import FieldMessage from "./FieldMessage.svelte";
  import { hotkeyText, isFocusMove } from "./hotkey";

  let {
    value = $bindable(),
    field,
    label,
    errors,
    warnings,
  }: {
    value: string;
    field: string;
    label: MessageId;
    errors: Record<string, MessageId>;
    warnings: Record<string, MessageId>;
  } = $props();

  function onKeydown(event: KeyboardEvent) {
    if (isFocusMove(event)) return;
    event.preventDefault();
    const text = hotkeyText(event);
    if (text !== null) value = text;
  }
</script>

<div class="field">
  <label for={controlId(field)}>{t(label)}</label>
  <input
    id={controlId(field)}
    type="text"
    readonly
    data-field={field}
    {value}
    onkeydown={onKeydown}
    aria-invalid={errors[field] ? "true" : undefined}
    aria-describedby={describedBy(field, errors, warnings, true)}
  />
  <p class="hint" id={hintId(field)}>{t("settings.hotkey.hint")}</p>
  <FieldMessage {errors} {warnings} {field} />
</div>
