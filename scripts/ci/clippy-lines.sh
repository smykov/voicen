#!/usr/bin/env bash
# T-089 (P-020, decides_on: token + literal): the voicen-core call bans (reqwest client
# construction outside engine::http, wall-clock reads outside crate::clock and tests/common) are
# decided by rustc: clippy disallowed-methods / disallowed-types in crates/voicen-core/clippy.toml,
# denied by the [lints.clippy] table of crates/voicen-core/Cargo.toml and proven to bite by
# make check-ban-canary. Every way left to lower a denied lint in the source spells the token
# clippy (allow / expect of the lint, of clippy::all or a group, a renamed lint, a -A flag in a
# cargo config or on check-core's command line, a config dir set by a build script or a make
# file), so this guard holds: every line holding that token is an owned line
# (docs/decisions/ci-toolchain.md, docs/tasks/T-089.md §4, §5, §12, review 1 #3).
#
# Scanned: every file `git -C <root> ls-files` lists whose name ends in .rs, whose base name is
#   Cargo.toml, or that is config or config.toml in a .cargo directory; and every make file:
#   base name Makefile, makefile or GNUmakefile, or a name ending in .mk. check-core's clippy
#   command line in the Makefile is where the bans apply to real code, so it is an owned line
#   bound to the Makefile: dropping --all-targets, swapping clippy for cargo check, or adding
#   -A clippy::disallowed_* to it is a finding (the edited line is not owned, its entry is stale).
#   Comments are lines like any other (no // or # skip); the guard reads no Rust, TOML or make
#   syntax. Not seen (review only, T1): a make-level setting without the token
#   (export RUSTFLAGS := --cap-lints allow) and a check: that drops check-core.
# Token: a scanned line holds the token when it contains clippy in any case.
# Owned: scripts/ci/clippy-lines.txt under <root>; lines starting with # and blank lines are not
#   entries; an entry is <path> TAB <text>. A token line is owned when, with leading and trailing
#   blanks removed, it equals the <text> of an entry whose <path> is the line's file (relative to
#   root) or *.
#   - An entry bound to a path owns one line: a file holds exactly as many lines with that text
#     as there are entries for it (one more line is not owned, one fewer is a stale entry).
#   - A * entry owns its text in any file, any number of times, must match at least one line, and
#     its text must not contain disallowed in any case. That is the whole check on a * entry: an
#     entry naming a lint group (#[allow(clippy::all)], clippy::style) would lower the bans in
#     any file and passes; only the review of clippy-lines.txt edits catches it (T1).
#
# Usage: scripts/ci/clippy-lines.sh [root]   (default: the repository)
# Exit 0: "clippy-lines: ok: <n> line(s) owned". Exit 1: one "<file>:<line>: clippy: ..." per
# line that is not owned, one "scripts/ci/clippy-lines.txt:<n>: ..." per stale or refused entry.
# Exit 3: cannot run (root not a git work tree, no owned list, no scanned file, a file
# unreadable); never a pass.
set -uo pipefail
cannot_run() { echo "clippy-lines: cannot run: $1" >&2; exit 3; }
if [ $# -gt 0 ]; then
  root="$(cd "$1" 2>/dev/null && pwd -P)" || cannot_run "root $1 not found"
else
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)" || exit 3
fi
command -v git >/dev/null 2>&1 || cannot_run "git not found"
top="$(git -C "$root" rev-parse --show-toplevel 2>/dev/null)" || cannot_run "$root is not a git work tree"
top="$(cd "$top" && pwd -P)" || cannot_run "$root is not a git work tree"
[ "$top" = "$root" ] || cannot_run "$root is not the top of a git work tree"
cd "$root" || cannot_run "cannot enter $root"

list=scripts/ci/clippy-lines.txt
[ -f "$list" ] && [ -r "$list" ] || cannot_run "owned list $list not found or not readable"

mapfile -d '' tracked < <(git ls-files -z) || cannot_run "git ls-files failed"
files=()
for f in "${tracked[@]}"; do
  case "$f" in
    *.rs | Cargo.toml | */Cargo.toml | .cargo/config | .cargo/config.toml | */.cargo/config | */.cargo/config.toml) ;;
    Makefile | */Makefile | makefile | */makefile | GNUmakefile | */GNUmakefile | *.mk) ;;
    *) continue ;;
  esac
  [ -f "$f" ] && [ -r "$f" ] || cannot_run "$f is tracked but not readable"
  files+=("$f")
done
[ "${#files[@]}" -gt 0 ] || cannot_run "no tracked .rs, Cargo.toml, .cargo config or make file under $root"

report="$(LC_ALL=C awk -v list="$list" '
function trim(s) { sub(/^[ \t\r]+/, "", s); sub(/[ \t\r]+$/, "", s); return s }
FNR == 1 { inlist = (FILENAME == list && !listdone); if (!inlist) listdone = 1 }
inlist {
  if ($0 ~ /^#/ || $0 ~ /^[ \t\r]*$/) next
  t = index($0, "\t")
  if (t == 0) { print list ":" FNR ": entry without a TAB between <path> and <text>"; bad = 1; next }
  p = substr($0, 1, t - 1); x = trim(substr($0, t + 1))
  if (p == "" || x == "") { print list ":" FNR ": entry with an empty <path> or <text>"; bad = 1; next }
  if (tolower(x) !~ /clippy/) { print list ":" FNR ": entry text does not hold the token clippy"; bad = 1; next }
  if (p == "*") {
    if (tolower(x) ~ /disallowed/) {
      print list ":" FNR ": a * entry names a ban (disallowed); bind the exemption to its file"
      bad = 1; next
    }
    gen[x] = 1; genline[x] = genline[x] " " FNR
  } else {
    k = p SUBSEP x
    want[k]++; wline[k, want[k]] = FNR
  }
  next
}
{
  if (tolower($0) !~ /clippy/) next
  x = trim($0); k = FILENAME SUBSEP x
  if ((k in want) && got[k] < want[k]) { got[k]++; owned++; next }
  if (x in gen) { gused[x]++; owned++; next }
  print FILENAME ":" FNR ": clippy: a line holding the token clippy that is not owned by " list " (it can lower the voicen-core call bans; T-089)"
  bad = 1
}
END {
  for (k in want) {
    for (i = got[k] + 1; i <= want[k]; i++) {
      print list ":" wline[k, i] ": stale entry: its line is not in the file it names"
      bad = 1
    }
  }
  for (x in gen) {
    if (!(x in gused)) {
      n = split(genline[x], ls, " ")
      for (i = 1; i <= n; i++) print list ":" ls[i] ": stale entry: no scanned line holds its text"
      bad = 1
    }
  }
  print "#owned " owned + 0
  exit bad ? 1 : 0
}
' "$list" "${files[@]}")"
rc=$?
[ "$rc" -le 1 ] || cannot_run "awk failed"

owned="$(sed -n 's/^#owned //p' <<<"$report")"
findings="$(grep -v '^#owned ' <<<"$report" | sort -t: -k1,1 -k2,2n)"
if [ "$rc" -ne 0 ] || [ -n "$findings" ]; then
  printf '%s\n' "$findings"
  n="$(printf '%s\n' "$findings" | grep -c .)"
  echo "clippy-lines: FAIL: $n finding(s); every line holding clippy must be owned by $list (T-089, docs/decisions/ci-toolchain.md)" >&2
  exit 1
fi
echo "clippy-lines: ok: $owned line(s) owned"
