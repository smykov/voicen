// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

// The e2e run builds into a directory of its own (T-050, decisions #77): playwright.config.ts
// sets VOICEN_E2E_OUT_DIR for its webServer, so `vite build` / `vite preview` there use
// <dir>/kit as SvelteKit's outDir (what `vite preview` serves) and <dir>/build as the static
// output, and a concurrent `pnpm build` in the tree (licenses-bundle, another gate) cannot
// rewrite what the preview serves. Unset (lint, unit tests, `tauri build`, licenses-bundle):
// SvelteKit's defaults, .svelte-kit and build/.
const e2eOutDir = process.env.VOICEN_E2E_OUT_DIR;
const pages = e2eOutDir ? `${e2eOutDir}/build` : "build";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages,
      assets: pages,
      fallback: "index.html",
    }),
    ...(e2eOutDir ? { outDir: `${e2eOutDir}/kit` } : {}),
    // The message catalog lives at the repo root, shared with voicen-core (decisions #13).
    // The only definition of this alias: Vite, Vitest and tsconfig get it from SvelteKit.
    alias: {
      $i18n: "i18n",
    },
  },
};

export default config;
