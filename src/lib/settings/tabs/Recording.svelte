<script lang="ts">
  // Recording tab: the hotkey and the recording mode. The microphone field joins
  // with settings_list_microphones (T-006, decision #38 Q5).
  import { t, type MessageId } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, describedBy } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import HotkeyField from "../HotkeyField.svelte";

  let { draft = $bindable(), warnings }: { draft: Draft; warnings: Record<string, MessageId> } = $props();
  const mode = "recording.mode";
</script>

<HotkeyField bind:draft {warnings} />

<div class="field">
  <label for={controlId(mode)}>{t("settings.field.mode")}</label>
  <select
    id={controlId(mode)}
    data-field={mode}
    bind:value={draft.settings.mode}
    aria-invalid={draft.errors[mode] ? "true" : undefined}
    aria-describedby={describedBy(mode, draft.errors, warnings)}
  >
    <option value="hold">{t("settings.mode.hold")}</option>
    <option value="toggle">{t("settings.mode.toggle")}</option>
  </select>
  <FieldMessage errors={draft.errors} {warnings} field={mode} />
</div>
