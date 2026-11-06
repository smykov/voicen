// T-195 red e2e (invariant I1): no e2e verdict depends on how fast the page boots.
//
// The page's entry chunk (SvelteKit's `_app/immutable/entry/start.*.js`) is held for
// BOOT_DELAY_MS, which is more than Playwright's 5 s expect default and less than the
// 30 s test timeout. A spec that reaches its booted page through the boot helper passes: the
// helper's wait is bounded only by the test's own timeout. A boot wait on the 5 s expect
// default (what every spec's own open*() does today) fails here, on that wait.
//
// Contract for the helper (implementation: e2e/support/boot.ts, written by the developer):
//   bootPage(page: Page, url: string, ready: Locator): Promise<void>
// navigates `page` to `url`, then waits until `ready` is visible, with no expect timeout of
// its own (bounded by the test's timeout only). Every spec routes its navigation through it
// (the structural test in e2e/boot-failure.spec.ts).
//
// Why this is red today for the right reason and not on an import error: until the helper
// exists, the test falls back to the boot every spec does today (navigate, then wait on the
// default 5 s expect). It then fails on the 5 s wait of `ready`, which is the defect. Once
// the helper exists the fallback is never taken (the developer may delete it then).
import type { Locator, Page } from "@playwright/test";
import * as boot from "./support/boot";
import { installTauriMock } from "./support/tauriMock";

const { expect, test } = boot;

const BOOT_DELAY_MS = 6_000;

type BootPage = (page: Page, url: string, ready: Locator) => Promise<void>;

const bootPage: BootPage =
  (boot as unknown as { bootPage?: BootPage }).bootPage ??
  (async (page, url, ready) => {
    // Today's boot: navigation, then a wait on the 5 s expect default.
    const navigate = page.goto.bind(page);
    await navigate(url);
    await expect(ready).toBeVisible();
  });

test("a page whose entry chunk takes 6 s still boots through the boot helper (no boot wait on the 5 s expect default)", async ({
  page,
}) => {
  test.setTimeout(30_000);
  let held = 0;
  await page.route(/\/_app\/immutable\/entry\/start\.[\w-]+\.js$/, async (route) => {
    held += 1;
    await new Promise((resolve) => setTimeout(resolve, BOOT_DELAY_MS));
    await route.continue();
  });
  await installTauriMock(page, {});

  const startedAt = Date.now();
  await bootPage(page, "/settings", page.getByRole("tab", { selected: true }));

  expect(held, "the delayed entry chunk was requested").toBeGreaterThan(0);
  expect(Date.now() - startedAt, "the boot really waited for the delayed chunk").toBeGreaterThanOrEqual(
    BOOT_DELAY_MS - 500,
  );
  await expect(page.getByRole("tab", { selected: true })).toBeVisible();
});
