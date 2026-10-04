<script lang="ts">
  // The built-in engine's part of the Engine tab (spec 002 US1, T-045; settings-ui.md › M):
  // the model select and the five catalog models with Download / Cancel / Retry.
  //
  // - Rows (I2): only `local_models_list` and the `local-model://` events set them,
  //   through the sequencer in models.ts. Both listeners are registered before the first
  //   list; a list is issued on mount and again after every download invoke settles.
  //   Nothing here sets a state of its own (no optimistic `downloading`).
  // - The select (I3): `bind:value` on the draft's model_id; its options are exactly the
  //   downloaded rows. Options appearing or vanishing never write to the draft (Svelte
  //   writes a select back only on the user's change), so a saved model that is not
  //   downloaded stays in the draft and core refuses the save on the field (U2).
  // - Download / Retry are disabled while a row downloads or a download invoke is
  //   pending (I4). The invoke counts as pending until the re-list issued after it has
  //   settled, failed re-list included: core is already downloading when the command
  //   returns, and only that re-list shows it.
  // - Texts (I5): names by nameKey, reasons by messageKey with `reasonArgs`, labels by
  //   UI-only ids; every byte count through `formatSize`. A refused download, a rejection
  //   that is not a FailureReason and a rejected list are shown in the section; a
  //   rejection's own text never is. A failed row's reason is rendered inside a
  //   polite live region that stays mounted with the row, so a row turning failed
  //   changes the text of an existing region (announced), not a new one.
  import { onMount } from "svelte";
  import { t, type MessageId } from "$lib/i18n";
  import { currentLanguage } from "$lib/i18n/language.svelte";
  import type { Draft } from "$lib/settings/draft";
  import { controlId, describedBy } from "$lib/settings/fields";
  import FieldMessage from "$lib/settings/FieldMessage.svelte";
  import {
    asFailureReason,
    cancelLocalModelDownload,
    downloadLocalModel,
    listLocalModels,
    onLocalModelProgress,
    onLocalModelState,
    type FailureReason,
    type LocalModelView,
    type ModelId,
  } from "./localModelsApi";
  import {
    downloadBlocked,
    emptyModels,
    eventReceived,
    formatSize,
    listFailed,
    listReceived,
    listRequested,
    reasonArgs,
    selectable,
    type ModelsState,
  } from "./models";

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();

  const FIELD = "engine.builtin_local.model_id";

  let models = $state.raw<ModelsState>(emptyModels());
  let downloadPending = $state(false);
  /**
   * The newest list request (or a listener) failed: `error.ipc_unavailable`. No rows
   * until the first list succeeds; a failed re-list keeps the rows (they still take events).
   */
  let ipcListFailed = $state(false);
  /** The last download invoke's rejection: a contract refusal, or any other failure (`ipc`). */
  let refusal = $state.raw<{ kind: "refused"; reason: FailureReason } | { kind: "ipc" } | null>(null);

  const rows = $derived<readonly LocalModelView[]>(models.rows ?? []);
  const choices = $derived(selectable(rows));
  const blocked = $derived(downloadBlocked(rows, downloadPending));

  async function refresh(): Promise<void> {
    const issued = listRequested(models);
    models = issued.state;
    try {
      const listed = await listLocalModels();
      const before = models;
      models = listReceived(models, issued.request, listed);
      if (models !== before) ipcListFailed = false;
    } catch {
      const failed = listFailed(models, issued.request);
      models = failed.state;
      if (failed.newest) ipcListFailed = true;
    }
  }

  async function download(id: ModelId): Promise<void> {
    if (blocked) return;
    refusal = null;
    downloadPending = true;
    try {
      try {
        await downloadLocalModel(id);
      } catch (error) {
        const reason = asFailureReason(error);
        refusal = reason === null ? { kind: "ipc" } : { kind: "refused", reason };
      }
      // The command emits nothing: the list shows whether the download started. Until
      // it has settled the invoke is still pending (I4); refresh never rejects.
      await refresh();
    } finally {
      downloadPending = false;
    }
  }

  async function cancel(id: ModelId): Promise<void> {
    refusal = null;
    try {
      // The end state arrives as local-model://state from the download thread.
      await cancelLocalModelDownload(id);
    } catch {
      refusal = { kind: "ipc" };
    }
  }

  function percent(received: number, total: number): string {
    const whole = total > 0 ? Math.min(100, Math.floor((received * 100) / total)) : 0;
    return new Intl.NumberFormat(currentLanguage(), { style: "percent", maximumFractionDigits: 0 }).format(whole / 100);
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
        // Both listeners before the first list: an event in between is not lost (I2).
        keep(await onLocalModelProgress((payload) => (models = eventReceived(models, { event: "progress", payload }))));
        keep(await onLocalModelState((payload) => (models = eventReceived(models, { event: "state", payload }))));
      } catch {
        ipcListFailed = true;
        return;
      }
      if (!disposed) await refresh();
    })();
    return () => {
      disposed = true;
      for (const stop of unlisteners) stop();
    };
  });
</script>

<section class="local-models" data-testid="local-models" aria-labelledby="local-models-heading">
  <div class="field">
    <label for={controlId(FIELD)}>{t("settings.field_label.engine.builtin_local.model_id")}</label>
    <select
      id={controlId(FIELD)}
      data-field={FIELD}
      bind:value={draft.settings.builtin_local.model_id}
      aria-invalid={draft.errors[FIELD] ? "true" : undefined}
      aria-describedby={describedBy(FIELD, draft.errors, warnings)}
    >
      <option value={null} disabled
        >{t(choices.length > 0 ? "local_models.choose_model" : "local_models.no_model_downloaded")}</option
      >
      {#each choices as choice (choice.id)}
        <option value={choice.id}>{t(choice.nameKey)}</option>
      {/each}
    </select>
    <FieldMessage errors={draft.errors} {warnings} field={FIELD} />
  </div>

  <h2 id="local-models-heading">{t("local_models.heading")}</h2>

  {#if ipcListFailed || refusal !== null}
    <div class="message error" role="alert">
      {#if ipcListFailed || refusal?.kind === "ipc"}
        <p>{t("error.ipc_unavailable")}</p>
      {/if}
      {#if refusal?.kind === "refused"}
        <p>{t(refusal.reason.messageKey, reasonArgs(refusal.reason, currentLanguage()))}</p>
      {/if}
    </div>
  {/if}

  {#if models.rows !== null}
    <ul class="models">
      {#each models.rows as row (row.id)}
        <li class="model" data-testid={`local-model-${row.id}`} data-state={row.state.kind}>
          <div class="model-head">
            <span class="model-name">{t(row.nameKey)}</span>
            <span class="model-size">{formatSize(row.sizeBytes, currentLanguage())}</span>
            {#if row.recommended}
              <span class="badge" data-testid="local-model-recommended">{t("local_models.recommended")}</span>
            {/if}
          </div>

          {#if row.state.kind === "downloading"}
            {@const progress = row.state}
            <progress
              max={progress.total}
              value={progress.received}
              aria-label={t("local_models.progress_label", { name: t(row.nameKey) })}
            ></progress>
            <span class="model-progress">
              {t("local_models.progress", {
                received: formatSize(progress.received, currentLanguage()),
                total: formatSize(progress.total, currentLanguage()),
                percent: percent(progress.received, progress.total),
              })}
            </span>
            <button type="button" data-testid="local-model-cancel" onclick={() => cancel(row.id)}
              >{t("local_models.cancel")}</button
            >
          {:else if row.state.kind === "failed"}
            <button type="button" data-testid="local-model-retry" disabled={blocked} onclick={() => download(row.id)}
              >{t("local_models.retry")}</button
            >
          {:else if row.state.kind === "not_downloaded"}
            <button type="button" data-testid="local-model-download" disabled={blocked} onclick={() => download(row.id)}
              >{t("local_models.download")}</button
            >
          {:else}
            <span class="model-downloaded">{t("local_models.downloaded")}</span>
          {/if}

          <!-- Mounted with the row, so a reason appearing is a change inside an existing
               polite region. Not an alert: it arrives asynchronously. -->
          <div class="model-status" aria-live="polite">
            {#if row.state.kind === "failed"}
              <p class="model-reason" data-testid="local-model-reason">
                {t(row.state.reason.messageKey, reasonArgs(row.state.reason, currentLanguage()))}
              </p>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .local-models h2 {
    margin: 1rem 0 0.5rem;
    font-size: 1em;
  }

  .models {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .model {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid #ddd;
  }

  .model-head {
    display: flex;
    flex: 1 1 100%;
    order: -2;
    align-items: baseline;
    gap: 0.5rem;
  }

  .model-name {
    font-weight: 600;
  }

  .model-size,
  .model-progress,
  .model-downloaded {
    color: #555;
    font-size: 0.9em;
  }

  .badge {
    padding: 0 0.4rem;
    border-radius: 3px;
    background: #e3eefc;
    color: #1a5fb4;
    font-size: 0.8em;
  }

  .model-status {
    /* Shown under the head, above the row's buttons. */
    flex: 1 1 100%;
    order: -1;
  }

  .model-status:empty {
    /* An empty region takes no space; cancel the row gap it would add. */
    margin-top: -0.5rem;
  }

  .model-reason {
    margin: 0;
    color: #b00020;
    font-size: 0.9em;
  }

  .message {
    padding: 0.5rem 0.75rem;
    border-radius: 4px;
  }

  .message.error {
    background: #fde7ea;
  }

  .message p {
    margin: 0;
  }
</style>
