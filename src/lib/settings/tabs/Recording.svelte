<script lang="ts">
  // Recording tab: the hotkey and the recording mode. The microphone field joins
  // with settings_list_microphones (T-006, decision #38 Q5).
  import { t } from "$lib/i18n";
  import type { Draft } from "../draft";
  import { controlId, errorId } from "../fields";
  import FieldMessage from "../FieldMessage.svelte";
  import HotkeyField from "../HotkeyField.svelte";

  let { draft = $bindable() }: { draft: Draft } = $props();
  const mode = "recording.mode";
</script>

<HotkeyField bind:draft />

<div class="field">
  <label for={controlId(mode)}>{t("settings.field.mode")}</label>
  <select
    id={controlId(mode)}
    data-field={mode}
    bind:value={draft.settings.mode}
    aria-invalid={draft.errors[mode] ? "true" : undefined}
    aria-describedby={draft.errors[mode] ? errorId(mode) : undefined}
  >
    <option value="hold">{t("settings.mode.hold")}</option>
    <option value="toggle">{t("settings.mode.toggle")}</option>
  </select>
  <FieldMessage errors={draft.errors} field={mode} />
</div>
