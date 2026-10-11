#!/usr/bin/env bash
# T-035/T-038/T-092 guard (class ci-toolchain F-002, guard-model F-003/F-015, P-020;
# docs/decisions/ci-toolchain.md). src-tauri/build.rs gives the Windows link configuration of the
# release exe (the Common-Controls manifest) to the bin and, through rustc-link-arg-tests, to every
# kind-test target (src-tauri/tests/*.rs). No other test exe of the voicen package may hold shell
# code: not the lib or bin unit-test exe, not a lib doctest, example or bench.
#
# The guard reads no Rust, TOML or workflow text (T-092: a macro-built #[cfg(test)] #[test] and a
# quoted `cargo test --d"oc"` passed the old lexer). It decides on two things only:
#   1. cargo's own target list: `cargo metadata --no-deps --offline --format-version 1` for
#      <shell-dir>/Cargo.toml, run in the core image (scripts/tw-run core). The voicen package's
#      targets, without kind-test rows, projected one per line as
#        <kinds,comma-joined> <name> <src_path relative to the shell dir> test=<b> doctest=<b>
#      and sorted, must equal scripts/ci/shell-targets.txt byte for byte. So cargo itself reports
#      test = false and doctest = false for lib and bin, the crate roots are the owned files, and
#      any other target (example, bench, a second bin, a missing build.rs) is a difference.
#   2. line 1 of every lib and bin crate root cargo reports, byte for byte (no trailing \r):
#      lib `#![cfg(not(any(test, doctest)))]`, bin `#![cfg(not(test))]`. rustc applies a
#      crate-level cfg before name resolution, so under --cfg test (cargo test --lib, --bins,
#      --all-targets, any spelling) and doctest (cargo test --doc, cargo rustdoc -- --test) the
#      crate is empty: nothing in it reaches a test exe, whatever macro builds it. test = false
#      alone does not do this: --lib, --bins and --all-targets select the targets regardless
#      (cargo 1.99 unit_generator.rs; T-092 Investigation §2-3).
# The JSON is read with host `python3 -I` (json module), not a grammar model.
#
# Usage: scripts/ci/shell-test-layout.sh [shell-dir]   (repo-relative; default src-tauri)
# Exit 0: as owned (prints "ok:"). Exit 1: a violation; the listing names the target or crate
# root. Exit 3: cannot run (no Cargo.toml, docker, the core image or cargo metadata failing, the
# JSON not readable, a crate root not readable); never reported as a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
shell="${1:-src-tauri}"
owned=scripts/ci/shell-targets.txt
lib_pin='#![cfg(not(any(test, doctest)))]'
bin_pin='#![cfg(not(test))]'

cannot_run() { echo "shell-test-layout: cannot run: $1" >&2; exit 3; }
# scripts/tw-run mounts only the repository: the shell dir must lie inside it.
case "$shell" in
  /* | .. | ../* | */.. | */../*) cannot_run "$shell is not a repo-relative dir inside the repository" ;;
esac
[ -f "$shell/Cargo.toml" ] || cannot_run "$shell/Cargo.toml not found"
[ -f "$owned" ] || cannot_run "$owned not found"
command -v python3 >/dev/null 2>&1 || cannot_run "python3 not found"
run="$(mktemp -d)" || cannot_run "mktemp failed"
trap 'rm -rf "$run"' EXIT

# 1. cargo's own target list (no build, no network).
if ! scripts/tw-run core -- 'cargo metadata --no-deps --offline --format-version 1 --manifest-path' \
  "$shell/Cargo.toml" >"$run/metadata.json" 2>"$run/metadata.err"; then
  sed 's/^/  | /' "$run/metadata.err" >&2
  cannot_run "cargo metadata failed in the core image for $shell/Cargo.toml (image missing: make core-image)"
fi

# The projection and the line-1 findings. Any error here is exit 3 (the reader never decides a
# violation by failing); violations are data: the projection on stdout, and in pins.txt one line
# per crate root whose line 1 is not its pin.
if ! python3 -I - "$run/metadata.json" "$shell" "$lib_pin" "$bin_pin" "$run/pins.txt" \
  >"$run/targets.txt" 2>"$run/py.err" <<'PY'; then
import json
import os
import sys

meta_path, shell, lib_pin, bin_pin, pins_out = sys.argv[1:6]
with open(meta_path, encoding="utf-8") as f:
    meta = json.load(f)
want_manifest = os.path.normpath(os.path.join(shell, "Cargo.toml"))
# cargo reports absolute paths in its own file system (/work/... in the core image): the
# package is the one named voicen whose manifest path ends with the repo-relative manifest.
packages = [
    p for p in meta["packages"]
    if p["name"] == "voicen"
    and os.path.normpath(p["manifest_path"]).endswith(os.sep + want_manifest)
]
if len(packages) != 1:
    sys.exit("expected one package voicen at %s, cargo metadata reports %d" % (want_manifest, len(packages)))
base = os.path.dirname(os.path.normpath(packages[0]["manifest_path"]))
lib_kinds = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
rows = []
findings = []
for t in packages[0]["targets"]:
    kinds = t["kind"]
    if kinds == ["test"]:
        continue
    rel = os.path.relpath(os.path.normpath(t["src_path"]), base)
    rows.append("%s %s %s test=%s doctest=%s" % (
        ",".join(kinds), t["name"], rel, str(t["test"]).lower(), str(t["doctest"]).lower()))
    if "bin" in kinds:
        pin = bin_pin
    elif lib_kinds.intersection(kinds):
        pin = lib_pin
    else:
        continue
    # The crate root is read in the repo (the host path of cargo's src_path).
    local = os.path.join(shell, rel)
    with open(local, "rb") as f:
        line1 = f.read().split(b"\n", 1)[0]
    if line1 != pin.encode("utf-8"):
        findings.append("%s: line 1 is not the crate-level cfg pin (want: %s, got: %s)" % (
            local, pin, line1.decode("utf-8", "backslashreplace")))
for r in sorted(rows):
    print(r)
with open(pins_out, "w", encoding="utf-8") as f:
    f.write("".join(x + "\n" for x in findings))
PY
  sed 's/^/  | /' "$run/py.err" >&2
  cannot_run "could not read the cargo metadata of $shell (python3 -I)"
fi

found=""
if ! diff -u "$owned" "$run/targets.txt" >"$run/targets.diff"; then
  found="${found}  the voicen targets cargo reports (kind name src_path test doctest, kind-test rows left out) differ from $owned (- owned, + reported):"$'\n'
  found="${found}$(tail -n +3 "$run/targets.diff" | sed 's/^/    /')"$'\n'
fi
while IFS= read -r finding; do
  [ -n "$finding" ] && found="${found}  ${finding}"$'\n'
done <"$run/pins.txt"

if [ -n "$found" ]; then
  {
    echo "shell-test-layout: FAIL: only integration tests ($shell/tests/*.rs) may become a test exe of the voicen package;"
    echo "only they get the release exe's Windows link configuration from $shell/build.rs (F-002)."
    echo "Keep [lib] test = false and doctest = false, [[bin]] name = \"voicen\" test = false, the targets in $owned,"
    echo "and line 1 of the lib crate root '$lib_pin', of the bin crate root '$bin_pin'."
    echo "Why and what to do instead: docs/decisions/ci-toolchain.md"
    printf '%s' "$found"
  } >&2
  exit 1
fi
echo "shell-test-layout: ok: the voicen targets of $shell are as in $owned (lib and bin test = false, doctest = false), and line 1 of each lib and bin crate root is its crate-level cfg pin"
