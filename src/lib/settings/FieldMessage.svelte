<script lang="ts">
  // The error of one field (U2): the text of `error.<code>` from the last refusal,
  // referenced by the control's aria-describedby.
  import { tWire } from "$lib/i18n";
  import { errorId } from "./fields";

  let { errors, field }: { errors: Record<string, string>; field: string } = $props();
</script>

{#if errors[field]}
  <p class="field-error" id={errorId(field)}>{tWire(errors[field])}</p>
{/if}

<style>
  .field-error {
    color: #b00020;
    margin: 0.25rem 0 0;
    font-size: 0.9em;
  }
</style>
