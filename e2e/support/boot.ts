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
import { test as base, expect, type BrowserContext, type Page } from "@playwright/test";

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
