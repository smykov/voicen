<script lang="ts">
  // The message of one field (U2): `error.<code>` of the last refusal, or the
  // not-restored message of a partially_restored refusal (Draft.errors), referenced by
  // the control's aria-describedby.
  import { t, type MessageId } from "$lib/i18n";
  import { errorId } from "./fields";

  let { errors, field }: { errors: Record<string, MessageId>; field: string } = $props();
</script>

{#if errors[field]}
  <p class="field-error" id={errorId(field)}>{t(errors[field])}</p>
{/if}

<style>
  .field-error {
    color: #b00020;
    margin: 0.25rem 0 0;
    font-size: 0.9em;
  }
</style>
