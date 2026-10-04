<script lang="ts">
  // Output tab: whether the text is pasted into the focused field.
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, describedBy } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();
  const autoPaste = "output.auto_paste";
</script>

<div class="field check">
  <input
    id={controlId(autoPaste)}
    type="checkbox"
    data-field={autoPaste}
    bind:checked={draft.settings.auto_paste}
    aria-invalid={draft.errors[autoPaste] ? "true" : undefined}
    aria-describedby={describedBy(autoPaste, draft.errors, warnings)}
  />
  <label for={controlId(autoPaste)}>{t("settings.field.auto_paste")}</label>
  <FieldMessage errors={draft.errors} {warnings} field={autoPaste} />
</div>
