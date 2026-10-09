<script lang="ts">
  // Engine tab: the engine, its URL / model / key (or, for builtin_local, the model
  // select and the model list of BuiltinLocal.svelte), and the speech language. The
  // language list is core's (settings_speech_languages); names come from
  // Intl.DisplayNames in the UI language.
  //
  // T-013: the Test connection button and its result for api and local_server. The run
  // is the page's (+page.svelte `startTest`); this tab only renders it, so it survives
  // the tab being unmounted.
  import { currentLanguage } from "$lib/i18n/language.svelte";
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { fieldLabelId } from "../draft";
  import type { TestOutcome } from "../connectionTest";
  import { controlId, describedBy, hintId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import KeyField from "../KeyField.svelte";
  import BuiltinLocal from "$lib/local-models/BuiltinLocal.svelte";

  let {
    draft = $bindable(),
    warnings,
    languages,
    testPending,
    testShown,
    unrenderedTestFields,
    onTest,
  }: {
    draft: Draft;
    warnings: Record<string, MessageId>;
    languages: readonly string[];
    /** A connection test is in flight (the page's run). */
    testPending: boolean;
    /** The current run's outcome, or null. */
    testShown: TestOutcome | null;
    /** The fields of an `invalid` result with no control on the page, listed by label (L). */
    unrenderedTestFields: readonly string[];
    onTest: () => void;
  } = $props();

  const AUTO = "auto";

  const named = $derived.by(() => {
    const names = new Intl.DisplayNames([currentLanguage()], { type: "language" });
    return languages
      .map((code) => ({ code, name: names.of(code) ?? code }))
      .sort((a, b) => a.name.localeCompare(b.name, currentLanguage()));
  });

  function invalid(field: string): "true" | undefined {
    return draft.errors[field] ? "true" : undefined;
  }

  function described(field: string, hint = false): string | undefined {
    return describedBy(field, draft.errors, warnings, hint);
  }
</script>

<div class="field">
  <label for={controlId("engine.kind")}>{t("settings.field.engine")}</label>
  <select
    id={controlId("engine.kind")}
    data-field="engine.kind"
    bind:value={draft.settings.engine}
    aria-invalid={invalid("engine.kind")}
    aria-describedby={described("engine.kind")}
  >
    <option value="none">{t("settings.engine.none")}</option>
    <option value="api">{t("settings.engine.api")}</option>
    <option value="builtin_local">{t("settings.engine.builtin_local")}</option>
    <option value="local_server">{t("settings.engine.local_server")}</option>
  </select>
  <FieldMessage errors={draft.errors} {warnings} field="engine.kind" />
</div>

{#if draft.settings.engine === "api"}
  <div class="field">
    <label for={controlId("engine.api.base_url")}>{t("settings.field.base_url")}</label>
    <input
      id={controlId("engine.api.base_url")}
      type="url"
      data-field="engine.api.base_url"
      bind:value={draft.settings.api.base_url}
      aria-invalid={invalid("engine.api.base_url")}
      aria-describedby={described("engine.api.base_url", true)}
    />
    <p class="hint" id={hintId("engine.api.base_url")}>{t("settings.base_url.hint")}</p>
    <FieldMessage errors={draft.errors} {warnings} field="engine.api.base_url" />
  </div>
  <div class="field">
    <label for={controlId("engine.api.model")}>{t("settings.field.model")}</label>
    <input
      id={controlId("engine.api.model")}
      type="text"
      data-field="engine.api.model"
      bind:value={draft.settings.api.model}
      aria-invalid={invalid("engine.api.model")}
      aria-describedby={described("engine.api.model")}
    />
    <FieldMessage errors={draft.errors} {warnings} field="engine.api.model" />
  </div>
  <KeyField bind:draft {warnings} slot="transcription_api" field="engine.api.key" />
{:else if draft.settings.engine === "local_server"}
  <div class="field">
    <label for={controlId("engine.local_server.base_url")}>{t("settings.field.base_url")}</label>
    <input
      id={controlId("engine.local_server.base_url")}
      type="url"
      data-field="engine.local_server.base_url"
      bind:value={draft.settings.local_server.base_url}
      aria-invalid={invalid("engine.local_server.base_url")}
      aria-describedby={described("engine.local_server.base_url", true)}
    />
    <p class="hint" id={hintId("engine.local_server.base_url")}>{t("settings.base_url.hint")}</p>
    <FieldMessage errors={draft.errors} {warnings} field="engine.local_server.base_url" />
  </div>
  <div class="field">
    <label for={controlId("engine.local_server.model")}>{t("settings.field.model")}</label>
    <input
      id={controlId("engine.local_server.model")}
      type="text"
      data-field="engine.local_server.model"
      bind:value={draft.settings.local_server.model}
      aria-invalid={invalid("engine.local_server.model")}
      aria-describedby={described("engine.local_server.model")}
    />
    <FieldMessage errors={draft.errors} {warnings} field="engine.local_server.model" />
  </div>
  <KeyField bind:draft {warnings} slot="local_server" field="engine.local_server.key" />
{/if}

{#if draft.settings.engine === "api" || draft.settings.engine === "local_server"}
  <div class="field test-connection">
    <button
      type="button"
      data-testid="settings-test-connection"
      disabled={testPending}
      aria-busy={testPending ? "true" : undefined}
      onclick={onTest}
      >{testPending ? t("settings.test_connection.running") : t("settings.test_connection.button")}</button
    >
    <div data-testid="settings-test-result" role="status">
      {#if testShown !== null}
        <p>{t(testShown.message.id, testShown.message.args)}</p>
        {#if unrenderedTestFields.length > 0}
          <ul>
            {#each unrenderedTestFields as field (field)}
              <li>{t(fieldLabelId(field))}</li>
            {/each}
          </ul>
        {/if}
      {/if}
    </div>
  </div>
{/if}

{#if draft.settings.engine === "builtin_local"}
  <!-- The model select and the model list (spec 002, T-045): mounted only while the
       engine is builtin_local, so only then is local_models_list called. -->
  <BuiltinLocal bind:draft {warnings} />
{/if}

<div class="field">
  <label for={controlId("engine.speech_language")}>{t("settings.field.speech_language")}</label>
  <select
    id={controlId("engine.speech_language")}
    data-field="engine.speech_language"
    bind:value={
      () => draft.settings.speech_language ?? AUTO,
      (value: string) => (draft.settings.speech_language = value === AUTO ? null : value)
    }
    aria-invalid={invalid("engine.speech_language")}
    aria-describedby={described("engine.speech_language")}
  >
    <option value={AUTO}>{t("settings.speech_language.auto")}</option>
    {#each named as language (language.code)}
      <option value={language.code}>{language.name}</option>
    {/each}
  </select>
  <FieldMessage errors={draft.errors} {warnings} field="engine.speech_language" />
</div>
