<script lang="ts">
  // The Calls window (label `calls`; specs/007-call-mode-record contracts/ipc.md, T-143;
  // docs/decisions/calls-ui.md). It renders only what the shell sends, through
  // $lib/calls/callsApi and settings_get:
  // - the list in calls_list order (never re-sorted, D1); titles and names are wire text;
  //   date and time are the written digits of startedAt (D2, $lib/calls/view);
  // - a row's actions are `actionsFor(state)` (D3); a damaged row opens no detail;
  // - transcript text reaches the DOM only as text nodes, search matches as <mark> around
  //   text segments (D5), never as markup;
  // - a refused action shows one alert with `callErrorMessageId(error)` (D4); a cancelled
  //   export shows nothing; the alert remembers which load set it (list, detail), and that
  //   load's next success clears it (a list error is not cleared by a detail reply, nor an
  //   action error by either);
  // - the language and the call hotkey are the saved settings' (settings_get,
  //   settings://changed); a language change refetches the list, whose damaged titles core
  //   renders in the UI language (D8). calls://changed refetches the list and the open
  //   detail of that call; calls://open selects that call.
  // Delete is rendered from the action set; its behaviour (confirmation, refusals) is T-154's.
  import { onMount } from "svelte";
  import { setLanguage, t, type MessageId } from "$lib/i18n";
  import { getSettings, type SettingsView } from "$lib/settings/settingsApi";
  import {
    copyMarkdown,
    exportCall,
    getCall,
    listCalls,
    onCallsChanged,
    onCallsOpen,
    onSettingsChanged,
    openFolder,
    type CallDetail,
    type CallList,
    type CallRef,
    type CallSummary,
  } from "$lib/calls/callsApi";
  import {
    ACTION_LABEL,
    actionsFor,
    callErrorMessageId,
    failedMessageId,
    formatDuration,
    formatLineTime,
    formatStartedAt,
    noMatches,
    segmentsFor,
    speakerColor,
    stateMessageId,
    type CallAction,
  } from "$lib/calls/view";

  let list = $state.raw<CallList | null>(null);
  let hotkey = $state("");
  /** The call whose detail is shown (or being fetched); null = none. */
  let selected = $state.raw<CallRef | null>(null);
  let detail = $state.raw<CallDetail | null>(null);
  let query = $state("");
  /**
   * The one shown error (catalog id) and what set it, or null. `list` / `detail`: a refused
   * calls_list / calls_get, cleared when that fetch next succeeds; `other`: an action or the
   * IPC setup, cleared only by the next action or selection.
   */
  let error = $state.raw<{ id: MessageId; source: "list" | "detail" | "other" } | null>(null);
  const showError = (id: MessageId, source: "list" | "detail" | "other") => (error = { id, source });
  const clearErrorOf = (source: "list" | "detail") => {
    if (error?.source === source) error = null;
  };
  /** Generations: a reply of an older fetch is dropped. */
  let listGeneration = 0;
  let detailGeneration = 0;

  const sameRef = (a: CallRef | null, b: CallRef) => a !== null && a.root === b.root && a.id === b.id;

  function applySettings(view: SettingsView) {
    setLanguage(view.settings.ui_language);
    hotkey = view.settings.calls.hotkey;
  }

  async function refreshList() {
    const generation = ++listGeneration;
    try {
      const next = await listCalls();
      if (generation === listGeneration) {
        list = next;
        clearErrorOf("list");
      }
    } catch (refusal) {
      if (generation === listGeneration) showError(callErrorMessageId(refusal), "list");
    }
  }

  async function loadDetail(ref: CallRef) {
    const generation = ++detailGeneration;
    try {
      const next = await getCall(ref.root, ref.id);
      if (generation === detailGeneration && sameRef(selected, ref)) {
        detail = next;
        clearErrorOf("detail");
      }
    } catch (refusal) {
      if (generation === detailGeneration && sameRef(selected, ref)) {
        detail = null;
        showError(callErrorMessageId(refusal), "detail");
      }
    }
  }

  function select(ref: CallRef) {
    if (!sameRef(selected, ref)) {
      selected = { root: ref.root, id: ref.id };
      detail = null;
      query = "";
    }
    error = null;
    void loadDetail(ref);
  }

  async function run(action: CallAction, call: CallSummary) {
    error = null;
    try {
      switch (action) {
        case "copy":
          await copyMarkdown(call.root, call.id);
          break;
        case "exportMd":
          await exportCall(call.root, call.id, "md");
          break;
        case "exportJson":
          await exportCall(call.root, call.id, "json");
          break;
        case "openFolder":
          await openFolder(call.root, call.id);
          break;
        case "delete":
          // T-154: confirmation and calls_delete.
          break;
      }
    } catch (refusal) {
      showError(callErrorMessageId(refusal), "other");
    }
  }

  function speakerName(id: string): string {
    return detail?.speakers.find((s) => s.id === id)?.name ?? id;
  }

  const showNoMatches = $derived(detail !== null && noMatches(detail.lines, query));

  onMount(() => {
    const unlisteners: (() => void)[] = [];
    let disposed = false;
    const keep = (stop: () => void) => {
      if (disposed) stop();
      else unlisteners.push(stop);
    };
    (async () => {
      // Listeners first, so no change between them and the first fetch is lost.
      try {
        keep(
          await onCallsChanged((ref) => {
            void refreshList();
            if (sameRef(selected, ref)) void loadDetail(ref);
          }),
        );
        keep(await onCallsOpen((ref) => select(ref)));
        keep(
          await onSettingsChanged((view) => {
            applySettings(view);
            void refreshList();
          }),
        );
      } catch {
        showError("error.ipc_unavailable", "other");
      }
      try {
        applySettings(await getSettings());
      } catch {
        showError("error.ipc_unavailable", "other");
      }
      await refreshList();
    })();
    return () => {
      disposed = true;
      for (const stop of unlisteners) stop();
    };
  });
</script>

<main class="calls" data-testid="calls-page">
  {#if error !== null}
    <p class="error" role="alert">{t(error.id)}</p>
  {/if}

  {#if list !== null}
    {#each list.unavailableRoots as root (root)}
      <p class="root-unavailable" data-testid="call-root-unavailable">{t("calls.root_unavailable", { root })}</p>
    {/each}

    {#if list.calls.length === 0}
      <p class="empty" data-testid="call-empty">{t("calls.empty", { hotkey })}</p>
    {:else}
      <ul class="list" data-testid="call-list">
        {#each list.calls as call (`${call.root}\n${call.id}`)}
          <li class="row" class:selected={sameRef(selected, call)} data-testid="call-row">
            {#if call.state === "damaged"}
              <span class="title" data-testid="call-title">{call.title}</span>
            {:else}
              <button type="button" class="title select" data-testid="call-select" onclick={() => select(call)}>
                <span data-testid="call-title">{call.title}</span>
              </button>
            {/if}
            <span class="date" data-testid="call-date">{formatStartedAt(call.startedAt)}</span>
            <span class="duration" data-testid="call-duration">{formatDuration(call.durationMs)}</span>
            <span class="state" data-testid="call-state">{t(stateMessageId(call.state))}</span>
            {#if call.state === "failed" && call.failReason !== undefined}
              <span class="reason" data-testid="call-reason">{t(failedMessageId(call.failReason))}</span>
            {/if}
            <span class="actions" data-testid="call-actions">
              {#each actionsFor(call.state) as action (action)}
                <button type="button" onclick={() => run(action, call)}>{t(ACTION_LABEL[action])}</button>
              {/each}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  {#if detail !== null}
    <section class="detail" data-testid="call-detail">
      <h2>{detail.summary.title}</h2>
      <label class="search">
        {t("calls.search")}
        <input type="search" bind:value={query} />
      </label>
      {#if showNoMatches}
        <p class="no-matches">{t("calls.no_matches")}</p>
      {/if}
      <ol class="lines">
        {#each detail.lines as line, i (i)}
          <li class="line" data-testid="call-line">
            <span class="time" data-testid="line-time">{formatLineTime(line.startMs)}</span>
            <span class="swatch" data-testid="speaker-swatch" style:background-color={speakerColor(line.speaker)}></span>
            <span class="speaker" data-testid="line-speaker">{speakerName(line.speaker)}</span>
            <span class="text" data-testid="line-text"
              >{#each segmentsFor(line.text, query) as segment, j (j)}{#if segment.mark}<mark>{segment.text}</mark
                  >{:else}{segment.text}{/if}{/each}</span
            >
          </li>
        {/each}
      </ol>
    </section>
  {/if}
</main>

<style>
  .calls {
    font:
      14px/20px "Segoe UI",
      system-ui,
      sans-serif;
    padding: 12px 16px;
  }

  .error {
    color: #b00020;
  }

  .list,
  .lines {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 0;
    border-bottom: 1px solid #ddd;
  }

  .row.selected {
    background: #eef4fb;
  }

  .select {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .title {
    flex: 1 1 12em;
    font-weight: 600;
  }

  .date,
  .duration,
  .time {
    font-variant-numeric: tabular-nums;
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 0;
  }

  .swatch {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }

  .speaker {
    font-weight: 600;
  }

  .text {
    white-space: pre-wrap;
  }
</style>
