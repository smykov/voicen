// Boot fixture (T-050, decisions #77): every spec that uses `page` or `context` imports
// `test` and `expect` from here instead of "@playwright/test". It overrides the `context`
// fixture (the `page` fixture is derived from it), so it covers every page of the test's
// context: the `page` fixture's page and pages the test opens with `context.newPage()`.
// While the test runs it records what breaks a page's boot:
// - failed requests (method, URL and the browser's errorText, e.g. net::ERR_NETWORK_CHANGED);
//   errorText is never filtered, net::ERR_ABORTED included: a test that aborts a request on
//   purpose must scope that itself;
// - responses with status >= 500 (status, method, URL);
// - a renderer crash of any page.
// A test during which any of them happened fails in the fixture's teardown with that list,
// even when its own assertions passed, so a page that did not boot is reported with its
// cause and not as an unrelated assertion. No retries: the cause is the report.
//
// It also holds the one way a spec reaches a booted page, `bootPage` (T-195, invariant I1:
// no e2e verdict depends on how fast the page boots). Every navigation of a spec, fixture or
// support file goes through it (structural test in e2e/boot-failure.spec.ts); only the
// failure-expecting scenarios of e2e/fixtures/boot-failure/boot-failure.inner.ts navigate by
// themselves, because their verdict is that the boot fails.
import { test as base, expect, type BrowserContext, type Locator, type Page } from "@playwright/test";

/**
 * What tells that a page has booted: a locator that becomes visible, or, for a page that
 * shows nothing until it is driven (the overlay before its first state, a settings page whose
 * settings_get is held), a probe that turns true.
 */
export type BootReady = Locator | (() => Promise<boolean>);

/**
 * Navigates `page` to `url` and waits until `ready` holds. The wait has no expect timeout of
 * its own (`timeout: 0`): it is bounded only by the test's timeout, so a slow first render
 * (context start, chunk fetch, hydration, the mocked IPC under host load) is never judged
 * against the 5 s expect default. What the test checks after the boot keeps its own limits.
 */
export async function bootPage(page: Page, url: string, ready: BootReady): Promise<void> {
  await page.goto(url);
  if (typeof ready === "function") {
    await expect.poll(ready, { message: `the page at ${url} booted`, timeout: 0 }).toBe(true);
  } else {
    await expect(ready, `the page at ${url} booted`).toBeVisible({ timeout: 0 });
  }
}

export const test = base.extend<{ context: BrowserContext }>({
  context: async ({ context }, use) => {
    const causes: string[] = [];
    context.on("requestfailed", (req) => {
      causes.push(
        `request failed: ${req.method()} ${req.url()} (${req.failure()?.errorText ?? "no errorText"})`,
      );
    });
    context.on("response", (res) => {
      if (res.status() >= 500) {
        causes.push(`HTTP ${res.status()}: ${res.request().method()} ${res.url()}`);
      }
    });
    const watchCrash = (page: Page) => {
      page.on("crash", () => {
        causes.push(`renderer crash: the page crashed (last URL ${page.url()})`);
      });
    };
    context.pages().forEach(watchCrash);
    context.on("page", watchCrash);

    await use(context);

    if (causes.length > 0) {
      throw new Error(`page did not boot cleanly:\n  - ${causes.join("\n  - ")}`);
    }
  },
});

export { expect };
