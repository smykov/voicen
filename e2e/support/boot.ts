// Boot fixture (T-050, decisions #77): every spec that uses `page` imports `test` and
// `expect` from here instead of "@playwright/test". While the test runs it records what
// breaks a page's boot:
// - failed requests (method, URL and the browser's errorText, e.g. net::ERR_NETWORK_CHANGED);
// - responses with status >= 500 (status, method, URL);
// - a renderer crash.
// A test during which any of them happened fails in the fixture's teardown with that list,
// even when its own assertions passed, so a page that did not boot is reported with its
// cause and not as an unrelated assertion. No retries: the cause is the report.
import { test as base, expect, type Page } from "@playwright/test";

export const test = base.extend<{ page: Page }>({
  page: async ({ page }, use) => {
    const causes: string[] = [];
    page.on("requestfailed", (req) => {
      causes.push(
        `request failed: ${req.method()} ${req.url()} (${req.failure()?.errorText ?? "no errorText"})`,
      );
    });
    page.on("response", (res) => {
      if (res.status() >= 500) {
        causes.push(`HTTP ${res.status()}: ${res.request().method()} ${res.url()}`);
      }
    });
    page.on("crash", () => {
      causes.push(`renderer crash: the page crashed (last URL ${page.url()})`);
    });

    await use(page);

    if (causes.length > 0) {
      throw new Error(`page did not boot cleanly:\n  - ${causes.join("\n  - ")}`);
    }
  },
});

export { expect };
