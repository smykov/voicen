<script lang="ts">
  import { onMount } from "svelte";
  import { formatBuildInfo, loadBuildInfo } from "$lib/buildInfo";

  let buildInfo = $state("");
  let error = $state("");

  onMount(async () => {
    try {
      buildInfo = formatBuildInfo(await loadBuildInfo());
    } catch (e) {
      error = `Cannot read build info: ${e}`;
    }
  });
</script>

<main class="container">
  <h1>Voicen</h1>
  {#if error}
    <p role="alert">{error}</p>
  {:else}
    <p data-testid="build-info">{buildInfo}</p>
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
