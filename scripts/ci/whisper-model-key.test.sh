#!/usr/bin/env bash
# T-099 (review 1 finding 2, P-010, P-020; invariant 5): the windows job's cache key of the tiny
# whisper model is derived from the catalog file, never a copy of the catalog's data. A key that
# repeats the pinned commit and SHA-256 as a literal still hits after a catalog change, restores
# the old file (fetch_model keeps a file of catalog size), turns
# cached_tiny_model_is_the_pinned_catalog_file red on every ref, and with cache-hit true the save
# never runs, so nothing heals. GitHub evaluates hashFiles() only on the runner, so this test pins
# the key's text, raw lines and owned literals only (no YAML or Rust reader):
#   K1 ci.yml holds exactly one line `        id: whisper-tiny` (the restore step);
#   K2 that step (its id line up to the next `      - ` line) holds exactly one `          key: `
#      line, and inside one `${{ ... }}` of it a hashFiles( ... ) whose arguments include the
#      literal 'crates/voicen-core/src/local_models/catalog.rs';
#   K3 that step holds no restore-keys line: a prefix fallback restores another catalog's model
#      (cache-hit false), and the save after the tests would store it under the new key;
#   K4 no line of ci.yml (comments included) holds a 40- or 64-hex string of catalog.rs (the
#      model commit, a SHA-256): ci.yml keeps no second copy of the catalog;
#   K5 catalog.rs exists and defines the tiny entry (`        id: ModelId::Tiny,` and
#      `        file_name: "ggml-tiny.bin",`), so the hashed file is the one that pins the model.
# Not caught: a key that also holds a constant, or hashFiles over more files (both still change
# with catalog.rs); the save step's key (its byte-exact fragment
# scripts/ci/cache-steps/windows-whisper-tiny-save.yml, release-guard R11, keeps
# cache-primary-key).
# Usage: scripts/ci/whisper-model-key.test.sh [<repo root>]   (host bash; default: this repo)
# Exit 0: every check holds. Exit 1: a check fails (listed). Exit 3: cannot run.
set -uo pipefail
root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
ci="$root/.github/workflows/ci.yml"
catalog_rel="crates/voicen-core/src/local_models/catalog.rs"
catalog="$root/$catalog_rel"
[ -f "$ci" ] || { echo "whisper-model-key.test: cannot run: $ci not found" >&2; exit 3; }

failed=0
passed=0
check() { # check <label> <0 = holds> <why>
  if [ "$2" = 0 ]; then echo "ok   $1"; passed=$((passed + 1)); else echo "FAIL $1: $3" >&2; failed=$((failed + 1)); fi
}

# K1
id_lines="$(grep -nx '        id: whisper-tiny' "$ci" | cut -d: -f1)"
n_id="$(grep -cx '        id: whisper-tiny' "$ci")"
check K1-one-restore-step "$([ "$n_id" = 1 ]; echo $?)" "ci.yml has $n_id line(s) '        id: whisper-tiny', want exactly 1"

step=""
if [ "$n_id" = 1 ]; then
  step="$(awk -v s="$id_lines" 'NR > s && /^      - / { exit } NR >= s { print }' "$ci")"
fi

# K2
keys="$(grep -E '^          key: ' <<<"$step")"
n_key="$(grep -cE '^          key: ' <<<"$step")"
check K2-one-key-line "$([ "$n_key" = 1 ]; echo $?)" "the whisper-tiny restore step has $n_key key line(s), want exactly 1"
derived='\$\{\{[^}]*hashFiles\(([^)]*,[[:space:]]*)?'"'crates/voicen-core/src/local_models/catalog\.rs'"'([[:space:]]*,[^)]*)?\)[^}]*\}\}'
check K2-key-hashes-catalog "$(grep -qE -- "$derived" <<<"$keys"; echo $?)" "the key line does not hash '$catalog_rel' inside \${{ }}: [$keys]"

# K3
check K3-no-restore-keys "$(! grep -qE '^[[:space:]]*restore-keys:' <<<"$step"; echo $?)" "the whisper-tiny restore step has a restore-keys line"

# K5
holds_tiny=1
if [ -f "$catalog" ] && grep -qx '        id: ModelId::Tiny,' "$catalog" && grep -qx '        file_name: "ggml-tiny.bin",' "$catalog"; then
  holds_tiny=0
fi
check K5-catalog-defines-tiny "$holds_tiny" "$catalog_rel is missing or does not define the tiny entry"

# K4
copies=""
if [ -f "$catalog" ]; then
  while IFS= read -r h; do
    [ -n "$h" ] || continue
    hit="$(grep -nF -- "$h" "$ci" | cut -d: -f1 | tr '\n' ' ')"
    [ -n "$hit" ] && copies="$copies $h (ci.yml line(s) ${hit% })"
  done < <(grep -oE '(^|[^0-9a-f])([0-9a-f]{64}|[0-9a-f]{40})($|[^0-9a-f])' "$catalog" | grep -oE '[0-9a-f]{40,64}' | sort -u)
fi
check K4-no-catalog-copy "$([ -z "$copies" ]; echo $?)" "ci.yml repeats catalog data:$copies"

echo "whisper-model-key.test: $passed passed, $failed failed"
[ "$failed" = 0 ]
