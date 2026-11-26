<script lang="ts">
  // Calls tab (spec 007 US9, T-155): the call hotkey (captured like the dictation hotkey),
  // the reminder, the primary call language and the storage folder, all bound to the
  // draft. Every rule is core's: hotkey equality and availability, writability and the
  // reason come back as FieldErrors (U2, `errorsByField`). The provider line shows the
  // saved view's `provider_host` as core computed it (FR-031); the window parses no URL
  // (settings-ui W). The language list is core's (settings_speech_languages), in core's
  // order and without "auto": a call always has a primary language.
  import { t, type MessageId } from "$lib/i18n";
  import { currentLanguage } from "$lib/i18n/language.svelte";
  import type { Draft } from "../draft";
  import { controlId, describedBy, hintId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import HotkeyField from "../HotkeyField.svelte";

  let {
    draft = $bindable(),
    warnings,
    languages,
  }: { draft: Draft; warnings: Record<string, MessageId>; languages: readonly string[] } = $props();

  const reminder = "calls.reminder";
  const primaryLanguage = "calls.primary_language";
  const storageRoot = "calls.storage_root";

  const named = $derived.by(() => {
    const names = new Intl.DisplayNames([currentLanguage()], { type: "language" });
    return languages.map((code) => ({ code, name: names.of(code) ?? code }));
  });

  const providerHost = $derived(draft.baseline.provider_host);

  // T-232 (decisions #306, #373): the saved view says the call slot's registration failed;
  // the mark stays while the hotkey is edited until a Saved view clears it, and a draft
  // error of its own on the field wins. Calls tab only, never inferred here.
  const callHotkeyUnavailable: MessageId = "error.hotkey.unavailable";
  const hotkeyErrors = $derived(
    draft.baseline.call_hotkey_unavailable
      ? { "calls.hotkey": callHotkeyUnavailable, ...draft.errors }
      : draft.errors,
  );
</script>

<HotkeyField
  bind:value={draft.settings.calls.hotkey}
  field="calls.hotkey"
  label="settings.field_label.calls.hotkey"
  errors={hotkeyErrors}
  {warnings}
/>

<div class="field check">
  <input
    id={controlId(reminder)}
    type="checkbox"
    data-field={reminder}
    bind:checked={draft.settings.calls.reminder}
    aria-invalid={draft.errors[reminder] ? "true" : undefined}
    aria-describedby={describedBy(reminder, draft.errors, warnings)}
  />
  <label for={controlId(reminder)}>{t("settings.field_label.calls.reminder")}</label>
  <FieldMessage errors={draft.errors} {warnings} field={reminder} />
</div>

<div class="field">
  <label for={controlId(primaryLanguage)}>{t("settings.field_label.calls.primary_language")}</label>
  <select
    id={controlId(primaryLanguage)}
    data-field={primaryLanguage}
    bind:value={draft.settings.calls.primary_language}
    aria-invalid={draft.errors[primaryLanguage] ? "true" : undefined}
    aria-describedby={describedBy(primaryLanguage, draft.errors, warnings)}
  >
    {#each named as language (language.code)}
      <option value={language.code}>{language.name}</option>
    {/each}
  </select>
  <FieldMessage errors={draft.errors} {warnings} field={primaryLanguage} />
</div>

<div class="field">
  <label for={controlId(storageRoot)}>{t("settings.field_label.calls.storage_root")}</label>
  <!-- Empty means the default root: shown as "" for null, and an empty entry is sent as null. -->
  <input
    id={controlId(storageRoot)}
    type="text"
    data-field={storageRoot}
    bind:value={
      () => draft.settings.calls.storage_root ?? "",
      (value: string) => (draft.settings.calls.storage_root = value === "" ? null : value)
    }
    aria-invalid={draft.errors[storageRoot] ? "true" : undefined}
    aria-describedby={describedBy(storageRoot, draft.errors, warnings, true)}
  />
  <p class="hint" id={hintId(storageRoot)}>{t("settings.calls.storage_root.hint")}</p>
  <FieldMessage errors={draft.errors} {warnings} field={storageRoot} />
</div>

{#if providerHost !== null}
  <p class="hint">{t("settings.calls.provider_notice", { host: providerHost })}</p>
{/if}
