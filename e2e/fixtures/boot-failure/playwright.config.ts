// Inner Playwright config of e2e/boot-failure.spec.ts (T-050): runs only the
// *.inner.ts specs of this directory against the outer run's preview (BASE_URL), with no
// web server of its own, no retries and a JSON report the outer test reads.
// The gate's config never matches *.inner.ts, so these scenarios run only from the outer test.
import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.BASE_URL;
if (!baseURL) throw new Error("boot-failure inner config: BASE_URL is required");
const report = process.env.VOICEN_BOOT_INNER_REPORT;
if (!report) throw new Error("boot-failure inner config: VOICEN_BOOT_INNER_REPORT is required");

export default defineConfig({
  testDir: ".",
  testMatch: "*.inner.ts",
  outputDir: process.env.VOICEN_BOOT_INNER_OUT ?? "../../../test-results/boot-failure-inner",
  retries: 0,
  workers: 1,
  reporter: [["json", { outputFile: report }]],
  use: { baseURL, trace: "off" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
