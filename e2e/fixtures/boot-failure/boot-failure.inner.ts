// Inner scenarios of e2e/boot-failure.spec.ts (T-050, failure branch). Each one is a
// spec as the suite writes them: `test`/`expect` from the boot fixture
// (e2e/support/boot.ts), the Tauri IPC mocked, one goto. The outer test runs this file
// through ./playwright.config.ts and checks how each scenario is reported.
//
// Every request this file breaks on purpose is recorded as an annotation
// { type: "broken-request", description: <url> }, so the outer test can demand that the
// failure names exactly that URL. The assertions after the goto are deliberately
// unrelated to the boot (that is how the flake used to be reported).
import type { Page, Route } from "@playwright/test";
import { expect, test } from "../../support/boot";
import { installTauriMock } from "../../support/tauriMock";

// A route node of a page (nodes/2.*.js and up): not the root layout (0) or the error
// page (1), so SvelteKit's client still renders its own "500 / Internal Error" page.
function isPageNode(url: URL): boolean {
  return /\/_app\/immutable\/nodes\/([2-9]|\d{2,})\.[\w-]+\.js$/.test(url.pathname);
}

async function breakPageNodes(page: Page, how: (route: Route) => Promise<void>): Promise<void> {
  await page.route(isPageNode, async (route) => {
    test.info().annotations.push({ type: "broken-request", description: route.request().url() });
    await how(route);
  });
}

test.beforeEach(async ({ page }) => {
  await installTauriMock(page, { buildInfo: { version: "0.1.0", commit: "abc1234" } });
});

test("healthy boot passes", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toHaveText("Voicen 0.1.0 (abc1234)");
});

test("aborted page chunk, unrelated assertion", async ({ page }) => {
  await breakPageNodes(page, (route) => route.abort("internetdisconnected"));
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toBeVisible({ timeout: 5_000 });
});

test("page chunk answers 500, unrelated assertion", async ({ page }) => {
  await breakPageNodes(page, (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "boom" }),
  );
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toBeVisible({ timeout: 5_000 });
});

test("aborted page chunk, assertions that pass anyway", async ({ page }) => {
  await breakPageNodes(page, (route) => route.abort("internetdisconnected"));
  await page.goto("/");
  await expect(page).toHaveURL(/\/$/);
});

test("renderer crash, unrelated assertion", async ({ page }) => {
  await page.goto("/");
  // chrome://crash is refused as a navigation (net::ERR_ABORTED); CDP crashes the renderer.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Page.crash").catch(() => undefined);
  expect(1 + 1).toBe(2);
  expect("unrelated").toBe("assertion");
});

// Pages a test opens itself with `context.newPage()` (tauri-mock.spec.ts does this in 12
// tests, T-050 review 1 #1) boot like the `page` fixture's page and must be reported the
// same way: the boot fixture covers every page of the test's context, whatever its origin.
// The test takes `context`, not `page`, exactly as those specs do.

test("context.newPage: aborted page chunk, unrelated assertion", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { buildInfo: { version: "0.1.0", commit: "abc1234" } });
  await breakPageNodes(page, (route) => route.abort("internetdisconnected"));
  await page.goto("/");
  await expect(page.getByTestId("build-info")).toBeVisible({ timeout: 5_000 });
});

test("context.newPage: page chunk answers 500, assertions that pass anyway", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { buildInfo: { version: "0.1.0", commit: "abc1234" } });
  await breakPageNodes(page, (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "boom" }),
  );
  await page.goto("/");
  await expect(page).toHaveURL(/\/$/);
});

test("context.newPage: renderer crash, unrelated assertion", async ({ context }) => {
  const page = await context.newPage();
  await installTauriMock(page, { buildInfo: { version: "0.1.0", commit: "abc1234" } });
  await page.goto("/");
  const cdp = await context.newCDPSession(page);
  await cdp.send("Page.crash").catch(() => undefined);
  expect("unrelated").toBe("assertion");
});
