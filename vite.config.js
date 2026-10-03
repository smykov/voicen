import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
// @ts-expect-error type error without @types/node package
import { mkdirSync, writeFileSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import { join } from "node:path";
import { packagesFromModuleIds, readPackageLicense } from "./scripts/licenses/bundle.mjs";
const host = process.env.TAURI_DEV_HOST;

/**
 * Records the npm packages whose modules are in the client bundle (T-027) in
 * target/licenses/npm-bundled.json, which `make licenses-check` checks against about.toml
 * and `make licenses` lists in THIRD-PARTY-NOTICES.txt. Only the client build ships:
 * SvelteKit's server build (used for prerendering) is skipped.
 * @returns {import("vite").Plugin}
 */
function bundledPackages() {
  let root = "";
  let client = false;
  return {
    name: "voicen-bundled-packages",
    apply: "build",
    configResolved(config) {
      root = config.root;
      client = !config.build.ssr;
    },
    generateBundle(_options, bundle) {
      if (!client) return;
      const ids = Object.values(bundle).flatMap((out) => (out.type === "chunk" ? out.moduleIds : []));
      const packages = packagesFromModuleIds(ids).map(({ dir }) => {
        const { name, version } = readPackageLicense(dir);
        return { name, version, dir };
      });
      const out = join(root, "target", "licenses");
      mkdirSync(out, { recursive: true });
      writeFileSync(join(out, "npm-bundled.json"), `${JSON.stringify(packages, null, 2)}\n`);
    },
  };
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [sveltekit(), bundledPackages()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
