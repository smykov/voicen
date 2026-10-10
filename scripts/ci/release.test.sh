#!/usr/bin/env bash
# T-085 (rca, decisions #120; Investigation section 6.1) after T-026 (decisions #102, #103): host
# test of scripts/ci/release.sh, the one step of ci.yml's release job. The release check and the
# publish are one script, so the order and the exit status are proven by running it, not by
# reading YAML. Contract, pinned here:
#   Usage: scripts/ci/release.sh <tag> <run_number> <root> <installer dir>
#   - GH_TOKEN unset or empty: exit 1 naming GH_TOKEN, before any gh call and before anything is
#     written (gh would otherwise fall back to GITHUB_TOKEN or its stored login).
#   - Then scripts/ci/release-check.sh <tag> <run_number> <root> <installer dir> (its verdict,
#     unchanged): a refusal (any non-zero exit) exits non-zero, with no gh write call and nothing
#     written to <installer dir>.
#   - On a pass: <installer dir>/SHA256SUMS.txt in sha256sum format (one line "<sha256>  <name>"
#     per installer), the release notes in a file, then exactly one
#     `gh release create <tag> <installer> <...>/SHA256SUMS.txt --verify-tag ... --notes-file <notes>`
#     (no other gh call but `gh --version`); the notes name the version X.Y.Z and the
#     installer's file name. Exit 0.
#   - gh release create fails: exit non-zero (never a pass).
#   - Never prints GH_TOKEN.
#   - Exit 2: wrong number of arguments, nothing called.
# Stub gh first on PATH (offline): it logs every call and, at the moment of `release create`,
# copies the SHA256SUMS.txt and notes files it is given, so the test sees what gh would upload.
# Usage: scripts/ci/release.test.sh   (host bash, grep, sed, sha256sum)
# Exit 0: every case as expected. Exit 1: a case differs (listed), or the script is missing.
# Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/ci/release.sh"
if [ ! -x "$script" ]; then
  echo "release.test: FAIL: $script not found or not executable (T-085: check and publish in one script)" >&2
  exit 1
fi
command -v sha256sum >/dev/null || { echo "release.test: cannot run: sha256sum not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "release.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

token='example-fake-gh-token-not-a-secret'

# Stub gh: one log line per call; `release view` answered from STUB_VIEW (missing | exists |
# error); `release create` answered from STUB_CREATE (ok | fail) after copying, as they are at
# that moment, the SHA256SUMS.txt it is given (to sums.at-create), the --notes-file (to
# notes.at-create) and the names of the *-setup.exe files it is given (to installers.at-create).
mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_DIR/gh.log"
[ "$*" = --version ] && { echo "gh version 2.101.0 (stub)"; exit 0; }
if [ "${1:-}" = release ] && [ "${2:-}" = view ]; then
  case "${STUB_VIEW:-missing}" in
    exists) printf 'title:\t%s\ntag:\t%s\n' "${3:-}" "${3:-}"; exit 0 ;;
    missing) echo "release not found" >&2; exit 1 ;;
    *) echo "HTTP 401: Bad credentials" >&2; exit 1 ;;
  esac
fi
if [ "${1:-}" = release ] && [ "${2:-}" = create ]; then
  prev=
  for a in "$@"; do
    case "$a" in
      *SHA256SUMS.txt) [ -f "$a" ] && cp -- "$a" "$STUB_DIR/sums.at-create" ;;
      *-setup.exe) [ -f "$a" ] && basename -- "$a" >> "$STUB_DIR/installers.at-create" ;;
      --notes-file=*) [ -f "${a#--notes-file=}" ] && cp -- "${a#--notes-file=}" "$STUB_DIR/notes.at-create" ;;
    esac
    if [ "$prev" = --notes-file ] && [ -f "$a" ]; then cp -- "$a" "$STUB_DIR/notes.at-create"; fi
    prev="$a"
  done
  [ "${STUB_CREATE:-ok}" = ok ] && { echo "https://github.com/example/voicen/releases/tag/${3:-}"; exit 0; }
  echo "HTTP 422: Validation Failed" >&2
  exit 1
fi
echo "stub gh: unsupported call: $*" >&2
exit 1
EOF
chmod +x "$tmp/bin/gh"

# mk_tree <dir> <version>: the version files release-check.sh (version-stamp.sh) reads.
mk_tree() {
  local d="$1" v="$2"
  mkdir -p "$d/src-tauri"
  printf '[workspace]\nmembers = ["crates/voicen-core", "src-tauri"]\nresolver = "2"\n\n[workspace.package]\nversion = "%s"\nedition = "2021"\n' "$v" > "$d/Cargo.toml"
  printf '{\n  "productName": "Voicen",\n  "version": "%s"\n}\n' "$v" > "$d/src-tauri/tauri.conf.json"
  printf '{\n  "name": "voicen",\n  "version": "%s"\n}\n' "$v" > "$d/package.json"
  printf 'version = 4\n\n[[package]]\nname = "voicen"\nversion = "%s"\n\n[[package]]\nname = "voicen-core"\nversion = "%s"\n' "$v" "$v" > "$d/Cargo.lock"
}

# mk_dist <dir> <installer name>: an installer with bytes unique to its name.
mk_dist() {
  rm -rf "$1"
  mkdir -p "$1"
  printf 'MZ fake installer %s\n' "$2" > "$1/$2"
}

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

# call <label> <view> <create> <token|-|empty> <args...>: runs release.sh from a work dir with the
# stub gh first on PATH; "-" runs it without GH_TOKEN (and with a GITHUB_TOKEN gh would use),
# "empty" with GH_TOKEN="". Sets out/rc; gh calls are in $tmp/gh.log; $tmp/dist.before holds the
# installer dir's listing before the run.
call() {
  label="$1"; ok=1
  local view="$2" create="$3" tok="$4"
  shift 4
  rm -f "$tmp/gh.log" "$tmp/sums.at-create" "$tmp/notes.at-create" "$tmp/installers.at-create"
  : > "$tmp/gh.log"
  ls -A "$tmp/dist" > "$tmp/dist.before" 2>/dev/null || : > "$tmp/dist.before"
  local -a envs=(env -u GH_TOKEN -u GITHUB_TOKEN PATH="$tmp/bin:$PATH" STUB_DIR="$tmp" STUB_VIEW="$view" STUB_CREATE="$create")
  case "$tok" in
    -) envs+=(GITHUB_TOKEN="$token") ;;
    empty) envs+=(GH_TOKEN= GITHUB_TOKEN="$token") ;;
    *) envs+=(GH_TOKEN="$tok") ;;
  esac
  out="$(cd "$tmp/work" && "${envs[@]}" "$script" "$@" 2>&1)"
  rc=$?
  ! grep -qF -- "$token" <<<"$out" || fail "the output contains the token"
}
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
want_nonzero() { [ "$rc" != 0 ] || fail "exit 0, want non-zero"; }
gh_none() { [ ! -s "$tmp/gh.log" ] || fail "gh was called: $(tr '\n' '|' < "$tmp/gh.log")"; }
gh_no_write() {
  if grep -qwE -- 'create|upload|edit|delete|delete-asset' "$tmp/gh.log"; then
    fail "a gh write subcommand was called: $(tr '\n' '|' < "$tmp/gh.log")"
  fi
}
dist_unchanged() {
  ls -A "$tmp/dist" > "$tmp/dist.after" 2>/dev/null || : > "$tmp/dist.after"
  cmp -s "$tmp/dist.before" "$tmp/dist.after" ||
    fail "the installer dir changed: $(diff "$tmp/dist.before" "$tmp/dist.after" | tr '\n' '|')"
}
says() { grep -qF -- "$1" <<<"$out" || fail "the output does not contain $1 (${2:-})"; }

mkdir -p "$tmp/work"
mk_tree "$tmp/repo" 0.1.0
inst=Voicen_0.1.200_x64-setup.exe

# ---- pass: check, SHA256SUMS.txt, notes, one gh release create ------------------------------
mk_dist "$tmp/dist" "$inst"
want_sums="$(cd "$tmp/dist" && sha256sum -- "$inst")"
call pass-v0.1.200-run-100 missing ok "$token" v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_rc 0
# the release-exists check ran (release-check.sh with the token), before the one create
grep -qxE -- 'release view v0\.1\.200( .*)?' "$tmp/gh.log" || fail "gh release view v0.1.200 was not called: $(tr '\n' '|' < "$tmp/gh.log")"
n_create="$(grep -c -E -- '^release create ' "$tmp/gh.log")"
[ "$n_create" = 1 ] || fail "gh release create called $n_create times, want 1: $(tr '\n' '|' < "$tmp/gh.log")"
first_view="$(grep -n -m 1 -E -- '^release view ' "$tmp/gh.log" | cut -d: -f1)"
first_create="$(grep -n -m 1 -E -- '^release create ' "$tmp/gh.log" | cut -d: -f1)"
[ -n "$first_view" ] && [ -n "$first_create" ] && [ "$first_view" -lt "$first_create" ] ||
  fail "release view must come before release create: $(tr '\n' '|' < "$tmp/gh.log")"
grep -qE -- '^release create v0\.1\.200 ' "$tmp/gh.log" || fail "the create is not for tag v0.1.200: $(tr '\n' '|' < "$tmp/gh.log")"
grep -E -- '^release create ' "$tmp/gh.log" | grep -qwF -- '--verify-tag' || fail "the create has no --verify-tag"
# no other gh call (gh api, release upload, ...); gh --version is a read
if grep -qvE -- '^(release (view|create) |--version$)' "$tmp/gh.log"; then
  fail "a gh call other than release view / create / --version: $(tr '\n' '|' < "$tmp/gh.log")"
fi
[ "$(cat "$tmp/installers.at-create" 2>/dev/null)" = "$inst" ] ||
  fail "the create did not get exactly the installer $inst (got: $(tr '\n' '|' < "$tmp/installers.at-create" 2>/dev/null))"
[ -f "$tmp/sums.at-create" ] || fail "the create did not get an existing SHA256SUMS.txt"
[ "$(cat "$tmp/sums.at-create" 2>/dev/null)" = "$want_sums" ] ||
  fail "SHA256SUMS.txt is not sha256sum's line for the installer (want: $want_sums; got: $(cat "$tmp/sums.at-create" 2>/dev/null))"
[ -f "$tmp/dist/SHA256SUMS.txt" ] || fail "no SHA256SUMS.txt in the installer dir"
[ -f "$tmp/notes.at-create" ] || fail "the create did not get an existing --notes-file"
grep -qF -- 0.1.200 "$tmp/notes.at-create" 2>/dev/null || fail "the notes do not name the version 0.1.200"
grep -qF -- "$inst" "$tmp/notes.at-create" 2>/dev/null || fail "the notes do not name the installer $inst"
(cd "$tmp/dist" && sha256sum -c --quiet SHA256SUMS.txt) >/dev/null 2>&1 || fail "sha256sum -c fails on the written SHA256SUMS.txt"
done_case

# ---- no token: nothing at all ---------------------------------------------------------------
mk_dist "$tmp/dist" "$inst"
call refuse-no-gh-token missing ok - v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_rc 1; says GH_TOKEN "why it stops"; gh_none; dist_unchanged
done_case
mk_dist "$tmp/dist" "$inst"
call refuse-empty-gh-token missing ok empty v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_rc 1; says GH_TOKEN "why it stops"; gh_none; dist_unchanged
done_case

# ---- release-check.sh refuses: no write -----------------------------------------------------
# #102: MAJOR.MINOR differs (release-check names 0.2 and 0.1).
mk_dist "$tmp/dist" Voicen_0.2.200_x64-setup.exe
call refuse-check-102 missing ok "$token" v0.2.200 100 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
done_case
# #103: patch below the run number.
mk_dist "$tmp/dist" Voicen_0.1.99_x64-setup.exe
call refuse-check-103 missing ok "$token" v0.1.99 100 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
done_case
# The installer's version is not the tag's.
mk_dist "$tmp/dist" Voicen_0.1.7_x64-setup.exe
call refuse-check-installer missing ok "$token" v0.1.8 5 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
done_case
# The release exists: never overwritten.
mk_dist "$tmp/dist" "$inst"
call refuse-release-exists exists ok "$token" v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
grep -qE -- '^release view v0\.1\.200' "$tmp/gh.log" || fail "the release-exists check did not run"
done_case
# gh cannot tell whether the release exists.
mk_dist "$tmp/dist" "$inst"
call refuse-gh-view-error error ok "$token" v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
done_case
# A bad tag.
mk_dist "$tmp/dist" "$inst"
call refuse-bad-tag missing ok "$token" 0.1.200 100 "$tmp/repo" "$tmp/dist"
want_nonzero; gh_no_write; dist_unchanged
done_case

# ---- the publish fails: never a pass --------------------------------------------------------
mk_dist "$tmp/dist" "$inst"
call create-fails missing fail "$token" v0.1.200 100 "$tmp/repo" "$tmp/dist"
want_nonzero
[ "$(grep -c -E -- '^release create ' "$tmp/gh.log")" = 1 ] || fail "gh release create was not called exactly once"
done_case

# ---- usage ----------------------------------------------------------------------------------
mk_dist "$tmp/dist" "$inst"
call usage-three-arguments missing ok "$token" v0.1.200 100 "$tmp/repo"
want_rc 2; gh_none; dist_unchanged
done_case
call usage-five-arguments missing ok "$token" v0.1.200 100 "$tmp/repo" "$tmp/dist" extra
want_rc 2; gh_none; dist_unchanged
done_case

if [ "$failed" != 0 ]; then
  echo "release.test: FAIL: $failed case(s) differ, $passed as expected (T-085, release.sh)" >&2
  exit 1
fi
echo "release.test: ok: $passed cases as expected"
