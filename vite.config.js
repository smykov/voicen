import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
// @ts-expect-error type error without @types/node package
import { mkdirSync, writeFileSync } from "node:fs";
// @ts-expect-error type error without @types/node package
import { join } from "node:path";
import { classifyBundle, readPackageLicense } from "./scripts/licenses/bundle.mjs";
const host = process.env.TAURI_DEV_HOST;

/**
 * Records what the client bundle ships (T-027) in target/licenses/npm-bundled.json, which
 * `make licenses-check` checks against about.toml and `make licenses` lists in
 * THIRD-PARTY-NOTICES.txt: one entry per npm package with a module or an emitted asset in
 * the bundle (Vite's and rolldown's own helper modules count as vite and rolldown), and one
 * "unattributed" entry per module or asset that is neither a package file nor project
 * source, which fails the check (fail closed for what this hook sees). Assets without an
 * original file (SvelteKit's _app/version.json) pass on an assumption: today they are
 * generated from build data and carry no third-party code. This generateBundle hook never
 * sees files written in writeBundle/closeBundle, assets of plugins whose generateBundle runs
 * later, code added in renderChunk/banner/intro/footer, or static/ copies; adopting a Vite
 * plugin that emits files or injects code is a license-review point
 * (docs/decisions/licenses.md).
 * Only the client build ships: SvelteKit's server build (used for prerendering) is skipped.
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
      const outputs = Object.values(bundle);
      const { packages, unattributed } = classifyBundle({
        root,
        moduleIds: outputs.flatMap((out) => (out.type === "chunk" ? out.moduleIds : [])),
        assetFiles: outputs.flatMap((out) => (out.type === "asset" ? (out.originalFileNames ?? []) : [])),
      });
      const list = [
        ...packages.map(({ dir }) => {
          const { name, version } = readPackageLicense(dir);
          return { name, version, dir };
        }),
        ...unattributed.map(({ kind, id }) => ({ unattributed: id, kind })),
      ];
      const out = join(root, "target", "licenses");
      mkdirSync(out, { recursive: true });
      writeFileSync(join(out, "npm-bundled.json"), `${JSON.stringify(list, null, 2)}\n`);
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
