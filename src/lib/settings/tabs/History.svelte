<script lang="ts">
  // History tab: on/off and how many entries to keep. The range is core's rule
  // (history.size_range); the input only has to produce a whole number for the wire.
  import { t } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, errorId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";

  let { draft = $bindable() }: { draft: Draft } = $props();
  const enabled = "history.enabled";
  const size = "history.size";

  // The wire type is a whole number: an empty or non-integer entry is sent as 0,
  // which core refuses with history.size_range.
  function wholeNumber(value: number | null): number {
    return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : 0;
  }
</script>

<div class="field check">
  <input
    id={controlId(enabled)}
    type="checkbox"
    data-field={enabled}
    bind:checked={draft.settings.history.enabled}
    aria-invalid={draft.errors[enabled] ? "true" : undefined}
    aria-describedby={draft.errors[enabled] ? errorId(enabled) : undefined}
  />
  <label for={controlId(enabled)}>{t("settings.field.history_enabled")}</label>
  <FieldMessage errors={draft.errors} field={enabled} />
</div>

<div class="field">
  <label for={controlId(size)}>{t("settings.field.history_size")}</label>
  <input
    id={controlId(size)}
    type="number"
    step="1"
    data-field={size}
    bind:value={() => draft.settings.history.size, (value) => (draft.settings.history.size = wholeNumber(value))}
    aria-invalid={draft.errors[size] ? "true" : undefined}
    aria-describedby={draft.errors[size] ? errorId(size) : undefined}
  />
  <FieldMessage errors={draft.errors} field={size} />
</div>
