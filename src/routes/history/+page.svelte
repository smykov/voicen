<script lang="ts">
  // The History window (label `history`; specs/005-transcript-history contracts/ipc.md, T-022).
  // All its IPC goes through $lib/history/historyApi:
  // - the state is the shell's HistoryView, replaced whole by `history_get`, each
  //   `history://changed` payload and the view `history_clear` returns (no merge, no sort);
  //   a `history_get` reply older than an applied event or clear is dropped;
  // - the language is the saved settings' (settings_get, settings://changed);
  // - Copy -> history_copy { id }: "Copied" for 2 s, or the one copy alert (catalog text only;
  //   the rejection is never shown or logged, it may name nothing but a code anyway); a later
  //   successful copy, a clear or an off view clears that alert;
  // - Open settings -> the shell's open-settings request at the History tab.
  import { onMount } from "svelte";
  import { setLanguage, type MessageId, type UiLanguage } from "$lib/i18n";
  import HistoryList from "$lib/history/HistoryList.svelte";
  import {
    clearHistory,
    copyEntry,
    getHistory,
    getSettings,
    onHistoryChanged,
    onSettingsChanged,
    openHistorySettings,
    type HistoryEntry,
    type HistoryView,
  } from "$lib/history/historyApi";
  import type { SettingsView } from "$lib/settings/settingsApi";

  const COPIED_MS = 2_000;
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;

  let view = $state.raw<HistoryView | null>(null);
  let lang = $state<UiLanguage>("en");
  let now = $state(Date.now());
  let copied = $state(false);
  let copyFailed = $state(false);
  let error = $state<MessageId | null>(null);
  /** Bumped by every applied view: a `history_get` reply that started before is stale. */
  let generation = 0;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  function apply(next: HistoryView) {
    generation += 1;
    view = next;
    now = Date.now();
    if (!next.enabled) {
      copyFailed = false;
      copied = false;
    }
  }

  function applySettings(settings: SettingsView) {
    lang = settings.settings.ui_language;
    setLanguage(lang);
  }

  async function load() {
    const started = generation;
    try {
      const next = await getHistory();
      if (started === generation) apply(next);
    } catch {
      error = "error.ipc_unavailable";
    }
  }

  function showCopied() {
    clearTimeout(copiedTimer);
    copied = true;
    copiedTimer = setTimeout(() => (copied = false), COPIED_MS);
  }

  async function copy(entry: HistoryEntry) {
    clearTimeout(copiedTimer);
    copied = false;
    try {
      await copyEntry(entry.id);
      copyFailed = false;
      showCopied();
    } catch {
      // `not_found` or `clipboard_unavailable`: the same alert; the rejection is not shown.
      copyFailed = true;
    }
  }

  async function clear() {
    try {
      const next = await clearHistory();
      copyFailed = false;
      apply(next);
    } catch {
      error = "error.ipc_unavailable";
    }
  }

  async function openSettings() {
    try {
      await openHistorySettings();
    } catch {
      error = "error.ipc_unavailable";
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
      // Listeners first, so no change between them and the first read is lost.
      try {
        keep(await onHistoryChanged((next) => apply(next)));
        keep(await onSettingsChanged((settings) => applySettings(settings)));
      } catch {
        error = "error.ipc_unavailable";
      }
      try {
        applySettings(await getSettings());
      } catch {
        error = "error.ipc_unavailable";
      }
      await load();
    })();
    return () => {
      disposed = true;
      clearTimeout(copiedTimer);
      for (const stop of unlisteners) stop();
    };
  });
</script>

<main class="history" data-testid="history-page">
  <HistoryList
    {view}
    {lang}
    {now}
    {timeZone}
    {copied}
    {copyFailed}
    {error}
    onCopy={copy}
    onClear={clear}
    onOpenSettings={openSettings}
  />
</main>

<style>
  .history {
    font:
      14px/20px "Segoe UI",
      system-ui,
      sans-serif;
    padding: 12px 16px;
  }
</style>
