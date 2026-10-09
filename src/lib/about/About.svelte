<script lang="ts">
  // The "About Voicen" button and its modal dialog (FR-18; spec 006 US1, FR-002,
  // Clarification Q1; contracts/ipc.md "UI component"; T-023). Mounted only by the
  // General tab. docs/decisions/settings-ui.md "A — About".
  //
  // Every build-info text shown is formatBuildInfo of a get_build_info answer obtained
  // for this opening: the previous result is cleared when the dialog opens, and an
  // answer to an earlier opening is dropped. A rejection shows about.build_info_error
  // (role="alert"), never the rejection text. While the call is pending nothing is shown.
  //
  // The <dialog> is moved to <body> (portal), so it is not inside the page's
  // <fieldset disabled={saving}> nor inside <main>, which goes inert under the discard
  // prompt. `open` is bindable so the page can close About when the discard prompt
  // shows (the discard prompt wins, settings-ui D); About sets it false when it is
  // destroyed, so a torn-down About never reopens by itself.
  import { onDestroy } from "svelte";
  import type { Action } from "svelte/action";
  import { formatBuildInfo, loadBuildInfo, type BuildInfo } from "$lib/buildInfo";
  import { t } from "$lib/i18n";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  type Result = { kind: "ok"; info: BuildInfo } | { kind: "error" } | null;

  const titleId = $props.id();
  let dialog = $state<HTMLDialogElement | undefined>();
  let opener = $state<HTMLButtonElement | undefined>();
  let result = $state<Result>(null);
  /** The current opening; an answer to an earlier one is dropped. */
  let opening = 0;

  function openAbout() {
    const current = ++opening;
    result = null;
    open = true;
    loadBuildInfo().then(
      (info) => {
        if (current === opening) result = { kind: "ok", info };
      },
      () => {
        if (current === opening) result = { kind: "error" };
      },
    );
  }

  // Pre-effect: it runs before the page's effects, so when the page closes About for
  // the discard prompt, the modal is gone before the prompt takes focus.
  $effect.pre(() => {
    if (dialog === undefined) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  });

  /** Esc, Close or the page: the dialog closed. Focus goes back to the opener unless it moved elsewhere. */
  function onDialogClose() {
    open = false;
    const active = document.activeElement;
    if (active === null || active === document.body || (dialog?.contains(active) ?? false)) opener?.focus();
  }

  // Teardown (a settings://focus request switches the tab away from General while About
  // is open): removing an open modal fires no `close`, so the bound `open` would stay
  // true and the next instance would show the dialog unasked, with no get_build_info
  // call. About ends with its instance; the next showing comes only from the button,
  // and an answer still pending for this instance is dropped.
  onDestroy(() => {
    opening++;
    open = false;
  });

  /** Moves the node to <body> and removes it on destroy. */
  const portal: Action<HTMLElement> = (node) => {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      },
    };
  };
</script>

<div class="about">
  <button type="button" bind:this={opener} onclick={openAbout}>{t("about.button")}</button>
  <dialog bind:this={dialog} use:portal class="about-dialog" aria-labelledby={titleId} onclose={onDialogClose}>
    <h2 id={titleId}>{t("about.title")}</h2>
    {#if result?.kind === "ok"}
      <p>{formatBuildInfo(result.info)}</p>
    {:else if result?.kind === "error"}
      <p role="alert">{t("about.build_info_error")}</p>
    {/if}
    <p>{t("about.license")}</p>
    <div class="about-actions">
      <button type="button" onclick={() => (open = false)}>{t("about.close")}</button>
    </div>
  </dialog>
</div>

<style>
  .about {
    margin-top: 1rem;
  }

  .about-dialog {
    max-width: 28rem;
    padding: 1rem 1.25rem;
    border: none;
    border-radius: 6px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 25%);
  }

  .about-dialog::backdrop {
    background: rgb(0 0 0 / 35%);
  }

  .about-dialog h2 {
    margin-top: 0;
    font-size: 1.1em;
  }

  .about-actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
