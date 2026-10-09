// The text of a connection-test result (spec 004 US4, FR-017; T-013). The wire result
// carries a kind and no message id (T-046), so this is the one kind -> MessageId mapping:
// an exhaustive switch with literal ids, type-checked by svelte-check (no `as MessageId`
// cast, no default arm: a kind added to `ConnectionTestResult` fails the check until it
// has a case). Its completeness over core's kinds is checked by connectionTest.test.ts
// over e2e/fixtures/settings-wire.json, pinned by core.
import type { MessageArgs, MessageId } from "../i18n";
import type { ConnectionTestResult } from "./settingsApi";

/** A catalog id and its (string) args, rendered with `t`. */
export interface TestMessage {
  id: MessageId;
  args: MessageArgs;
}

/** The message of `result`; `invalid` names no field (the fields carry their errors). */
export function testMessage(result: ConnectionTestResult): TestMessage {
  switch (result.kind) {
    case "ok":
      return { id: "settings.test_connection.ok", args: { ms: String(result.latency_ms) } };
    case "cannot_reach":
      return { id: "failure.cannot_reach", args: { host: result.host } };
    case "invalid_key":
      return { id: "failure.invalid_api_key", args: {} };
    case "timeout":
      return { id: "failure.timeout", args: {} };
    case "http":
      return { id: "failure.server_error", args: { status: String(result.status) } };
    case "unexpected_response":
      return { id: "failure.unexpected_response", args: {} };
    case "key_store_unavailable":
      return { id: "failure.key_store_unavailable", args: {} };
    case "invalid":
      return { id: "settings.test_connection.invalid", args: {} };
  }
}

/**
 * What the page shows for the last connection test it ran: the message, and for an
 * `invalid` result the fields it named (each also highlighted on its control through
 * `applyTestErrors`), so the ones with no control on the page can be listed by label (L).
 */
export interface TestOutcome {
  message: TestMessage;
  fields: readonly string[];
}

/** The outcome of `result`. */
export function testOutcome(result: ConnectionTestResult): TestOutcome {
  const fields = result.kind === "invalid" ? [...new Set(result.errors.map((error) => error.field))] : [];
  return { message: testMessage(result), fields };
}

/** The outcome of a rejected invoke: `error.ipc_unavailable`, never the rejection text. */
export const TEST_IPC_FAILED: TestOutcome = { message: { id: "error.ipc_unavailable", args: {} }, fields: [] };
