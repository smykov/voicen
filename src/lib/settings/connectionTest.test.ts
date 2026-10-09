// T-013 red tests for the connection-test message mapping (spec 004 US4, FR-017).
//
// API pinned here for src/lib/settings/connectionTest.ts:
//
//   testMessage(result: ConnectionTestResult): { id: MessageId; args: MessageArgs }
//
// One exhaustive switch over the wire `kind` (no default arm): ok ->
// settings.test_connection.ok {ms}; cannot_reach -> failure.cannot_reach {host};
// invalid_key -> failure.invalid_api_key; timeout -> failure.timeout; http ->
// failure.server_error {status}; unexpected_response -> failure.unexpected_response;
// key_store_unavailable -> failure.key_store_unavailable; invalid ->
// settings.test_connection.invalid. Args are strings (MessageArgs).
//
// The results are core's wire (e2e/fixtures/settings-wire.json test_connection_results,
// pinned by core's e2e_settings_wire_fixture_test_connection_results_match_core), so a
// kind core sends that the switch does not know fails here.
import { describe, expect, it } from "vitest";
import en from "$i18n/en.json";
import ru from "$i18n/ru.json";
import wire from "../../../e2e/fixtures/settings-wire.json";
import { text } from "../i18n/render";
import type { Catalog } from "../i18n";
import { testMessage } from "./connectionTest";
import type { ConnectionTestResult } from "./settingsApi";

const catalog: Catalog = { en, ru };

const results = wire.test_connection_results as unknown as Record<string, ConnectionTestResult>;

/** The id and args the analysis fixes per kind (fixture values: 123 ms, 127.0.0.1:1, 500). */
const EXPECTED: Record<string, { id: string; args: Record<string, string>; en: string }> = {
  ok: { id: "settings.test_connection.ok", args: { ms: "123" }, en: "OK, 123 ms" },
  cannot_reach: { id: "failure.cannot_reach", args: { host: "127.0.0.1:1" }, en: "Cannot reach 127.0.0.1:1" },
  invalid_key: { id: "failure.invalid_api_key", args: {}, en: "Invalid API key" },
  timeout: { id: "failure.timeout", args: {}, en: "The server did not answer in time" },
  http: { id: "failure.server_error", args: { status: "500" }, en: "Server error (HTTP 500)" },
  unexpected_response: {
    id: "failure.unexpected_response",
    args: {},
    en: "Unexpected response from the server",
  },
  key_store_unavailable: {
    id: "failure.key_store_unavailable",
    args: {},
    en: "The API key could not be read from Windows Credential Manager.",
  },
  invalid: { id: "settings.test_connection.invalid", args: {}, en: "Check the highlighted fields" },
};

describe("testMessage (T-013)", () => {
  it("the fixture holds exactly the eight kinds the mapping covers, each tagged with its own kind", () => {
    expect(Object.keys(results).sort()).toEqual(Object.keys(EXPECTED).sort());
    for (const [name, result] of Object.entries(results)) expect(result.kind).toBe(name);
  });

  for (const kind of Object.keys(EXPECTED)) {
    it(`${kind} maps to ${EXPECTED[kind].id} with string args, rendered as the en text`, () => {
      const message = testMessage(results[kind]);
      expect(message.id).toBe(EXPECTED[kind].id);
      expect(message.args ?? {}).toEqual(EXPECTED[kind].args);
      for (const value of Object.values(message.args ?? {})) expect(typeof value).toBe("string");
      expect(text(catalog, "en", message.id, message.args)).toBe(EXPECTED[kind].en);
    });
  }

  it("every fixture result renders in en and ru with no raw id and no unfilled placeholder; ru differs from en", () => {
    for (const [kind, result] of Object.entries(results)) {
      const message = testMessage(result);
      for (const lang of ["en", "ru"] as const) {
        const messages = catalog[lang] as Record<string, string>;
        expect(Object.hasOwn(messages, message.id), `${lang} has ${message.id} (${kind})`).toBe(true);
        expect(messages[message.id], `${lang} ${message.id} is not empty`).not.toBe("");
        const rendered = text(catalog, lang, message.id, message.args);
        expect(rendered, `${kind} in ${lang}`).not.toBe(message.id);
        expect(rendered, `${kind} in ${lang}`).not.toMatch(/\{[a-z][a-z0-9_]*\}/);
      }
      expect(text(catalog, "ru", message.id, message.args), `${kind}: ru is translated`).not.toBe(
        text(catalog, "en", message.id, message.args),
      );
    }
  });

  it("the values of the result reach the text: another latency, host and status are rendered as sent", () => {
    // Bite: a constant "123" / host / status in the mapping instead of the result's own value.
    const ok = testMessage({ kind: "ok", latency_ms: 4567 });
    expect(text(catalog, "en", ok.id, ok.args)).toBe("OK, 4567 ms");
    const reach = testMessage({ kind: "cannot_reach", host: "voicen.example.com:8443" });
    expect(text(catalog, "en", reach.id, reach.args)).toBe("Cannot reach voicen.example.com:8443");
    const http = testMessage({ kind: "http", status: 418 });
    expect(text(catalog, "en", http.id, http.args)).toBe("Server error (HTTP 418)");
    const zero = testMessage({ kind: "ok", latency_ms: 0 });
    expect(text(catalog, "en", zero.id, zero.args)).toBe("OK, 0 ms");
  });

  it("an invalid result's message names no field and no code (the fields carry them)", () => {
    const message = testMessage({
      kind: "invalid",
      errors: [
        { field: "engine.api.base_url", code: "url.malformed" },
        { field: "timeouts.connect", code: "timeout.range" },
      ],
    });
    expect(message.id).toBe("settings.test_connection.invalid");
    const rendered = text(catalog, "en", message.id, message.args);
    expect(rendered).not.toContain("engine.api.base_url");
    expect(rendered).not.toContain("url.malformed");
  });
});
