#!/usr/bin/env bash
# T-079 (decisions #106, #113; docs/decisions/engine-http.md): every reqwest client of the crate
# is built by engine::http, with the deadline resolver, so a host-name lookup with no answer is
# a DNS failure at the deadline and is never awaited at the client drop. A client built anywhere
# else gets reqwest's default resolver back (the blocking-pool lookup T-079 removed).
#
# Over every *.rs under the source dir, recursively, except the top-level <dir>/engine/http.rs,
# no non-comment line constructs a reqwest client: `Client::builder`, `ClientBuilder::new` or
# `Client::new(`, bare or path-qualified (reqwest::blocking::Client::builder()). A longer
# identifier ending in Client / ClientBuilder (ApiClient::new, HttpClientBuilder::new) is
# another type. A line whose first non-blank characters are // (also /// and //!) is a comment.
# The rule decides on raw text (F-003). Not caught: a `use ... as` alias of Client or
# ClientBuilder, a client built by a macro, the shell (src-tauri, which has no reqwest client).
#
# Usage: scripts/ci/http-client-builder.sh [src-dir]   (default crates/voicen-core/src)
# Exit 0: "ok: <n> file(s)" (every *.rs read, engine/http.rs included). Exit 1: violations, one "<file>:<line>: client ..." per offending
# line. Exit 3: cannot run (dir missing, no *.rs, unreadable file); never reported as a pass.
# Self-test: scripts/ci/http-client-builder.test.sh (make check-http-client-builder).
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
dir="${1:-crates/voicen-core/src}"
dir="${dir%/}"
cannot_run() { echo "http-client-builder: cannot run: $1" >&2; exit 3; }
[ -d "$dir" ] && [ -r "$dir" ] || cannot_run "source dir $dir not found or not readable"

mapfile -d '' all < <(find "$dir" -type f -name '*.rs' -print0 | sort -z)
[ "${#all[@]}" -gt 0 ] || cannot_run "no *.rs under $dir"
files=()
for f in "${all[@]}"; do
  [ -r "$f" ] || cannot_run "$f is not readable"
  [ "$f" = "$dir/engine/http.rs" ] && continue
  files+=("$f")
done

if [ "${#files[@]}" -eq 0 ]; then
  echo "http-client-builder: ok: ${#all[@]} file(s)"
  exit 0
fi

report="$(awk '
/^[ \t]*\/\// { next }
{
  line = $0
  if (line ~ /(^|[^A-Za-z0-9_])(Client::builder|ClientBuilder::new|Client::new[ \t]*\()/)
    print FILENAME ":" FNR ": client built outside engine/http.rs; take engine::http::client / client_with_read_timeout (T-079)"
}
' "${files[@]}")" || cannot_run "awk failed"

if [ -n "$report" ]; then
  printf '%s\n' "$report"
  n="$(printf '%s\n' "$report" | wc -l)"
  echo "http-client-builder: FAIL: $n line(s) build a reqwest client outside engine/http.rs (T-079, docs/decisions/engine-http.md)" >&2
  exit 1
fi
echo "http-client-builder: ok: ${#all[@]} file(s)"
