<script lang="ts">
  // The overlay window (spec 001 contracts/ipc.md overlay; T-053 analysis invariants (4)
  // and (5)). Display only: no button, no focusable element, nothing the user can act on;
  // the shell creates, shows and destroys the window.
  //
  // - It shows only what the shell sends: the overlay://state events and the
  //   overlay_ready reply, through `apply` (the payload with the highest seq wins, in
  //   whatever order they arrive). The listener is registered first, then overlay_ready
  //   is invoked, so no change between the two is lost. A rejected listen or reply shows
  //   nothing of its own, never the rejection text: after a rejected listen nothing more
  //   is invoked; after a rejected reply the listener stays, so later events still show.
  // - A message is the text Rust rendered (nested mic_reason.* ids included), shown as
  //   given. The page renders only its own ids, overlay.recording and
  //   overlay.processing, with `t` in the payload's `lang` (the UI derives no language).
  // - No expiry timer: a state changes only with a payload (every expiry is core's). The
  //   one timer redraws a recording's elapsed m:ss and runs only while a recording is
  //   shown.
  import { onMount } from "svelte";
  import { setLanguage, t } from "$lib/i18n";
  import { onOverlayState, overlayReady, type OverlayPayload } from "$lib/overlay/overlayApi";
  import { apply, formatElapsed, stateAt, type Shown } from "$lib/overlay/state";

  /**
   * How often the elapsed m:ss is redrawn while a recording is shown. The shown time
   * lags the real one by at most this (each redraw reads the clock, so it never leads);
   * well under the one second the contract allows.
   */
  const REDRAW_MS = 250;

  /** The page's monotonic clock (ms); the `receivedAt` of `Shown`. */
  const clock = () => performance.now();

  // Raw: `apply` returns the same object for a dropped payload, compared by identity.
  let shown = $state.raw<Shown | null>(null);
  /** The time the view is drawn for: the last arrival or redraw. */
  let now = $state(0);
  const view = $derived(stateAt(shown, now));
  const recording = $derived(shown?.payload.state.kind === "recording");
  /** The text to show, or null when nothing is shown (hidden, or no payload yet). */
  const text = $derived(
    view.kind === "recording"
      ? t("overlay.recording", { elapsed: formatElapsed(view.elapsedMs) })
      : view.kind === "processing"
        ? t("overlay.processing")
        : view.kind === "message"
          ? view.text
          : null,
  );

  function receive(payload: OverlayPayload) {
    const at = clock();
    const next = apply(shown, payload, at);
    if (next === shown) return;
    // The language first, in the same update: the new state never shows in the old one.
    setLanguage(next.payload.lang);
    shown = next;
    now = at;
  }

  $effect(() => {
    if (!recording) return;
    const timer = setInterval(() => (now = clock()), REDRAW_MS);
    return () => clearInterval(timer);
  });

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    (async () => {
      // Only the two IPC calls are caught; nothing is shown of its own (the page shows
      // only what the shell sent). An exception of the page's own code (`receive`) is not
      // swallowed: it surfaces as an unhandled rejection.
      const stop = await onOverlayState(receive).catch(() => null);
      // No listener: a reply would show a state nothing can update, so stop here.
      if (stop === null) return;
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
      // A rejected reply: the listener stays, so the shell's next change is shown.
      const current = await overlayReady().catch(() => null);
      if (current !== null && !disposed) receive(current);
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  });
</script>

{#if text !== null}
  <div class="overlay" data-testid="overlay" data-state={view.kind}>
    {#if view.kind === "recording"}
      <span class="dot" aria-hidden="true"></span>
    {/if}
    <span class="text" data-testid="overlay-text">{text}</span>
  </div>
{/if}

<style>
  /* Look: OQ-10 defaults (a dark pill, a red dot before the recording time, a message on
     at most two lines). The window itself is transparent and sized by the shell. */
  :global(html),
  :global(body) {
    margin: 0;
    overflow: hidden;
    background: transparent;
  }

  .overlay {
    box-sizing: border-box;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 320px;
    max-width: 100vw;
    height: 56px;
    margin: 0 auto;
    padding: 0 20px;
    border-radius: 28px;
    background: rgb(24 24 24 / 85%);
    color: #f5f5f5;
    font:
      14px/18px "Segoe UI",
      system-ui,
      sans-serif;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
    user-select: none;
  }

  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #e53935;
  }

  .text {
    display: -webkit-box;
    overflow: hidden;
    text-align: center;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }
</style>
