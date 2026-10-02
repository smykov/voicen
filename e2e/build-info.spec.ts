import { expect, test } from "@playwright/test";

// The Tauri IPC is mocked: the web UI runs in Chromium without the Rust side.
test("shows the version and commit reported by the app", async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: async (cmd: string) => {
        if (cmd === "get_build_info") return { version: "0.1.0", commit: "abc1234" };
        throw new Error(`unexpected command ${cmd}`);
      },
      transformCallback: () => 0,
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toHaveText("Voicen 0.1.0 (abc1234)");
});

test("reports a failure to read the build info", async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke: async () => {
        throw new Error("ipc down");
      },
      transformCallback: () => 0,
    };
  });
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("Cannot read build info");
});
