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
// Red before the helper existed: the spec then fell back to the boot every spec did
// (navigate, then a wait on the default 5 s expect) and failed on that wait (T-195 red runs).
import { bootPage, expect, test } from "./support/boot";
import { installTauriMock } from "./support/tauriMock";

const BOOT_DELAY_MS = 6_000;

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
