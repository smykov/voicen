import { expect, test } from "@playwright/test";
import { calls, installTauriMock } from "./support/tauriMock";

// The Tauri IPC is mocked (e2e/support/tauriMock.ts): the web UI runs in Chromium
// without the Rust side.
test("shows the version and commit reported by the app", async ({ page }) => {
  await installTauriMock(page, { buildInfo: { version: "0.1.0", commit: "abc1234" } });
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toHaveText("Voicen 0.1.0 (abc1234)");
  expect((await calls(page, "get_build_info")).length).toBe(1);
});

test("reports a failure to read the build info", async ({ page }) => {
  await installTauriMock(page, { buildInfo: { reject: "ipc down" } });
  await page.goto("/");
  await expect(page.getByRole("alert")).toContainText("Cannot read build info");
});
