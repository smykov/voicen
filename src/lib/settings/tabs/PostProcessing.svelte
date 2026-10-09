<script lang="ts">
  // Post-processing tab (spec 003 US3, T-021): on/off, the endpoint, model and key of the
  // LLM, and the prompt. Every value is the view's (U1): the prompt shown is whatever core
  // sent, the starter prompt on a first run. The UI holds no post-processing rule: what
  // is required while it is on is core's post_process::settings::validate, and a refusal
  // reaches the fields through Draft.errors like any other (U2). The fields stay editable
  // while the toggle is off (004 FR-004: kept as entered). The privacy note is always
  // shown; its text already says when it applies. No base URL hint: the transcription
  // hint (`settings.base_url.hint`) names /audio/transcriptions and does not apply here.
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, describedBy } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import KeyField from "../KeyField.svelte";

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();
  const enabled = "post_processing.enabled";
  const baseUrl = "post_processing.base_url";
  const model = "post_processing.model";
  const prompt = "post_processing.prompt";

  function invalid(field: string): "true" | undefined {
    return draft.errors[field] ? "true" : undefined;
  }

  function described(field: string): string | undefined {
    return describedBy(field, draft.errors, warnings);
  }
</script>

<p class="hint privacy-note">{t("settings.post_processing.privacy_note")}</p>

<div class="field check">
  <input
    id={controlId(enabled)}
    type="checkbox"
    data-field={enabled}
    bind:checked={draft.settings.post_processing.enabled}
    aria-invalid={invalid(enabled)}
    aria-describedby={described(enabled)}
  />
  <label for={controlId(enabled)}>{t("settings.field.post_processing_enabled")}</label>
  <FieldMessage errors={draft.errors} {warnings} field={enabled} />
</div>

<div class="field">
  <label for={controlId(baseUrl)}>{t("settings.field.base_url")}</label>
  <input
    id={controlId(baseUrl)}
    type="url"
    data-field={baseUrl}
    bind:value={draft.settings.post_processing.base_url}
    aria-invalid={invalid(baseUrl)}
    aria-describedby={described(baseUrl)}
  />
  <FieldMessage errors={draft.errors} {warnings} field={baseUrl} />
</div>

<div class="field">
  <label for={controlId(model)}>{t("settings.field.model")}</label>
  <input
    id={controlId(model)}
    type="text"
    data-field={model}
    bind:value={draft.settings.post_processing.model}
    aria-invalid={invalid(model)}
    aria-describedby={described(model)}
  />
  <FieldMessage errors={draft.errors} {warnings} field={model} />
</div>

<KeyField bind:draft {warnings} slot="post_processing" field="post_processing.key" />

<div class="field">
  <label for={controlId(prompt)}>{t("settings.field.prompt")}</label>
  <textarea
    id={controlId(prompt)}
    rows="8"
    data-field={prompt}
    bind:value={draft.settings.post_processing.prompt}
    aria-invalid={invalid(prompt)}
    aria-describedby={described(prompt)}
  ></textarea>
  <FieldMessage errors={draft.errors} {warnings} field={prompt} />
</div>

<style>
  textarea {
    box-sizing: border-box;
    width: 100%;
    font: inherit;
  }
</style>
