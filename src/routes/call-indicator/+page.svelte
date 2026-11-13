<script lang="ts">
  // The call-indicator window (specs/007-call-mode-record contracts/ipc.md "UI components";
  // FR-005, FR-006, FR-046; T-131 analysis invariants I1 to I5).
  //
  // - It shows only the newest IndicatorState and IndicatorTick the shell sent (I1): no UI
  //   timer (the 10 s reminder is core's), no local filter of warnings (Dismiss only invokes
  //   call_indicator_dismiss; a warning leaves when the next state omits it, and a re-raised
  //   kind shows because the page remembers no dismissed id or kind).
  // - The listeners are registered first, then call_indicator_state is invoked; the reply is
  //   applied only if no indicator://state event arrived since the listener was registered
  //   (every change emits an event, in order, so the last event is at least as new; I3). A
  //   rejected listen or reply shows nothing of its own, never the rejection text.
  // - Texts are the state's (reminder, headphones hint, warnings: rendered by Rust, shown as
  //   given) or the page's own ids rendered with `t` in the state's `lang`, set before the
  //   state is shown (I4); the page reads no settings and no browser language.
  // - Every control is a <button> with a catalog name; the drag region holds none (I5).
  import { onMount } from "svelte";
  import { setLanguage, t } from "$lib/i18n";
  import { formatDuration } from "$lib/calls/view";
  import {
    dismiss,
    indicatorState,
    onIndicatorState,
    onIndicatorTick,
    stop,
    type IndicatorState,
    type IndicatorTick,
  } from "$lib/callIndicator/indicatorApi";

  let shown = $state.raw<IndicatorState | null>(null);
  let tick = $state.raw<IndicatorTick | null>(null);

  const micLevel = $derived(tick?.micLevel ?? 0);
  const systemLevel = $derived(tick?.systemLevel ?? 0);

  /** A level as a bar width in percent (the wire's value stays the aria value). */
  const width = (level: number) => `${Math.min(1, Math.max(0, level)) * 100}%`;

  function show(next: IndicatorState) {
    // The language first, in the same update: the new state never shows in the old one.
    setLanguage(next.lang);
    shown = next;
  }

  // Stop and Dismiss: the shell answers with its next state; a refusal shows nothing of its own.
  function onStop() {
    stop().catch(() => null);
  }

  function onDismiss(warningId: number) {
    dismiss(warningId).catch(() => null);
  }

  onMount(() => {
    let disposed = false;
    /** An indicator://state event arrived since the state listener was registered. */
    let sawEvent = false;
    const unlisteners: (() => void)[] = [];
    (async () => {
      // Only the IPC calls are caught; an exception of the page's own code is not swallowed.
      const stopState = await onIndicatorState((next) => {
        sawEvent = true;
        show(next);
      }).catch(() => null);
      // No state listener: a reply would show a state nothing can update, so stop here.
      if (stopState === null) return;
      unlisteners.push(stopState);
      const stopTick = await onIndicatorTick((next) => (tick = next)).catch(() => null);
      if (stopTick !== null) unlisteners.push(stopTick);
      if (disposed) {
        for (const unlisten of unlisteners.splice(0)) unlisten();
        return;
      }
      // A rejected reply: the listeners stay, so the shell's next change is shown.
      const current = await indicatorState().catch(() => null);
      if (current !== null && !sawEvent && !disposed) show(current);
    })();
    return () => {
      disposed = true;
      for (const unlisten of unlisteners.splice(0)) unlisten();
    };
  });
</script>

{#if shown !== null}
  <div class="indicator">
    <div class="bar" data-tauri-drag-region>
      {#if shown.recording}
        <span class="label">{t("indicator.recording")}</span>
        <span class="timer" data-testid="indicator-timer">{formatDuration(tick?.elapsedMs ?? 0)}</span>
      {/if}
    </div>

    {#if shown.recording}
      <div class="levels">
        <div class="level">
          <span class="level-name" aria-hidden="true">{t("speaker.me")}</span>
          <div
            class="meter"
            role="meter"
            aria-label={t("speaker.me")}
            aria-valuemin={0}
            aria-valuemax={1}
            aria-valuenow={micLevel}
          >
            <div class="fill" style:width={width(micLevel)}></div>
          </div>
        </div>
        <div class="level">
          <span class="level-name" aria-hidden="true">{t("speaker.other")}</span>
          <div
            class="meter"
            role="meter"
            aria-label={t("speaker.other")}
            aria-valuemin={0}
            aria-valuemax={1}
            aria-valuenow={systemLevel}
          >
            <div class="fill" style:width={width(systemLevel)}></div>
          </div>
        </div>
      </div>

      <button type="button" class="stop" onclick={onStop}>{t("indicator.stop")}</button>
    {/if}

    {#if shown.reminderVisible}
      <p class="reminder">{shown.reminderText}</p>
      <p class="hint">{shown.headphonesHint}</p>
    {/if}

    {#if shown.warnings.length > 0}
      <ul class="warnings" data-testid="indicator-warnings">
        {#each shown.warnings as warning (warning.id)}
          <li class="warning" data-kind={warning.kind}>
            <span class="warning-text">{warning.text}</span>
            <button type="button" class="dismiss" onclick={() => onDismiss(warning.id)}>{t("indicator.dismiss")}</button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

<style>
  :global(html),
  :global(body) {
    margin: 0;
    background: rgb(24 24 24);
    color: #f5f5f5;
    font:
      13px/17px "Segoe UI",
      system-ui,
      sans-serif;
  }

  .indicator {
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    font-variant-numeric: tabular-nums;
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 22px;
    cursor: move;
    user-select: none;
  }

  .label {
    color: #ef5350;
    font-weight: 600;
  }

  .levels {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .level {
    display: grid;
    grid-template-columns: 6em 1fr;
    align-items: center;
    gap: 6px;
  }

  .meter {
    height: 6px;
    overflow: hidden;
    border-radius: 3px;
    background: rgb(255 255 255 / 15%);
  }

  .fill {
    height: 100%;
    background: #66bb6a;
  }

  .reminder,
  .hint {
    margin: 0;
  }

  .hint {
    opacity: 0.8;
  }

  .warnings {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .warning {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 4px 6px;
    border-radius: 4px;
    background: rgb(255 179 0 / 18%);
  }

  .warning-text {
    overflow-wrap: anywhere;
  }

  button {
    flex: none;
    padding: 2px 10px;
    border: 1px solid rgb(255 255 255 / 30%);
    border-radius: 4px;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
</style>
