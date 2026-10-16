#!/usr/bin/env bash
# T-089 (P-020, decides_on: switch): the voicen-core call bans are rustc's path resolution, not
# a spelling. crates/voicen-core/clippy.toml names the banned items (reqwest client types and the
# get shortcuts outside engine::http; std::time::Instant::now / elapsed outside crate::clock and
# tests/common) and crates/voicen-core/Cargo.toml [lints] pins both lints at deny. A config path
# that does not resolve is only a non-denied warning (the build passes), and a lost deny level,
# a lint cap or a config dir set from outside is silent too, so this canary proves the switch:
#
# It lints the package twice: with the feature ban-canary-lib (--lib), which compiles the owned
# probe file crates/voicen-core/src/ban_canary.rs, and with ban-canary-tests (--all-targets),
# which compiles crates/voicen-core/tests/ban_canary.rs; each probe a spelling the retired text guards did not
# read (H1-H21, k1-k15), and compares every diagnostic of the run, as rows
#   <target kind>:<target name> TAB <file>:<line> TAB <level> TAB <lint code> TAB <banned path>
# (the banned path is the one the message names; any other message is its text), sorted, with
# the owned list scripts/ci/ban-canary.expected.txt (lines starting with # and blank lines are
# not rows). Every row is compared, so a diagnostic of any other file is a difference too.
# The list itself must cover the probe files: the <file>:<line> set of its rows must equal the
# set of lines in the two probe files that hold the raw token `probe: ` followed by an id, so
# no probe can be dropped from the list unseen and no row can sit on an unmarked line.
#
# Clippy runs through scripts/tw-run core (the CI image), -j2 (host load limit), --message-format
# json, -- -D warnings (as check-core). Not proven here: the lib's own unit-test build (cfg(test)
# of src), which the lib probes cannot reach while the lib itself is denied. Python 3 (-I, stdlib json) reads the output on the host.
#
# Usage: scripts/ci/ban-canary.sh
# Exit 0: "ok: <n> diagnostic(s) as owned". Exit 1: the list and the run differ (diff printed)
# or the list does not cover the probe lines. Exit 3: cannot run (tw-run or docker missing, the
# image absent, the run did not finish, a probe file or the list unreadable); never a pass.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
expected=scripts/ci/ban-canary.expected.txt
probes=(crates/voicen-core/src/ban_canary.rs crates/voicen-core/tests/ban_canary.rs)
cannot_run() { echo "ban-canary: cannot run: $1" >&2; exit 3; }
[ -f "$expected" ] && [ -r "$expected" ] || cannot_run "$expected not found or not readable"
for f in "${probes[@]}"; do
  [ -f "$f" ] && [ -r "$f" ] || cannot_run "probe file $f not found or not readable"
done
command -v python3 >/dev/null 2>&1 || cannot_run "python3 not found"
tmp="$(mktemp -d)" || cannot_run "mktemp failed"
trap 'rm -rf "$tmp"' EXIT

failed=0

# 1. The owned list covers exactly the marked probe lines.
grep -nE 'probe: [hk][0-9]+' "${probes[@]}" | cut -d: -f1,2 | sort -u >"$tmp/marked" \
  || cannot_run "no line marked probe: in ${probes[*]}"
grep -vE '^[[:space:]]*(#|$)' "$expected" | sort >"$tmp/want"
cut -f2 "$tmp/want" | sort -u >"$tmp/listed"
if ! diff -u --label "marked probe lines" --label "lines in $expected" \
  "$tmp/marked" "$tmp/listed" >"$tmp/cover"; then
  echo "ban-canary: $expected does not cover the probe lines exactly:"
  sed 's/^/  | /' "$tmp/cover"
  failed=1
fi

# 2. The runs' diagnostics equal the owned list. Two runs: a denied probe in the lib stops the
# build of every test target that links it, so the lib probes (feature ban-canary-lib, --lib)
# and the integration-test probes (feature ban-canary-tests, --all-targets, the lib clean) are
# linted apart; their rows are compared as one list.
: >"$tmp/got.unsorted"
rcs=""
for run in "ban-canary-lib --lib" "ban-canary-tests --all-targets"; do
  feature="${run%% *}"
  targets="${run#* }"
  scripts/tw-run core -- cargo clippy -j2 -p voicen-core --features "$feature" $targets \
    --message-format=json -- -D warnings >"$tmp/run.json" 2>"$tmp/run.err"
  rc=$?
  rcs="$rcs $feature=$rc"
  case "$rc" in
    2 | 126 | 127) cannot_run "scripts/tw-run core exited $rc: $(tail -n 3 "$tmp/run.err")" ;;
  esac
  grep -q '"reason":"build-finished"' "$tmp/run.json" \
    || cannot_run "cargo clippy ($feature) did not finish (exit $rc): $(tail -n 5 "$tmp/run.err")"
  python3 -I - "$tmp/run.json" >>"$tmp/got.unsorted" <<'PY' || cannot_run "reading the clippy output ($feature) failed"
import json, re, sys
summary = re.compile(r"^(aborting due to |\d+ warnings? emitted)")
banned = re.compile(r"^use of a disallowed (?:method|type) `([^`]*)`")
for line in open(sys.argv[1], encoding="utf-8"):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if m.get("reason") != "compiler-message":
        continue
    d = m["message"]
    msg = d.get("message") or ""
    spans = d.get("spans") or []
    if not spans and summary.match(msg):
        continue
    t = m.get("target") or {}
    target = "%s:%s" % ((t.get("kind") or ["?"])[0], t.get("name", "?"))
    if spans:
        p = next((s for s in spans if s.get("is_primary")), spans[0])
        loc = "%s:%s" % (p["file_name"], p["line_start"])
    else:
        loc = "-"
    code = (d.get("code") or {}).get("code") or "-"
    b = banned.match(msg)
    what = b.group(1) if b else " ".join(msg.split())
    print("\t".join([target, loc, d.get("level", "?"), code, what]))
PY
done
sort "$tmp/got.unsorted" >"$tmp/got"
if ! diff -u --label "$expected" --label "clippy runs (exit$rcs)" "$tmp/want" "$tmp/got" >"$tmp/diff"; then
  echo "ban-canary: the clippy diagnostics differ from $expected (a ban that does not bite, a"
  echo "ban-canary: config path that does not resolve, a lost deny level or a new diagnostic):"
  sed 's/^/  | /' "$tmp/diff"
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  echo "ban-canary: FAIL: the call bans are not proven (T-089, docs/decisions/ci-toolchain.md)" >&2
  exit 1
fi
echo "ban-canary: ok: $(wc -l <"$tmp/got") diagnostic(s) as owned, $(wc -l <"$tmp/marked") probe line(s)"
