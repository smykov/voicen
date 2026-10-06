// T-064 probe (decisions #77, #89): reports the network namespace the e2e run's Playwright
// worker (and so the Chromium it launches) runs in. Driven by scripts/ci/e2e-net-churn.sh,
// which compares it with the host's: no e2e browser may share the host's network namespace.
//
// Opt-in by a file, not by an environment variable: the e2e entry point runs Playwright in
// the ui container through scripts/tw-run, which passes no environment of the caller. The
// repository is mounted into the container, so a file in the tree reaches the run either way.
// Without target/e2e-netns-probe/enabled the test is skipped and writes nothing.
//
// Each run of the test writes one file target/e2e-netns-probe/netns-<pid>-<repeat>.txt with
// the target of /proc/self/ns/net (e.g. "net:[4026531840]"). Workers are children of the
// runner and launch the browser, so all three share this namespace.
import { existsSync, readlinkSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { test } from "@playwright/test";

const PROBE_DIR = resolve("target/e2e-netns-probe");

test("netns probe: report the network namespace of the e2e run", async () => {
  test.skip(!existsSync(join(PROBE_DIR, "enabled")), "netns probe not requested");
  const info = test.info();
  writeFileSync(
    join(PROBE_DIR, `netns-${process.pid}-${info.repeatEachIndex}-${info.retry}.txt`),
    `${readlinkSync("/proc/self/ns/net")}\n`,
  );
});
