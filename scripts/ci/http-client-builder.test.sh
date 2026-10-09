#!/usr/bin/env bash
# T-079 (decisions #106, #113; docs/decisions/engine-http.md) guard of the guard: the tripwire
# scripts/ci/http-client-builder.sh must give its documented exit code on every committed
# fixture source dir under scripts/ci/fixtures/http-client-builder/<case>/, on two cases built
# here (no dir, a dir without *.rs) and on the real crates/voicen-core/src.
#
# The rule the tripwire enforces: every reqwest client of the crate is built by engine::http
# (with the deadline resolver, so a lookup with no answer is a DNS failure at the deadline and
# is never awaited at the client drop). Over every *.rs under the source dir, recursively,
# except the top-level <dir>/engine/http.rs, no non-comment line constructs a reqwest client:
#   `Client::builder`, `ClientBuilder::new` or `Client::new(` (bare or path-qualified, e.g.
#   reqwest::blocking::Client::builder()); a longer identifier ending in Client / ClientBuilder
#   (ApiClient::new, HttpClientBuilder::new) is another type.
# A line whose first non-blank characters are // (also /// and //!) is a comment.
#   ok-*  exit 0, "ok: <n> file(s)" (n pins that the files were read);
#   v-*   exit 1, the listing names <file>:<line>: client ..., one line per offending line;
#   c-*   exit 3, "cannot run".
# Every fixture dir must appear in the table, so a case cannot be dropped silently.
# Usage: scripts/ci/http-client-builder.test.sh   (host bash)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/http-client-builder.sh
fx=scripts/ci/fixtures/http-client-builder
[ -d "$fx" ] || { echo "http-client-builder.test: cannot run: $fx not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "http-client-builder.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0
declare -A seen=()

# run <label> <expected exit> <needle> [guard args...]; the needle must be in the output.
run() {
  local label="$1" want="$2" needle="$3" out got
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the tripwire does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$@" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $label: exit $got as wanted, but the output does not contain: $needle" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>: the case's dir as the source dir. A needle starting
# with "/" is taken relative to the case dir (file:line: client).
check() {
  local n="$3"
  seen["$1"]=1
  [ "${n#/}" = "$n" ] || n="$fx/$1$n"
  run "$1" "$2" "$n" "$fx/$1"
}

# not_listed <case> <needle>: the case's output must not contain the needle (a file or line
# that is allowed must not be reported next to the one that is not).
not_listed() {
  local out
  [ -x "$guard" ] || return
  out="$("$guard" "$fx/$1" 2>&1)"
  if grep -qF -- "$fx/$1$2" <<<"$out"; then
    echo "FAIL $1: the output lists the allowed $2" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $1: does not list $2"
    passed=$((passed + 1))
  fi
}

# Allowed.
check ok-builder-in-http        0 "ok: 2 file(s)"   # Client::builder and ClientBuilder::new in engine/http.rs; a caller of http::client
check ok-comments               0 "ok: 1 file(s)"   # //, ///, //! lines (and an indented //) naming the constructors
check ok-other-constructors     0 "ok: 2 file(s)"   # ApiClient::new, HttpClientBuilder::new, ModelStore::new, multipart::Form::new

# A client built outside engine/http.rs.
check v-download-own-builder    1 "/local_models/download.rs:5: client"  # T-079's second site: Downloader::transfer's own builder
not_listed v-download-own-builder "/engine/http.rs"                       # the one allowed file is not reported next to it
check v-qualified-path          1 "/post_process/chat.rs:2: client"      # reqwest::blocking::Client::builder()
check v-client-new              1 "/probe.rs:3: client"                  # reqwest::blocking::Client::new(): the default resolver
check v-clientbuilder-new       1 "/builder.rs:4: client"                # ClientBuilder::new()
check v-other-http-rs           1 "/post_process/http.rs:4: client"      # only engine/http.rs is exempt, not any http.rs
check v-nested-engine-http      1 "/shell/engine/http.rs:2: client"      # only the top-level engine/http.rs is exempt

# Cannot run.
run c-no-dir 3 "cannot run" "$tmp/absent"
mkdir -p "$tmp/no-rs" && printf 'x\n' >"$tmp/no-rs/README.md"
run c-no-rs-files 3 "cannot run" "$tmp/no-rs"

# The real tree with the guard's default (crates/voicen-core/src): red until
# Downloader::transfer builds its client through engine::http (T-079).
run "real tree (crates/voicen-core/src)" 0 "ok:"

# Every committed fixture dir is in the table.
for d in "$fx"/*/; do
  c="$(basename "$d")"
  if [ -z "${seen[$c]:-}" ]; then
    echo "FAIL $c: fixture dir has no expected exit code in $0" >&2
    failed=$((failed + 1))
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "http-client-builder.test: FAIL: $failed case(s) differ, $passed as expected (T-079)" >&2
  exit 1
fi
echo "http-client-builder.test: ok: $passed cases as expected"
