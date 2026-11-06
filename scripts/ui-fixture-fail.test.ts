// T-195 fixture (not a test of anything): one test that fails only when VOICEN_FIXTURE_FAIL=1,
// so scripts/ui-test-select.test.ts can run a unit group that exits non-zero through
// scripts/ui-unit-test.mjs and scripts/ui-test-select.mjs. It passes in every normal run (the
// gate's `pnpm test` stays green). A plain `it` with a runtime check: `vitest list` collects
// statically and does not see `it.skipIf(...)`.
import { expect, it } from "vitest";

it("fails on purpose when VOICEN_FIXTURE_FAIL=1", () => {
  if (process.env.VOICEN_FIXTURE_FAIL !== "1") return;
  expect(1).toBe(2);
});
