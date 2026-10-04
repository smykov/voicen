<script lang="ts">
  // The settings window (spec 004, T-004). It renders only what the shell sends:
  // every SettingsView (settings_get, a Saved outcome, settings://changed) reaches the
  // draft through draft.applyView / draft.applyOutcome, and the UI language is always
  // the saved view's ui_language (U1). Refusals come from core and are shown on their
  // fields (U2); a typed key lives only in the draft until the next Saved (U3).
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { setLanguage, t, tWire, type MessageId } from "$lib/i18n";
  import { applyOutcome, applyView, draftFromView, saveRequest, type Draft } from "$lib/settings/draft";
  import {
    getSettings,
    onSettingsChanged,
    saveSettings,
    speechLanguages,
    type SettingsView,
  } from "$lib/settings/settingsApi";
  import Engine from "$lib/settings/tabs/Engine.svelte";
  import General from "$lib/settings/tabs/General.svelte";
  import History from "$lib/settings/tabs/History.svelte";
  import Output from "$lib/settings/tabs/Output.svelte";
  import Recording from "$lib/settings/tabs/Recording.svelte";

  type Tab = "engine" | "recording" | "output" | "history" | "general";
  const TABS: readonly { id: Tab; label: MessageId }[] = [
    { id: "engine", label: "settings.tab.engine" },
    { id: "recording", label: "settings.tab.recording" },
    { id: "output", label: "settings.tab.output" },
    { id: "history", label: "settings.tab.history" },
    { id: "general", label: "settings.tab.general" },
  ];

  /** `?tab=` from the shell (settings_window URL), else the Engine tab. */
  function initialTab(): Tab {
    const requested = page.url.searchParams.get("tab");
    return TABS.find((tab) => tab.id === requested)?.id ?? "engine";
  }

  let draft = $state<Draft | null>(null);
  let languages = $state<string[]>([]);
  let tab = $state<Tab>(initialTab());
  let saving = $state(false);
  let saved = $state(false);
  let ipcFailed = $state(false);

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
    ipcFailed = false;
    try {
      const outcome = await saveSettings(saveRequest(draft));
      draft = applyOutcome(draft, outcome);
      saved = "Saved" in outcome;
    } catch {
      // The rejection text is never shown (contracts/ipc.md "Errors"); the draft stays.
      ipcFailed = true;
    } finally {
      saving = false;
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    (async () => {
      try {
        // Listen first, so a save from elsewhere between the two calls is not lost.
        const stop = await onSettingsChanged(receive);
        if (disposed) stop();
        else unlisten = stop;
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
      unlisten?.();
    };
  });
</script>

<main class="settings">
  <h1>{t("settings.title")}</h1>

  {#if ipcFailed || draft?.formError}
    <div class="message error" role="alert">
      {#if ipcFailed}
        <p>{t("error.ipc_unavailable")}</p>
      {/if}
      {#if draft?.formError}
        <p>{tWire(draft.formError.message)}</p>
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

    <div class="panel" id="settings-panel" role="tabpanel" aria-labelledby={`tab-${tab}`}>
      {#if tab === "engine"}
        <Engine bind:draft {languages} />
      {:else if tab === "recording"}
        <Recording bind:draft />
      {:else if tab === "output"}
        <Output bind:draft />
      {:else if tab === "history"}
        <History bind:draft />
      {:else}
        <General bind:draft />
      {/if}
    </div>

    <div class="actions">
      <button type="button" data-testid="settings-save" disabled={saving} onclick={save}>{t("settings.save")}</button>
      {#if saved}
        <p role="status">{t("settings.saved")}</p>
      {/if}
    </div>
  {/if}
</main>

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

  .actions {
    display: flex;
    align-items: center;
    gap: 1rem;
  }
</style>
