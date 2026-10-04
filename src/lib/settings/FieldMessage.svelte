<script lang="ts">
  // The messages of one field, referenced by the control's aria-describedby
  // (`describedBy`): the error of the last refusal (U2: `error.<code>`, or the
  // not-restored message of a partially_restored refusal; Draft.errors), and the warning
  // of the last Saved outcome (T-015: core's message id as given; page state, not the
  // Draft). A warning is not an error: it never sets aria-invalid.
  import { t, type MessageId } from "$lib/i18n";
  import { errorId, warningId } from "./fields";

  let {
    errors,
    warnings,
    field,
  }: { errors: Record<string, MessageId>; warnings: Record<string, MessageId>; field: string } = $props();
</script>

{#if errors[field]}
  <p class="field-error" id={errorId(field)}>{t(errors[field])}</p>
{/if}
{#if warnings[field]}
  <p class="field-warning" id={warningId(field)}>{t(warnings[field])}</p>
{/if}

<style>
  .field-error,
  .field-warning {
    margin: 0.25rem 0 0;
    font-size: 0.9em;
  }

  .field-error {
    color: #b00020;
  }

  .field-warning {
    color: #7a4f00;
  }
</style>
