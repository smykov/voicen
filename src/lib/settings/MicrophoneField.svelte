<script lang="ts">
  // The microphone select of the Recording tab (T-012; settings-ui.md › P), fed by
  // `settings_list_microphones` once per mount (a remount re-lists, as M does).
  //
  // - Options: "" = System default (microphone null), then the listed devices in list
  //   order by id, the one with `is_default` marked. A device the draft or the saved
  //   settings name that the list does not have is one more option by its id: "not
  //   connected" once the list is known, its saved name while the list is pending or
  //   could not be read. Matching is by the whole id; the name is for display.
  // - The draft (R): a function binding reads `microphone?.id ?? ""` (never undefined,
  //   so mounting writes nothing) and writes only on the user's change: "" -> null, a
  //   listed device -> `{ id, listed name }`, an unlisted one -> the `{ id, name }` it
  //   came from. Options arriving later never write the draft.
  // - A rejected list shows `error.ipc_unavailable` in the field, never the rejection.
  import { onMount } from "svelte";
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "./draft";
  import { controlId, describedBy } from "./fields";
  import FieldMessage from "./FieldMessage.svelte";
  import { listMicrophones, type InputDevice, type Settings } from "./settingsApi";

  const field = "recording.microphone";
  type Microphone = NonNullable<Settings["microphone"]>;

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();

  /** The listed devices; null until the list arrives (or when it could not be read). */
  let devices = $state.raw<readonly InputDevice[] | null>(null);
  let listFailed = $state(false);

  /** Saved or drafted devices the list does not have (unique by id; draft first). */
  const unlisted = $derived.by<Microphone[]>(() => {
    const out: Microphone[] = [];
    for (const mic of [draft.settings.microphone, draft.baseline.settings.microphone]) {
      if (mic === null || out.some((m) => m.id === mic.id)) continue;
      if (devices?.some((d) => d.id === mic.id)) continue;
      out.push({ id: mic.id, name: mic.name });
    }
    return out;
  });

  function selected(): string {
    return draft.settings.microphone?.id ?? "";
  }

  function choose(value: string): void {
    if (value === "") {
      draft.settings.microphone = null;
      return;
    }
    const listed = devices?.find((d) => d.id === value);
    const mic = listed ? { id: listed.id, name: listed.name } : unlisted.find((m) => m.id === value);
    if (mic) draft.settings.microphone = { id: mic.id, name: mic.name };
  }

  onMount(() => {
    let disposed = false;
    listMicrophones().then(
      (list) => {
        if (disposed) return;
        devices = list;
        listFailed = false;
      },
      () => {
        if (!disposed) listFailed = true;
      },
    );
    return () => {
      disposed = true;
    };
  });
</script>

<div class="field">
  <label for={controlId(field)}>{t("settings.field_label.recording.microphone")}</label>
  <select
    id={controlId(field)}
    data-field={field}
    bind:value={selected, choose}
    aria-invalid={draft.errors[field] ? "true" : undefined}
    aria-describedby={describedBy(field, draft.errors, warnings)}
  >
    <option value="">{t("settings.microphone.system_default")}</option>
    {#each devices ?? [] as device (device.id)}
      <option value={device.id}
        >{device.is_default ? t("settings.microphone.windows_default", { name: device.name }) : device.name}</option
      >
    {/each}
    {#each unlisted as mic (mic.id)}
      <option value={mic.id}
        >{devices === null ? mic.name : t("settings.microphone.not_connected", { name: mic.name })}</option
      >
    {/each}
  </select>
  {#if listFailed}
    <p class="list-error" role="alert">{t("error.ipc_unavailable")}</p>
  {/if}
  <FieldMessage errors={draft.errors} {warnings} field={field} />
</div>

<style>
  .list-error {
    margin: 0.25rem 0 0;
    color: #b00020;
    font-size: 0.9em;
  }
</style>
