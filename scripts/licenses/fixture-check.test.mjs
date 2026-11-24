// T-181 red tests (T-094 r1#3, I-5): fixture-check.sh must also run cargo-about with the real
// about.toml plus a later top-level `accepted` and require a non-zero exit whose output says
// "duplicate key" (docs/decisions/licenses.md: "cargo-about refuses a later top-level accepted").
//
// fixture-check.sh needs the core image's cargo-about, which a unit test cannot start, so the
// script runs here in a throwaway root with a FAKE `scripts/tw-run` (the one command it calls)
// that behaves like cargo-about 0.9.2 on the GPL fixture and, per FAKE_DUP, on the duplicate
// config: real (fails with "duplicate key"), passes (exit 0: the refusal is gone), other (fails
// without the words), network (a download failure). A config is "duplicate" when it holds two
// top-level `accepted` lines. The script's own wiring (which config it hands over, how it reads
// the exit status and the message) is what is tested.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));

const FAKE_TW_RUN = `#!/bin/sh
echo "$*" >> "$FAKE_DIR/calls"
case "$*" in *"about --version"*) echo "cargo-about 0.9.2"; exit 0 ;; esac
cfg=""; prev=""
for a in "$@"; do [ "$prev" = "-c" ] && cfg="$a"; prev="$a"; done
[ -n "$cfg" ] || { echo "fake tw-run: no -c in: $*" >&2; exit 97; }
n=$(grep -c '^accepted *=' "$cfg")
if [ "$n" -ge 2 ]; then
  cp "$cfg" "$FAKE_DIR/dup-config-seen"
  case "$FAKE_DUP" in
    real) echo "error: failed to parse $cfg: duplicate key \\\`accepted\\\` in document root"; exit 1 ;;
    passes) echo "cargo-about: ok"; exit 0 ;;
    other) echo "error: something else broke"; exit 1 ;;
    network) echo "error: failed to download crates"; exit 1 ;;
  esac
fi
echo "error: failed to satisfy license requirements"
echo "  --> licenses/fixtures/gpl/gpl-fixture/Cargo.toml"
echo "  GPL-3.0-only"
echo "error: unable to synthesize license expression for 'nolicense-fixture 0.1.0': no license"
exit 1
`;

/** @param {import("node:test").TestContext} t @param {string} mode */
function runIn(t, mode) {
  const dir = mkdtempSync(join(tmpdir(), "voicen-fixture-check-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "scripts/licenses"), { recursive: true });
  mkdirSync(join(dir, "fake"), { recursive: true });
  copyFileSync(join(ROOT, "scripts/licenses/fixture-check.sh"), join(dir, "scripts/licenses/fixture-check.sh"));
  copyFileSync(join(ROOT, "about.toml"), join(dir, "about.toml"));
  writeFileSync(join(dir, "about.hbs"), "x");
  writeFileSync(join(dir, "scripts/tw-run"), FAKE_TW_RUN);
  chmodSync(join(dir, "scripts/tw-run"), 0o755);
  const r = spawnSync("bash", [join(dir, "scripts/licenses/fixture-check.sh")], {
    encoding: "utf8",
    cwd: dir,
    env: { ...process.env, FAKE_DIR: join(dir, "fake"), FAKE_DUP: mode },
  });
  const seen = join(dir, "fake/dup-config-seen");
  return { r, dir, seen: existsSync(seen) ? readFileSync(seen, "utf8") : null };
}

const detail = (/** @type {{ r: import("node:child_process").SpawnSyncReturns<string> }} */ x) =>
  `status ${x.r.status}; stdout: ${x.r.stdout.slice(0, 300)}; stderr: ${x.r.stderr.slice(0, 600)}`;

test("fixture-check passes when cargo-about rejects the GPL fixture and a later top-level accepted (duplicate key)", (t) => {
  const x = runIn(t, "real");
  assert.equal(x.r.status, 0, detail(x));
  assert.notEqual(x.seen, null, "cargo-about was run on a config with a second top-level accepted");
});

test("the duplicate config is the real about.toml followed by a later top-level `accepted = [\"MIT\"]`", (t) => {
  const x = runIn(t, "real");
  const real = readFileSync(join(ROOT, "about.toml"), "utf8");
  assert.notEqual(x.seen, null, detail(x));
  assert.ok(x.seen.startsWith(real), "begins with the real about.toml, byte for byte");
  assert.match(x.seen.slice(real.length), /^\s*accepted = \["MIT"\]\s*$/);
  assert.equal(readFileSync(join(x.dir, "about.toml"), "utf8"), real, "the real about.toml is not edited");
});

test("fixture-check fails (1) naming `duplicate key` when cargo-about accepts the second accepted", (t) => {
  const x = runIn(t, "passes");
  assert.equal(x.r.status, 1, detail(x));
  assert.match(x.r.stderr, /duplicate key/);
});

test("fixture-check fails (1) when cargo-about rejects the duplicate config without saying duplicate key", (t) => {
  const x = runIn(t, "other");
  assert.equal(x.r.status, 1, detail(x));
  assert.match(x.r.stderr, /duplicate key/);
});

test("fixture-check exits 3 (cannot run), not 1, when the duplicate run fails on the network", (t) => {
  const x = runIn(t, "network");
  assert.equal(x.r.status, 3, detail(x));
});
