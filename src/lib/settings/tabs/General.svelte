<script lang="ts">
  // General tab: the interface language, one option per `LANGUAGES` entry (the one
  // list in the UI). It takes effect when saved (the window renders in the saved
  // view's ui_language). Start with Windows is T-034's.
  import { LANGUAGES, t } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, errorId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";

  let { draft = $bindable() }: { draft: Draft } = $props();
  const uiLanguage = "general.ui_language";
</script>

<div class="field">
  <label for={controlId(uiLanguage)}>{t("settings.field.ui_language")}</label>
  <select
    id={controlId(uiLanguage)}
    data-field={uiLanguage}
    bind:value={draft.settings.ui_language}
    aria-invalid={draft.errors[uiLanguage] ? "true" : undefined}
    aria-describedby={draft.errors[uiLanguage] ? errorId(uiLanguage) : undefined}
  >
    {#each LANGUAGES as lang (lang)}
      <option value={lang}>{t(`settings.language.${lang}`)}</option>
    {/each}
  </select>
  <FieldMessage errors={draft.errors} field={uiLanguage} />
</div>
