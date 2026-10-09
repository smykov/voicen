<script lang="ts">
  // General tab: the interface language, one option per `LANGUAGES` entry (the one
  // list in the UI), and Start with Windows. Both take effect when saved (the window
  // renders in the saved view's ui_language; the save step writes the Run value), and the
  // five dictation timeouts (TimeoutsFields, T-073), used by the next job. Last, the
  // "About Voicen" button and dialog (T-023); `aboutOpen` is bound by the page, which
  // closes About when the discard prompt shows.
  import About from "$lib/about/About.svelte";
  import { LANGUAGES, t, type MessageId } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, describedBy } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import TimeoutsFields from "../TimeoutsFields.svelte";

  let {
    draft = $bindable(),
    warnings,
    aboutOpen = $bindable(false),
  }: { draft: Draft; warnings: Record<string, MessageId>; aboutOpen?: boolean } = $props();
  const uiLanguage = "general.ui_language";
  const startWithWindows = "general.start_with_windows";
</script>

<div class="field">
  <label for={controlId(uiLanguage)}>{t("settings.field.ui_language")}</label>
  <select
    id={controlId(uiLanguage)}
    data-field={uiLanguage}
    bind:value={draft.settings.ui_language}
    aria-invalid={draft.errors[uiLanguage] ? "true" : undefined}
    aria-describedby={describedBy(uiLanguage, draft.errors, warnings)}
  >
    {#each LANGUAGES as lang (lang)}
      <option value={lang}>{t(`settings.language.${lang}`)}</option>
    {/each}
  </select>
  <FieldMessage errors={draft.errors} {warnings} field={uiLanguage} />
</div>

<div class="field check">
  <input
    id={controlId(startWithWindows)}
    type="checkbox"
    data-field={startWithWindows}
    bind:checked={draft.settings.start_with_windows}
    aria-invalid={draft.errors[startWithWindows] ? "true" : undefined}
    aria-describedby={describedBy(startWithWindows, draft.errors, warnings)}
  />
  <label for={controlId(startWithWindows)}>{t("settings.field.start_with_windows")}</label>
  <FieldMessage errors={draft.errors} {warnings} field={startWithWindows} />
</div>

<TimeoutsFields bind:draft {warnings} />

<About bind:open={aboutOpen} />
