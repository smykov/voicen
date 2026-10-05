// Global teardown of playwright.config.ts (T-050): removes the run-private build directory
// the runner created for its webServer. A directory given from outside (VOICEN_E2E_OUT_DIR
// set before the run) is not this run's and stays.
import { rmSync } from "node:fs";

export default function globalTeardown(): void {
  const dir = process.env.VOICEN_E2E_OUT_DIR;
  if (dir && process.env.VOICEN_E2E_OUT_DIR_OWNER === String(process.pid)) {
    rmSync(dir, { recursive: true, force: true });
  }
}
