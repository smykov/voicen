<script lang="ts">
  // The five dictation timeouts (decisions #97, #99; FR-24), whole seconds, all shown
  // whatever the engine. The range is core's rule only (timeout.range): an entry is sent
  // as typed when it is a whole number, otherwise as 0 (the History tab's entry rule),
  // and core refuses it on save. No client clamp.
  import { untrack } from "svelte";
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "./draft";
  import type { Settings } from "./settingsApi";
  import { controlId, describedBy, hintId } from "./fields";
  import FieldMessage from "./FieldMessage.svelte";

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();

  type Role = "connect" | "api_transcription" | "local_server" | "post_processing" | "builtin_local";
  type Key = keyof Settings["timeouts"];
  const ROLES: readonly Role[] = ["connect", "api_transcription", "local_server", "post_processing", "builtin_local"];
  const key = (role: Role): Key => `${role}_s`;
  const field = (role: Role) => `timeouts.${role}` as const;

  function wholeNumber(entry: string): number {
    const value = Number(entry);
    return entry.trim() !== "" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
  }

  // Each input shows what the user typed; it follows the draft only when the draft's
  // number changes to one the entry does not mean (a view applied, a Saved).
  let entries = $state<Partial<Record<Role, string>>>({});
  $effect.pre(() => {
    const values = ROLES.map((role) => draft.settings.timeouts[key(role)]);
    untrack(() => {
      ROLES.forEach((role, i) => {
        const entry = entries[role];
        if (entry === undefined || wholeNumber(entry) !== values[i]) entries[role] = String(values[i]);
      });
    });
  });

  function onInput(role: Role, event: Event & { currentTarget: HTMLInputElement }) {
    const entry = event.currentTarget.value;
    entries[role] = entry;
    draft.settings.timeouts[key(role)] = wholeNumber(entry);
  }
</script>

<fieldset class="timeouts">
  <legend>{t("settings.timeouts.group")}</legend>
  {#each ROLES as role (role)}
    {@const id = field(role)}
    <div class="field">
      <label for={controlId(id)}>{t(`settings.field_label.${id}`)}</label>
      <input
        id={controlId(id)}
        type="number"
        step="1"
        data-field={id}
        value={entries[role] ?? ""}
        oninput={(event) => onInput(role, event)}
        aria-invalid={draft.errors[id] ? "true" : undefined}
        aria-describedby={describedBy(id, draft.errors, warnings, true)}
      />
      <p class="hint" id={hintId(id)}>{t(`settings.timeouts.${role}.hint`)}</p>
      <FieldMessage errors={draft.errors} {warnings} field={id} />
    </div>
  {/each}
</fieldset>

<style>
  .timeouts legend {
    font-weight: 600;
    margin-bottom: 0.5rem;
  }
</style>
