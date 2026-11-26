<script lang="ts">
  // The History window's content (specs/005-transcript-history contracts/ipc.md "UI contract",
  // T-022): it renders the view it is given and reports the user's actions; it does no IPC.
  // - the rows are the view's entries in view order (never re-sorted); the text reaches the DOM
  //   only as a text node; a row shows its preview, time and engine label, an expand toggle
  //   (`aria-expanded`) only for a cut preview, and Copy;
  // - enabled with no entries: "No transcripts yet"; off: "History is off" + "Open settings",
  //   no entries and no Clear;
  // - alerts: the copy failure (catalog text only, never entry text or the error code) and the
  //   view's notice while the view carries it; `copied` shows one status "Copied".
  import { SvelteSet } from "svelte/reactivity";
  import { t, type MessageId, type UiLanguage } from "$lib/i18n";
  import { engineLabel, formatReadyAt, previewOf } from "./format";
  import type { HistoryEntry, HistoryNotice, HistoryView } from "./historyApi";

  let {
    view,
    lang,
    now,
    timeZone,
    copied,
    copyFailed,
    error,
    onCopy,
    onClear,
    onOpenSettings,
  }: {
    view: HistoryView | null;
    lang: UiLanguage;
    /** The present (epoch ms) the times are formatted against. */
    now: number;
    timeZone: string;
    copied: boolean;
    copyFailed: boolean;
    /** An IPC failure of the page (catalog id), or null. */
    error: MessageId | null;
    onCopy: (entry: HistoryEntry) => void;
    onClear: () => void;
    onOpenSettings: () => void;
  } = $props();

  const NOTICE: Record<HistoryNotice, MessageId> = {
    save_failed: "history.notice.save_failed",
    delete_failed: "history.notice.delete_failed",
  };

  /** Ids of the rows shown whole. */
  const expanded = new SvelteSet<number>();

  function toggle(id: number) {
    if (expanded.has(id)) expanded.delete(id);
    else expanded.add(id);
  }
</script>

<h1>{t("history.title")}</h1>

{#if error !== null}
  <p class="alert" role="alert">{t(error)}</p>
{/if}
{#if view !== null && view.notice !== null}
  <p class="alert" role="alert">{t(NOTICE[view.notice])}</p>
{/if}
{#if copyFailed}
  <p class="alert" role="alert">{t("history.copy_failed")}</p>
{/if}
{#if copied}
  <p class="status" role="status">{t("history.copied")}</p>
{/if}

{#if view !== null}
  {#if !view.enabled}
    <p class="off">{t("history.off")}</p>
    <button type="button" onclick={onOpenSettings}>{t("history.open_settings")}</button>
  {:else}
    <div class="toolbar">
      <button type="button" onclick={onClear}>{t("history.clear")}</button>
    </div>
    {#if view.entries.length === 0}
      <p class="empty">{t("history.empty")}</p>
    {:else}
      <ul class="list" aria-label={t("history.list_label")}>
        {#each view.entries as entry (entry.id)}
          {@const preview = previewOf(entry.text)}
          {@const whole = expanded.has(entry.id)}
          <li class="row">
            <p class="text">{whole || !preview.truncated ? entry.text : preview.preview}</p>
            <div class="meta">
              <span class="time">{formatReadyAt(entry.readyAtMs, { now, lang, timeZone })}</span>
              <span class="engine">{engineLabel(entry.engine, lang)}</span>
              {#if preview.truncated}
                <button type="button" aria-expanded={whole} onclick={() => toggle(entry.id)}>
                  {t(whole ? "history.collapse" : "history.expand")}
                </button>
              {/if}
              <button type="button" onclick={() => onCopy(entry)}>{t("history.copy")}</button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
{/if}

<style>
  h1 {
    font-size: 16px;
    margin: 0 0 8px;
  }

  .alert {
    color: #b00020;
  }

  .toolbar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 8px;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    padding: 6px 0;
    border-bottom: 1px solid #ddd;
  }

  .text {
    margin: 0 0 4px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    color: #555;
  }

  .time {
    font-variant-numeric: tabular-nums;
  }

  .engine {
    flex: 1 1 auto;
  }
</style>
