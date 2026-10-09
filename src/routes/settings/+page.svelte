<script lang="ts">
  // The settings window (spec 004, T-004; docs/decisions/settings-ui.md). It renders
  // only what the shell sends: every SettingsView (settings_get, a Saved outcome,
  // settings://changed) reaches the draft through draft.applyView / draft.applyOutcome,
  // and the UI language is always the saved view's ui_language (U1). Refusals come
  // from core and are shown on their fields (U2); a typed key lives only in the draft
  // until the next Saved (U3). While a save is in flight the fields are disabled, so no
  // edit can be made that the Saved draft would replace.
  //
  // T-039: a focus request (`?tab=&field=` on load, `settings://focus` on the open page)
  // selects `tabOf(tab)` and focuses the control whose data-field equals the field (F);
  // the window closes only through destroy, after an in-page discard prompt when the
  // draft is dirty (D); a not_restored field with no control on the page is named in the
  // form-level message by its label, `settings.field_label.<FieldId>` (L).
  //
  // T-023, T-019: the discard prompt wins over every in-page modal (About, the delete
  // confirmation): onClose dismisses each one registered through $lib/settings/modals
  // (its own keep path) before it shows the prompt.
  //
  // T-015 (decision #52): the warnings of the last Saved outcome are rendered as core
  // sent them (no URL, scheme or host rule here) and kept in page state beside `saved`,
  // not in the Draft (the settings://changed echo of the save rebuilds the Draft). A
  // warning shows next to its field's control; a field with no control on the page is
  // listed at form level by its label (L). The polite "Saved" status adds a one-line
  // summary (`settings.saved_with_warnings`) when there is any. A warning never blocks or
  // changes a save.
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { setLanguage, t, type MessageId } from "$lib/i18n";
  import {
    applyOutcome,
    applyView,
    draftFromView,
    fieldLabelId,
    isDirty,
    saveRequest,
    warningsByField,
    type Draft,
  } from "$lib/settings/draft";
  import {
    destroyWindow,
    getSettings,
    onCloseRequested,
    onFocusRequest,
    onSettingsChanged,
    saveSettings,
    speechLanguages,
    type SettingsView,
  } from "$lib/settings/settingsApi";
  import { provideModalHost } from "$lib/settings/modals";
  import Engine from "$lib/settings/tabs/Engine.svelte";
  import General from "$lib/settings/tabs/General.svelte";
  import History from "$lib/settings/tabs/History.svelte";
  import Output from "$lib/settings/tabs/Output.svelte";
  import PostProcessing from "$lib/settings/tabs/PostProcessing.svelte";
  import Recording from "$lib/settings/tabs/Recording.svelte";

  type Tab = "engine" | "recording" | "output" | "post_processing" | "history" | "general";
  const TABS: readonly { id: Tab; label: MessageId }[] = [
    { id: "engine", label: "settings.tab.engine" },
    { id: "recording", label: "settings.tab.recording" },
    { id: "output", label: "settings.tab.output" },
    { id: "post_processing", label: "settings.tab.post_processing" },
    { id: "history", label: "settings.tab.history" },
    { id: "general", label: "settings.tab.general" },
  ];

  /**
   * The tab a request names (`?tab=` or `settings://focus`): that tab if the page has
   * it, else Engine. The one rule for both paths; no tab is derived from a FieldId.
   */
  function tabOf(requested: string | null | undefined): Tab {
    return TABS.find((item) => item.id === requested)?.id ?? "engine";
  }

  let draft = $state<Draft | null>(null);
  let languages = $state<string[]>([]);
  let tab = $state<Tab>(tabOf(page.url.searchParams.get("tab")));
  let saving = $state(false);
  let saved = $state(false);
  /** FieldId -> message id of the last Saved outcome's warnings (`warningsByField`). */
  let warnings = $state<Record<string, MessageId>>({});
  let ipcFailed = $state(false);
  /** The panel element: the controls on the page are the `[data-field]` elements in it. */
  let panel = $state<HTMLElement | undefined>();
  /** The FieldId to focus once its control can have rendered; a new object per request. */
  let focusRequest = $state<{ field: string } | null>(fieldRequest(page.url.searchParams.get("field")));
  /** The FieldIds of the controls the panel renders now (kept by a MutationObserver). */
  let renderedFields = $state<readonly string[]>([]);
  /** The discard prompt is shown (a close was requested with a dirty draft). */
  let confirmDiscard = $state(false);
  let keepButton = $state<HTMLButtonElement | undefined>();
  /** The in-page modals (About, the delete confirmation), dismissed by a close request. */
  const modals = provideModalHost();

  function fieldRequest(field: string | null | undefined): { field: string } | null {
    return typeof field === "string" ? { field } : null;
  }

  /** A focus request from the URL or the shell: select its tab, then focus its field. */
  function requestFocus(requestedTab: string | null | undefined, field: string | null | undefined) {
    tab = tabOf(requestedTab);
    focusRequest = fieldRequest(field);
  }

  /** The `[data-field]` elements of `root`, compared by value (never a built selector). */
  function fieldControls(root: HTMLElement): HTMLElement[] {
    return Array.from(root.querySelectorAll<HTMLElement>("[data-field]"));
  }

  // Effects run after the DOM update, so the requested tab's controls exist here. No
  // control with exactly that data-field (another tab, another engine, a malformed id):
  // nothing is focused and nothing is shown.
  $effect(() => {
    const request = focusRequest;
    if (request === null || draft === null || panel === undefined) return;
    focusRequest = null;
    fieldControls(panel)
      .find((control) => control.dataset.field === request.field)
      ?.focus();
  });

  // What "has a control on the page" means for the form-level list (L): the panel's
  // [data-field] elements, whatever tab, engine or later rule decided to render them.
  $effect(() => {
    const root = panel;
    if (root === undefined) return;
    const collect = () => {
      renderedFields = fieldControls(root).map((control) => control.dataset.field ?? "");
    };
    collect();
    const observer = new MutationObserver(collect);
    observer.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-field"] });
    return () => observer.disconnect();
  });

  /** The last Saved had a warning (field-level or form-level): the Saved status says so. */
  const hasWarnings = $derived(Object.keys(warnings).length > 0);

  /** The warnings whose field has no control on the page, listed at form level (L). */
  const unrenderedWarnings = $derived(Object.entries(warnings).filter(([field]) => !renderedFields.includes(field)));

  /** The not_restored fields of the form error with no control on the page (L). */
  const unrenderedNotRestored = $derived(
    (draft?.formError?.not_restored ?? []).filter((field) => !renderedFields.includes(field)),
  );

  $effect(() => {
    if (confirmDiscard) keepButton?.focus();
  });

  /**
   * `tauri://close-requested` (D). tauri destroys the window after this returns unless
   * it was prevented, so a dirty draft is prevented here, synchronously, and the prompt
   * decides. A clean or not-yet-loaded draft is not prevented. Never throws: a throwing
   * handler would leave the window impossible to close.
   */
  function onClose(event: { preventDefault(): void }) {
    try {
      if (draft !== null && isDirty(draft)) {
        event.preventDefault();
        // The discard prompt wins: an open modal dialog would leave the prompt inert.
        modals.dismissAll();
        confirmDiscard = true;
      }
    } catch {
      // Not prevented: the window closes, as for a clean draft.
    }
  }

  function keepEditing() {
    confirmDiscard = false;
  }

  /** Discard: the window closes through destroy only (never close(), not granted). */
  async function discard() {
    try {
      await destroyWindow();
    } catch {
      confirmDiscard = false;
      ipcFailed = true;
    }
  }

  function onWindowKeydown(event: KeyboardEvent) {
    if (confirmDiscard && event.key === "Escape") keepEditing();
  }

  // The UI language follows the saved view, whichever path brought it.
  $effect(() => {
    if (draft) setLanguage(draft.baseline.settings.ui_language);
  });

  function receive(view: SettingsView) {
    draft = draft === null ? draftFromView(view) : applyView(draft, view);
  }

  async function save() {
    if (draft === null || saving) return;
    saving = true;
    saved = false;
    warnings = {};
    ipcFailed = false;
    try {
      const outcome = await saveSettings(saveRequest(draft));
      draft = applyOutcome(draft, outcome);
      saved = "Saved" in outcome;
      if ("Saved" in outcome) warnings = warningsByField(outcome.Saved.warnings);
    } catch {
      // The rejection text is never shown (contracts/ipc.md "Errors"); the draft stays.
      ipcFailed = true;
    } finally {
      saving = false;
    }
  }

  onMount(() => {
    const unlisteners: (() => void)[] = [];
    let disposed = false;
    const keep = (stop: () => void) => {
      if (disposed) stop();
      else unlisteners.push(stop);
    };
    (async () => {
      try {
        // The close guard first, then every other listener, all before settings_get.
        // Only settings://changed and settings_get build a draft, and both come after
        // the guard, so no draft is ever editable without it (D; while the draft is
        // null onClose prevents nothing). A save from elsewhere before settings_get is
        // not lost, and a focus request sent while the page loads waits for the draft
        // (the focus effect) and replaces the URL's.
        keep(await onCloseRequested(onClose));
        keep(await onSettingsChanged(receive));
        keep(await onFocusRequest((request) => requestFocus(request.tab, request.field)));
        receive(await getSettings());
      } catch {
        ipcFailed = true;
      }
    })();
    speechLanguages().then(
      (list) => (languages = list),
      () => (ipcFailed = true),
    );
    return () => {
      disposed = true;
      for (const stop of unlisteners) stop();
    };
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

<main class="settings" inert={confirmDiscard}>
  <h1>{t("settings.title")}</h1>

  {#if ipcFailed || draft?.formError}
    <div class="message error" role="alert">
      {#if ipcFailed}
        <p>{t("error.ipc_unavailable")}</p>
      {/if}
      {#if draft?.formError}
        <p>{t(draft.formError.message)}</p>
        {#if unrenderedNotRestored.length > 0}
          <ul>
            {#each unrenderedNotRestored as field, index (index)}
              <li>{t(fieldLabelId(field))}</li>
            {/each}
          </ul>
        {/if}
      {/if}
    </div>
  {/if}

  {#if draft !== null}
    {#if draft.baseline.unavailable}
      <p class="message notice" role="status">{t("notice.settings_unavailable")}</p>
    {/if}
    {#if draft.baseline.reset_notice}
      <p class="message notice" role="status">{t("notice.settings_reset")}</p>
    {/if}

    <div class="tabs" role="tablist">
      {#each TABS as item (item.id)}
        <button
          type="button"
          role="tab"
          id={`tab-${item.id}`}
          data-testid={`tab-${item.id}`}
          aria-selected={tab === item.id ? "true" : "false"}
          aria-controls="settings-panel"
          tabindex={tab === item.id ? 0 : -1}
          onclick={() => (tab = item.id)}>{t(item.label)}</button
        >
      {/each}
    </div>

    <div class="panel" id="settings-panel" role="tabpanel" aria-labelledby={`tab-${tab}`} bind:this={panel}>
      <fieldset disabled={saving}>
        {#if tab === "engine"}
          <Engine bind:draft {warnings} {languages} />
        {:else if tab === "recording"}
          <Recording bind:draft {warnings} />
        {:else if tab === "output"}
          <Output bind:draft {warnings} />
        {:else if tab === "post_processing"}
          <PostProcessing bind:draft {warnings} />
        {:else if tab === "history"}
          <History bind:draft {warnings} />
        {:else}
          <General bind:draft {warnings} />
        {/if}
      </fieldset>
    </div>

    <div class="actions">
      <button type="button" data-testid="settings-save" disabled={saving} onclick={save}>{t("settings.save")}</button>
      {#if saved}
        <!-- Polite: a warning is not an alert; the summary names no field and no text (W).
             The separating space is inside the expression: Svelte trims whitespace at the
             start of a block body. -->
        <p role="status">
          {t("settings.saved")}{#if hasWarnings}{` ${t("settings.saved_with_warnings")}`}{/if}
        </p>
      {/if}
    </div>

    {#if unrenderedWarnings.length > 0}
      <div class="message warning" data-testid="settings-warnings">
        <ul>
          {#each unrenderedWarnings as [field, message] (field)}
            <li><strong>{t(fieldLabelId(field))}</strong> {t(message)}</li>
          {/each}
        </ul>
      </div>
    {/if}
  {/if}
</main>

{#if confirmDiscard}
  <div class="backdrop">
    <div
      class="dialog"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="settings-discard-title"
      aria-describedby="settings-discard-message"
      tabindex="-1"
    >
      <h2 id="settings-discard-title">{t("settings.discard.title")}</h2>
      <p id="settings-discard-message">{t("settings.discard.message")}</p>
      <div class="actions">
        <button type="button" data-testid="settings-discard-keep" bind:this={keepButton} onclick={keepEditing}
          >{t("settings.discard.keep")}</button
        >
        <button type="button" data-testid="settings-discard-confirm" onclick={discard}
          >{t("settings.discard.confirm")}</button
        >
      </div>
    </div>
  </div>
{/if}

<style>
  :root {
    font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
    color: #0f0f0f;
    background-color: #f6f6f6;
  }

  .settings {
    max-width: 40rem;
    margin: 0 auto;
    padding: 1rem;
  }

  .tabs {
    display: flex;
    gap: 0.25rem;
    border-bottom: 1px solid #ccc;
  }

  .tabs button {
    border: none;
    background: none;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
  }

  .tabs button[aria-selected="true"] {
    border-bottom: 2px solid #1a5fb4;
    font-weight: 600;
  }

  .panel {
    padding: 1rem 0;
  }

  .panel fieldset {
    border: none;
    margin: 0;
    padding: 0;
    min-width: 0;
  }

  .panel :global(.field) {
    margin-bottom: 1rem;
  }

  .panel :global(.field label) {
    display: block;
    margin-bottom: 0.25rem;
  }

  .panel :global(.field.check label) {
    display: inline;
  }

  .panel :global(.hint) {
    margin: 0.25rem 0 0;
    color: #555;
    font-size: 0.9em;
  }

  .panel :global([aria-invalid="true"]) {
    border-color: #b00020;
    outline: 1px solid #b00020;
  }

  .message {
    padding: 0.5rem 0.75rem;
    border-radius: 4px;
  }

  .message.error {
    background: #fde7ea;
  }

  .message.notice {
    background: #fff4d6;
  }

  .message.warning {
    margin-top: 1rem;
    background: #fff4d6;
  }

  .message.warning ul {
    margin: 0;
    padding-left: 1.25rem;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 35%);
  }

  .dialog {
    max-width: 28rem;
    padding: 1rem 1.25rem;
    border-radius: 6px;
    background: #fff;
    box-shadow: 0 4px 16px rgb(0 0 0 / 25%);
  }

  .dialog h2 {
    margin-top: 0;
    font-size: 1.1em;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 1rem;
  }
</style>
