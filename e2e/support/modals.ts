// Shared check for decision A (docs/decisions/settings-ui.md, "the discard prompt wins"):
// a close request with a dirty draft shows the discard prompt, and no native modal <dialog>
// may be left open behind it (an open modal, even hidden by CSS, makes the rest inert).
// Every "discard prompt wins" case calls `expectNoModalBehindPrompt`; a task that adds a
// new modal adds such a case (T-178 line 6).
import { expect, type Page } from "@playwright/test";

/** How many elements are `:modal` (an open modal dialog, shown or hidden by CSS). */
export async function modalCount(page: Page): Promise<number> {
  return page.evaluate(() => document.querySelectorAll(":modal").length);
}

/** After a close request with a dirty draft and the prompt shown: no element is `:modal`. */
export async function expectNoModalBehindPrompt(page: Page): Promise<void> {
  await expect.poll(() => modalCount(page), { message: "no modal dialog is left open behind the discard prompt" }).toBe(0);
}
