<script lang="ts">
  // Output tab: whether the text is pasted into the focused field.
  import { t } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, errorId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";

  let { draft = $bindable() }: { draft: Draft } = $props();
  const autoPaste = "output.auto_paste";
</script>

<div class="field check">
  <input
    id={controlId(autoPaste)}
    type="checkbox"
    data-field={autoPaste}
    bind:checked={draft.settings.auto_paste}
    aria-invalid={draft.errors[autoPaste] ? "true" : undefined}
    aria-describedby={draft.errors[autoPaste] ? errorId(autoPaste) : undefined}
  />
  <label for={controlId(autoPaste)}>{t("settings.field.auto_paste")}</label>
  <FieldMessage errors={draft.errors} field={autoPaste} />
</div>
