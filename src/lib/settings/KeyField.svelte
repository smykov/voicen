<script lang="ts">
  // A key input (U3): type=password, empty at start, presence shown as text only.
  // Typing sets Replace, emptying the input sets Untouched again, the button sets
  // Clear; a Saved resets the slot to Untouched, which empties the input.
  import { t } from "$lib/i18n";
  import { clearKey, resetKey, typeKey, type Draft } from "./draft";
  import { controlId, errorId } from "./fields";
  import FieldMessage from "./FieldMessage.svelte";
  import type { KeySlot } from "./settingsApi";

  let { draft = $bindable(), slot, field }: { draft: Draft; slot: KeySlot; field: string } = $props();

  const edit = $derived(draft.keys[slot]);
  const typed = $derived(typeof edit === "object" ? edit.Replace : "");
  const present = $derived(draft.baseline.keys[slot]);

  function onInput(event: Event & { currentTarget: HTMLInputElement }) {
    const value = event.currentTarget.value;
    draft = value === "" ? resetKey(draft, slot) : typeKey(draft, slot, value);
  }
</script>

<div class="field">
  <label for={controlId(field)}>{t("settings.field.key")}</label>
  <input
    id={controlId(field)}
    type="password"
    autocomplete="off"
    spellcheck="false"
    data-field={field}
    value={typed}
    oninput={onInput}
    aria-invalid={draft.errors[field] ? "true" : undefined}
    aria-describedby={draft.errors[field] ? errorId(field) : undefined}
  />
  {#if edit === "Clear"}
    <p class="hint">{t("settings.key.cleared")}</p>
  {:else if present}
    <p class="hint">{t("settings.key.saved")}</p>
    <button type="button" onclick={() => (draft = clearKey(draft, slot))}>{t("settings.key.clear")}</button>
  {/if}
  <FieldMessage errors={draft.errors} {field} />
</div>
