<script lang="ts">
  // The hotkey input: shows the hotkey text and captures a new one from a key press
  // (hotkey.ts, no allowlist); core decides whether it is valid and available.
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "./draft";
  import { controlId, describedBy } from "./fields";
  import FieldMessage from "./FieldMessage.svelte";
  import { hotkeyText, isFocusMove } from "./hotkey";

  const field = "recording.hotkey";
  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();

  function onKeydown(event: KeyboardEvent) {
    if (isFocusMove(event)) return;
    event.preventDefault();
    const text = hotkeyText(event);
    if (text !== null) draft.settings.hotkey = text;
  }
</script>

<div class="field">
  <label for={controlId(field)}>{t("settings.field.hotkey")}</label>
  <input
    id={controlId(field)}
    type="text"
    readonly
    data-field={field}
    value={draft.settings.hotkey}
    onkeydown={onKeydown}
    aria-invalid={draft.errors[field] ? "true" : undefined}
    aria-describedby={describedBy(field, draft.errors, warnings)}
  />
  <p class="hint">{t("settings.hotkey.hint")}</p>
  <FieldMessage errors={draft.errors} {warnings} {field} />
</div>
