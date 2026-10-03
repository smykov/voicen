<script lang="ts">
  import { onMount } from "svelte";
  import { formatBuildInfo, loadBuildInfo, type BuildInfo } from "$lib/buildInfo";
  import { t } from "$lib/i18n";

  // Texts are rendered in the template, so they follow a change of the UI language.
  let buildInfo = $state<BuildInfo | null>(null);
  let failure = $state<string | null>(null);

  onMount(async () => {
    try {
      buildInfo = await loadBuildInfo();
    } catch (e) {
      failure = String(e);
    }
  });
</script>

<main class="container">
  <h1>Voicen</h1>
  {#if failure !== null}
    <p role="alert">{t("app.build_info_error", { reason: failure })}</p>
  {:else}
    <p data-testid="build-info">{buildInfo ? formatBuildInfo(buildInfo) : ""}</p>
  {/if}
</main>

<style>
  :root {
    font-family: Inter, Avenir, Helvetica, Arial, sans-serif;
    color: #0f0f0f;
    background-color: #f6f6f6;
  }

  .container {
    padding-top: 10vh;
    text-align: center;
  }
</style>
