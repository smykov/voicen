<script lang="ts">
  // History tab: on/off and how many entries to keep. The range is core's rule
  // (history.size_range); the input only has to produce a whole number for the wire.
  import { untrack } from "svelte";
  import { t } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, errorId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";

  let { draft = $bindable() }: { draft: Draft } = $props();
  const enabled = "history.enabled";
  const size = "history.size";

  // The wire type is an unsigned whole number: an entry that is not one (empty,
  // fractional, negative) is sent as 0, which core refuses with history.size_range.
  // The range itself is core's rule only.
  function wholeNumber(entry: string): number {
    const value = Number(entry);
    return entry.trim() !== "" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
  }

  // The input shows what the user typed, not the coerced number: a cleared field stays
  // empty (it is not rewritten to "0", so typing a digit gives that digit). The entry
  // follows the draft only when the draft's number changes to one the entry does not
  // mean (a view applied, a Saved).
  let entry = $state<string | null>(null);
  $effect.pre(() => {
    const value = draft.settings.history.size;
    untrack(() => {
      if (entry === null || wholeNumber(entry) !== value) entry = String(value);
    });
  });

  function onSizeInput(event: Event & { currentTarget: HTMLInputElement }) {
    entry = event.currentTarget.value;
    draft.settings.history.size = wholeNumber(entry);
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
    value={entry ?? ""}
    oninput={onSizeInput}
    aria-invalid={draft.errors[size] ? "true" : undefined}
    aria-describedby={draft.errors[size] ? errorId(size) : undefined}
  />
  <FieldMessage errors={draft.errors} field={size} />
</div>
