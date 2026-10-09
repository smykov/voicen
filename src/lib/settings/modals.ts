// The settings page's in-page modals and the discard prompt (docs/decisions/settings-ui.md
// A, M). The discard prompt wins over every modal the page shows: a modal <dialog> left
// open makes the prompt inert. So each component that can open one registers how it is
// dismissed, and the page dismisses all of them before it shows the prompt. A dismiss is
// the modal's own "keep" path (About closes; the delete confirmation keeps the model and
// sends nothing), never its confirm.
import { createContext, onDestroy } from "svelte";

/** The page side: dismiss every registered modal. */
export interface ModalHost {
  dismissAll(): void;
}

const [getRegistry, setRegistry, hasRegistry] = createContext<Set<() => void>>();

/** Called once by the settings page during its initialisation. */
export function provideModalHost(): ModalHost {
  const registry = setRegistry(new Set());
  return {
    dismissAll() {
      for (const dismiss of [...registry]) dismiss();
    },
  };
}

/**
 * Called during a component's initialisation: `dismiss` runs whenever the page is about
 * to show the discard prompt, until the component is destroyed. Outside a settings page
 * (no host) nothing is registered.
 */
export function dismissOnDiscardPrompt(dismiss: () => void): void {
  if (!hasRegistry()) return;
  const registry = getRegistry();
  registry.add(dismiss);
  onDestroy(() => registry.delete(dismiss));
}
