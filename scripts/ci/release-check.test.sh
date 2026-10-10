#!/usr/bin/env bash
# T-026 (decisions #102, #103): "release job version check" -- host test of
# scripts/ci/release-check.sh, the one release verdict of .github/workflows/ci.yml (the release
# job before sha256sum and gh release create, and the windows job's premise / failure-branch
# steps on every ref). Contract, pinned here:
#   Usage: scripts/ci/release-check.sh <tag> <run_number> <root> <installer dir>
#   - <tag> must be vX.Y.Z (decimal, no leading zeros), else exit 1 naming the tag.
#   - The committed MAJOR.MINOR is read from <root>: [workspace.package] version of Cargo.toml and
#     the top-level "version" of src-tauri/tauri.conf.json and package.json (the files
#     version-stamp.sh reads), which must agree; else exit 1. X.Y must equal it (#102), else
#     exit 1 naming both MAJOR.MINOR values (e.g. 0.2 and 0.1).
#   - <run_number> must be a decimal number, else exit 1 (naming it when not empty). Z must be
#     >= it, compared as numbers (#103; Z = run number passes), else exit 1 naming Z and the
#     run number (e.g. 99 and 100).
#   - <installer dir> must hold exactly one *-setup.exe, and its name must contain _X.Y.Z_, else
#     exit 1 (a wrong version names the installer's version and the tag's).
#   - With GH_TOKEN set: `gh release view <tag>` decides whether the release exists. Exit 0 ->
#     exit 1 naming the tag (never overwrite); exit non-zero with "release not found" -> no release;
#     any other gh failure -> exit 1 (cannot tell, never a pass). Without GH_TOKEN (the windows
#     job's premise) gh is not called and the output names GH_TOKEN as the reason.
#   - Never calls a gh write subcommand (create, upload, edit, delete, delete-asset); never prints
#     GH_TOKEN.
#   - Exit 0: the release may be published; the output names the installer path.
#   - Exit 2: wrong number of arguments.
# A number is "named" when it stands alone in the output: not part of a longer dotted number, so
# the tag v0.1.99 alone does not name 99, nor the version 0.1.0 the MAJOR.MINOR 0.1.
# Stub gh first on PATH (offline); fixture trees with decoy version lines; a copy of the real
# repository's version files.
# Usage: scripts/ci/release-check.test.sh   (host bash, grep, sed)
# Exit 0: every case as expected. Exit 1: a case differs (listed), or the script is missing.
# Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
script="$root/scripts/ci/release-check.sh"
if [ ! -x "$script" ]; then
  echo "release-check.test: FAIL: $script not found or not executable (release job version check)" >&2
  exit 1
fi
tmp="$(mktemp -d)" || { echo "release-check.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

token='example-fake-gh-token-not-a-secret'

# Stub gh: logs each call (all arguments on one line) and answers `release view` from
# STUB_VIEW (missing | exists | error); every other call is unsupported (exit 1).
mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_DIR/gh.log"
if [ "${1:-}" = release ] && [ "${2:-}" = view ]; then
  case "${STUB_VIEW:-missing}" in
    exists) printf 'title:\t%s\ntag:\t%s\n' "${3:-}" "${3:-}"; exit 0 ;;
    missing) echo "release not found" >&2; exit 1 ;;
    *) echo "HTTP 401: Bad credentials (https://api.github.com/repos/example/example/releases/tags/${3:-})" >&2; exit 1 ;;
  esac
fi
echo "stub gh: unsupported call: $*" >&2
exit 1
EOF
chmod +x "$tmp/bin/gh"

# mk_tree <dir> <cargo ver> [tauri ver] [package ver]: the files version-stamp.sh reads, with
# decoy version lines of another MAJOR.MINOR placed before the real fields.
mk_tree() {
  local d="$1" cv="$2" tv="${3:-$2}" pv="${4:-$2}"
  mkdir -p "$d/src-tauri"
  cat > "$d/Cargo.toml" <<EOF
[workspace.metadata.decoy]
version = "7.7.0"

[workspace]
members = ["crates/voicen-core", "src-tauri"]
resolver = "2"

[workspace.package]
version = "$cv"
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "7.7.0", features = ["derive"] }
EOF
  cat > "$d/src-tauri/tauri.conf.json" <<EOF
{
  "decoy": {
    "version": "7.7.0"
  },
  "productName": "Voicen",
  "version": "$tv"
}
EOF
  cat > "$d/package.json" <<EOF
{
  "name": "voicen",
  "config": {
    "version": "7.7.0"
  },
  "version": "$pv"
}
EOF
  cat > "$d/Cargo.lock" <<EOF
version = 4

[[package]]
name = "voicen"
version = "$cv"

[[package]]
name = "voicen-core"
version = "$cv"
EOF
}

# mk_dir <dir> <file names...>: an installer dir holding the named files (fake bytes) plus decoys
# that are not *-setup.exe.
mk_dir() {
  local d="$1"
  shift
  rm -rf "$d"
  mkdir -p "$d"
  printf 'decoy' > "$d/notes.txt"
  printf 'decoy' > "$d/Voicen_9.9.9_x64-setup.exe.sig"
  local f
  for f in "$@"; do printf 'MZ' > "$d/$f"; done
}

failed=0
passed=0
fail() { echo "FAIL $label: $1" >&2; sed 's/^/       | /' <<<"$out" >&2; failed=$((failed + 1)); ok=0; }
done_case() { [ "$ok" = 1 ] && { echo "ok   $label"; passed=$((passed + 1)); }; }

# call <label> <view> <token|-> <args...>: runs the script with the stub gh first on PATH;
# token "-" runs it without GH_TOKEN. Sets out/rc; the gh calls are in $tmp/gh.log.
call() {
  label="$1"; ok=1
  local view="$2" tok="$3"
  shift 3
  : > "$tmp/gh.log"
  if [ "$tok" = - ]; then
    out="$(env -u GH_TOKEN -u GITHUB_TOKEN PATH="$tmp/bin:$PATH" STUB_DIR="$tmp" STUB_VIEW="$view" "$script" "$@" 2>&1)"
  else
    out="$(env -u GITHUB_TOKEN PATH="$tmp/bin:$PATH" STUB_DIR="$tmp" STUB_VIEW="$view" GH_TOKEN="$tok" "$script" "$@" 2>&1)"
  fi
  rc=$?
  common
}
# Every case: no gh write subcommand, the token never printed.
common() {
  if grep -qwE -- 'create|upload|edit|delete|delete-asset' "$tmp/gh.log"; then
    fail "a gh write subcommand was called: $(tr '\n' '|' < "$tmp/gh.log")"
  fi
  ! grep -qF -- "$token" <<<"$out" || fail "the output contains GH_TOKEN"
}
want_rc() { [ "$rc" = "$1" ] || fail "exit $rc, want $1"; }
# names <number>: the number stands alone in the output (not inside a longer dotted number).
names() {
  local re
  re="$(sed 's/\./\\./g' <<<"$1")"
  grep -qE -- "(^|[^0-9.])${re}(\$|[^0-9.]|\.(\$|[^0-9]))" <<<"$out" || fail "the output does not name $1 (${2:-})"
}
# says <text>: the text appears verbatim.
says() { grep -qF -- "$1" <<<"$out" || fail "the output does not contain $1 (${2:-})"; }
gh_viewed() {
  grep -qE -- "^release view (.* )?$(sed 's/\./\\./g' <<<"$1")( |\$)" "$tmp/gh.log" ||
    fail "gh release view $1 was not called (gh calls: $(tr '\n' '|' < "$tmp/gh.log"))"
}
gh_not_called() { [ ! -s "$tmp/gh.log" ] || fail "gh was called: $(tr '\n' '|' < "$tmp/gh.log")"; }

mk_tree "$tmp/repo" 0.1.0
mk_tree "$tmp/stamped" 0.1.100
mk_tree "$tmp/disagree" 0.1.0 0.1.0 0.2.0

# ---- releasable ----------------------------------------------------------------------------

# The owner's tag on the committed (unstamped) tree: the release job's pass path.
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe
call pass-v0.1.200-run-100 missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 0
says "$tmp/i/Voicen_0.1.200_x64-setup.exe" "the installer path"
gh_viewed v0.1.200
done_case

# #103 boundary: patch = run number passes (no earlier build had it).
mk_dir "$tmp/i" Voicen_0.1.100_x64-setup.exe
call pass-boundary-v0.1.100-run-100 missing "$token" v0.1.100 100 "$tmp/repo" "$tmp/i"
want_rc 0
says "$tmp/i/Voicen_0.1.100_x64-setup.exe" "the installer path"
gh_viewed v0.1.100
done_case

# Numbers, not strings: 1000 >= 200.
mk_dir "$tmp/i" Voicen_0.1.1000_x64-setup.exe
call pass-numeric-v0.1.1000-run-200 missing "$token" v0.1.1000 200 "$tmp/repo" "$tmp/i"
want_rc 0
done_case

# Each part may be up to 65535 (validation 2 sweep: the bound, not one below it).
mk_dir "$tmp/i" Voicen_0.1.65535_x64-setup.exe
call pass-v0.1.65535-run-100 missing "$token" v0.1.65535 100 "$tmp/repo" "$tmp/i"
want_rc 0
done_case

# The windows job's premise on a wip run: stamped tree <MAJOR.MINOR>.<run>, tag v$VOICEN_VERSION,
# no token. gh would say "exists", but must not be asked.
mk_dir "$tmp/i" Voicen_0.1.100_x64-setup.exe
call premise-stamped-no-token exists - v0.1.100 100 "$tmp/stamped" "$tmp/i"
want_rc 0
says "$tmp/i/Voicen_0.1.100_x64-setup.exe" "the installer path"
says GH_TOKEN "why the release-exists check was not made"
gh_not_called
done_case

# A copy of the real repository's version files.
real="$tmp/real"
mkdir -p "$real/src-tauri"
cp Cargo.toml Cargo.lock package.json "$real/" && cp src-tauri/tauri.conf.json "$real/src-tauri/" ||
  { echo "release-check.test: cannot run: cannot copy the repository files" >&2; exit 3; }
repo_ver="$(awk '/^\[/{s=($0=="[workspace.package]")} s && /^version *=/{gsub(/.*= *"|".*/,""); print; exit}' Cargo.toml)"
mm="${repo_ver%.*}"
if [ -z "$mm" ]; then echo "release-check.test: cannot run: no [workspace.package] version in Cargo.toml" >&2; exit 3; fi
mk_dir "$tmp/i" "Voicen_${mm}.1000_x64-setup.exe"
call "real-repo-v${mm}.1000-run-100" missing "$token" "v${mm}.1000" 100 "$real" "$tmp/i"
want_rc 0
done_case

# ---- refused: exit 1 -----------------------------------------------------------------------

# #102: tag MAJOR.MINOR differs from the committed one.
mk_dir "$tmp/i" Voicen_0.2.200_x64-setup.exe
call refuse-102-v0.2.200-on-0.1 missing "$token" v0.2.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; names 0.2 "the tag's MAJOR.MINOR"; names 0.1 "the repository's MAJOR.MINOR"
done_case
mk_dir "$tmp/i" Voicen_1.1.200_x64-setup.exe
call refuse-102-v1.1.200-on-0.1 missing "$token" v1.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; names 1.1 "the tag's MAJOR.MINOR"; names 0.1 "the repository's MAJOR.MINOR"
done_case
mk_dir "$tmp/i" Voicen_0.10.200_x64-setup.exe
call refuse-102-v0.10.200-on-0.1 missing "$token" v0.10.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; names 0.10 "the tag's MAJOR.MINOR"; names 0.1 "the repository's MAJOR.MINOR"
done_case
# The windows job's #102 failure branch: v<MAJOR>.<MINOR+1>.<run> on the stamped tree, no token.
mk_dir "$tmp/i" Voicen_0.1.100_x64-setup.exe
call refuse-102-failure-branch-stamped missing - v0.2.100 100 "$tmp/stamped" "$tmp/i"
want_rc 1; names 0.2 "the tag's MAJOR.MINOR"; names 0.1 "the repository's MAJOR.MINOR"
done_case

# #103: tag patch below this run's number.
mk_dir "$tmp/i" Voicen_0.1.99_x64-setup.exe
call refuse-103-v0.1.99-run-100 missing "$token" v0.1.99 100 "$tmp/repo" "$tmp/i"
want_rc 1; names 99 "the tag patch"; names 100 "the run number"
done_case
mk_dir "$tmp/i" Voicen_0.1.1_x64-setup.exe
call refuse-103-v0.1.1-run-100 missing "$token" v0.1.1 100 "$tmp/repo" "$tmp/i"
want_rc 1; names 100 "the run number"
done_case
# Numbers, not strings: 200 < 1000 although "200" > "1000".
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe
call refuse-103-v0.1.200-run-1000 missing "$token" v0.1.200 1000 "$tmp/repo" "$tmp/i"
want_rc 1; names 200 "the tag patch"; names 1000 "the run number"
done_case
# The windows job's #103 failure branch: v<MAJOR>.<MINOR>.<run-1> on the stamped tree, no token.
mk_dir "$tmp/i" Voicen_0.1.100_x64-setup.exe
call refuse-103-failure-branch-stamped missing - v0.1.99 100 "$tmp/stamped" "$tmp/i"
want_rc 1; names 99 "the tag patch"; names 100 "the run number"
done_case

# Run number: empty or not a decimal number. The tag patch 200 would pass any numeric reading.
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe
call refuse-run-number-empty missing "$token" v0.1.200 '' "$tmp/repo" "$tmp/i"
want_rc 1
grep -qiE -- 'run.?number' <<<"$out" || fail "the output does not name the run number"
done_case
# 0, a leading zero and 21 digits (beyond any integer test, so #103 could not refuse it) are
# refused by the run-number rule itself (validation 2 sweep).
for n in abc 1e2 0x64 -5 '10 0' 0 0100 123456789012345678901; do
  call "refuse-run-number-$n" missing "$token" v0.1.200 "$n" "$tmp/repo" "$tmp/i"
  want_rc 1; says "$n" "the bad run number"
  done_case
done

# The installer.
mk_dir "$tmp/i" Voicen_0.1.7_x64-setup.exe
call refuse-installer-0.1.7-under-v0.1.8 missing "$token" v0.1.8 5 "$tmp/repo" "$tmp/i"
want_rc 1; names 0.1.7 "the installer's version"; names 0.1.8 "the tag's version"
done_case
mk_dir "$tmp/i" Voicen_0.1.80_x64-setup.exe
call refuse-installer-0.1.80-under-v0.1.8 missing "$token" v0.1.8 5 "$tmp/repo" "$tmp/i"
want_rc 1
done_case
mk_dir "$tmp/i"
call refuse-no-installer missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; says "0 *-setup.exe" "the installer count (a refusal, not a crash)"
done_case
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe Voicen_0.1.200_x86-setup.exe
call refuse-two-installers missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1
done_case
# An installer whose name holds no X.Y.Z at all (validation 2 sweep).
mk_dir "$tmp/i" Voicen_x64-setup.exe
call refuse-installer-unversioned missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; says Voicen_x64-setup.exe "the installer"
done_case
call refuse-missing-installer-dir missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/no-such-dir"
want_rc 1
done_case

# The tag is not vX.Y.Z.
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe
for t in 0.1.200 v0.1 v0.1.200-rc.1 v0.01.200 v0.1.0200 refs/tags/v0.1.200 'v0.1.200 '; do
  call "refuse-tag-$t" missing "$token" "$t" 100 "$tmp/repo" "$tmp/i"
  want_rc 1; says "$t" "the bad tag"
  done_case
done
call refuse-tag-empty missing "$token" '' 100 "$tmp/repo" "$tmp/i"
want_rc 1
done_case
# Only the tag rule can refuse these: the installer carries the same version and every other rule
# would pass (validation 2 sweep: leading zero, and the 65535 bound).
mk_dir "$tmp/i" Voicen_0.1.0200_x64-setup.exe
call refuse-tag-leading-zero-only missing "$token" v0.1.0200 100 "$tmp/repo" "$tmp/i"
want_rc 1; says v0.1.0200 "the bad tag"
done_case
mk_dir "$tmp/i" Voicen_0.1.65536_x64-setup.exe
call refuse-tag-v0.1.65536 missing "$token" v0.1.65536 100 "$tmp/repo" "$tmp/i"
want_rc 1; says v0.1.65536 "the bad tag"
done_case
mk_dir "$tmp/i" Voicen_0.1.200_x64-setup.exe

# The committed version files disagree: no single MAJOR.MINOR to compare with.
call refuse-repo-files-disagree missing "$token" v0.1.200 100 "$tmp/disagree" "$tmp/i"
want_rc 1
done_case

# No version files under <root>: no MAJOR.MINOR, so #102 cannot be checked; never a pass.
mkdir -p "$tmp/empty-root"
call refuse-version-files-missing missing "$token" v0.1.200 100 "$tmp/empty-root" "$tmp/i"
want_rc 1
done_case

# The release already exists: never overwritten.
call refuse-release-exists exists "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; says v0.1.200 "the existing release's tag"; gh_viewed v0.1.200
done_case

# gh fails for another reason: whether the release exists is unknown, never a pass.
call refuse-gh-error error "$token" v0.1.200 100 "$tmp/repo" "$tmp/i"
want_rc 1; gh_viewed v0.1.200
done_case

# ---- usage ---------------------------------------------------------------------------------

call usage-three-arguments missing "$token" v0.1.200 100 "$tmp/repo"
want_rc 2
done_case
call usage-five-arguments missing "$token" v0.1.200 100 "$tmp/repo" "$tmp/i" extra
want_rc 2
done_case

if [ "$failed" != 0 ]; then
  echo "release-check.test: FAIL: $failed case(s) differ, $passed as expected (T-026, release job version check)" >&2
  exit 1
fi
echo "release-check.test: ok: $passed cases as expected"
