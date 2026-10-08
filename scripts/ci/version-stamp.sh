#!/usr/bin/env bash
# T-077 (decisions #98): the one place CI computes and writes the build version. The windows job
# of .github/workflows/ci.yml calls it in one bash step before its first cargo command, so every
# compile, the NSIS installer (tauri.conf.json version) and the Telegram caption of the run carry
# one version V. The stamped tree is never committed; the gate job does not stamp (its --locked
# cargo calls run on the committed tree) and runs scripts/ci/version-stamp.test.sh instead.
#
# Contract:
#   - Reads [workspace.package] version of <root>/Cargo.toml and the top-level "version" of
#     <root>/src-tauri/tauri.conf.json and <root>/package.json. Each must be X.Y.Z (decimal, no
#     leading zeros, each part <= 65535) and all three equal; else exit 1 naming file and value.
#   - ref_type tag: ref_name must be vX.Y.Z (same rules), else exit 1 naming the tag; V = X.Y.Z,
#     the run number is not used. Any other ref_type (a branch, a pull request):
#     V = <repo MAJOR>.<repo MINOR>.<run_number>, run_number [1-9][0-9]* and <= 65535 (Windows
#     VS_FIXEDFILEINFO / NSIS VIProductVersion fields are 16-bit), else exit 1 naming it.
#   - Writes V into the three fields and into the version of the two workspace members of
#     <root>/Cargo.lock ([[package]] voicen and voicen-core, no source line), so the stamped tree
#     still passes --locked; no other line changes (CRLF line ends are kept). Then re-reads all
#     five places, prints "version: V" and appends "VOICEN_VERSION=V" to $GITHUB_ENV when set.
#   - Nothing is written unless every input is valid: the new files are built in a temp dir
#     first, and $GITHUB_ENV is appended only after the re-read.
# Inputs come from the workflow through env: (github.ref_type, github.ref_name,
# github.run_number), never interpolated into the step's script text.
#
# Usage: scripts/ci/version-stamp.sh <ref_type> <ref_name> <run_number> [root]   (root: ".")
# Exit 0: stamped. Exit 1: an input cannot give a valid V (nothing written). Exit 2: wrong usage.
set -uo pipefail
if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "usage: version-stamp.sh <ref_type> <ref_name> <run_number> [root]" >&2
  exit 2
fi
ref_type="$1"
ref_name="$2"
run_number="$3"
root="${4:-.}"

die() { echo "::error::version-stamp: $*" >&2; exit 1; }

cargo_toml="Cargo.toml"
tauri_conf="src-tauri/tauri.conf.json"
package_json="package.json"
cargo_lock="Cargo.lock"
for f in "$cargo_toml" "$tauri_conf" "$package_json" "$cargo_lock"; do
  [ -f "$root/$f" ] || die "$root/$f not found"
done

# A number part of a version: decimal, no leading zero, <= 65535.
part_ok() { [[ "$1" =~ ^(0|[1-9][0-9]{0,4})$ ]] && [ "$1" -le 65535 ]; }
# X.Y.Z with three valid parts.
version_ok() {
  local x y z
  [[ "$1" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || return 1
  x="${BASH_REMATCH[1]}" y="${BASH_REMATCH[2]}" z="${BASH_REMATCH[3]}"
  part_ok "$x" && part_ok "$y" && part_ok "$z"
}

# toml_version <mode> <file> [new]: [workspace.package] version of a Cargo.toml.
# mode read: prints the value. mode write: prints the file with the value replaced by new.
# Exit 3 when the field is not found exactly once.
toml_version() {
  awk -v mode="$1" -v new="${3:-}" '
    { line = $0; cr = ""; if (sub(/\r$/, "", line)) cr = "\r" }
    line ~ /^[ \t]*\[/ {
      sec = line; sub(/^[ \t]*/, "", sec); sub(/[ \t]*(#.*)?$/, "", sec)
      insec = (sec == "[workspace.package]")
    }
    insec && line ~ /^[ \t]*version[ \t]*=[ \t]*"[^"]*"[ \t]*(#.*)?$/ {
      n++
      match(line, /^[ \t]*version[ \t]*=[ \t]*"/); pre = substr(line, 1, RLENGTH)
      rest = substr(line, RLENGTH + 1); q = index(rest, "\"")
      val = substr(rest, 1, q - 1)
      if (mode == "write") line = pre new substr(rest, q)
    }
    mode == "write" { printf "%s%s\n", line, cr }
    END { if (n != 1) exit 3; if (mode == "read") print val }
  ' "$2"
}

# json_version <mode> <file> [new]: the top-level "version" of a JSON file whose top-level
# members each start on their own line (depth counted outside strings).
json_version() {
  awk -v mode="$1" -v new="${3:-}" '
    { line = $0; cr = ""; if (sub(/\r$/, "", line)) cr = "\r" }
    {
      if (depth == 1 && line ~ /^[ \t]*"version"[ \t]*:[ \t]*"[^"\\]*"[ \t]*,?[ \t]*$/) {
        n++
        match(line, /^[ \t]*"version"[ \t]*:[ \t]*"/); pre = substr(line, 1, RLENGTH)
        rest = substr(line, RLENGTH + 1); q = index(rest, "\"")
        val = substr(rest, 1, q - 1)
        if (mode == "write") line = pre new substr(rest, q)
      }
      instr = 0
      for (i = 1; i <= length(line); i++) {
        c = substr(line, i, 1)
        if (instr) { if (c == "\\") i++; else if (c == "\"") instr = 0 }
        else if (c == "\"") instr = 1
        else if (c == "{" || c == "[") depth++
        else if (c == "}" || c == "]") depth--
      }
    }
    mode == "write" { printf "%s%s\n", line, cr }
    END { if (n != 1) exit 3; if (mode == "read") print val }
  ' "$2"
}

# lock_version <mode> <file> [new]: the versions of the workspace members voicen and voicen-core
# in Cargo.lock ([[package]] blocks without a source line). mode read: prints "name value" per
# member. Exit 3 unless each member is found exactly once.
lock_version() {
  awk -v mode="$1" -v new="${3:-}" '
    FNR == NR {
      line = $0; sub(/\r$/, "", line)
      if (line ~ /^\[/) { b++; pkg[b] = (line == "[[package]]") }
      else if (line ~ /^name = "/) { v = line; sub(/^name = "/, "", v); sub(/".*/, "", v); name[b] = v }
      else if (line ~ /^source = /) src[b] = 1
      else if (line ~ /^version = "[^"]*"$/) { v = line; sub(/^version = "/, "", v); sub(/"$/, "", v); vline[b] = FNR; vval[b] = v }
      next
    }
    FNR == 1 { targets() }
    {
      line = $0; cr = ""; if (sub(/\r$/, "", line)) cr = "\r"
      if (FNR in target) line = "version = \"" new "\""
      printf "%s%s\n", line, cr
    }
    function targets(   i) {
      if (done) return; done = 1
      for (i = 1; i <= b; i++)
        if (pkg[i] && !src[i] && (name[i] == "voicen" || name[i] == "voicen-core") && vline[i]) {
          seen[name[i]]++; target[vline[i]] = 1
          if (mode == "read") print name[i], vval[i]
        }
      if (seen["voicen"] != 1 || seen["voicen-core"] != 1) { bad = 1; exit 3 }
      if (mode == "read") exit 0
    }
    END { if (bad) exit 3; if (!done) { targets(); if (bad) exit 3 } }
  ' "$2" "$2"
}

# ---- read and check the repository version ---------------------------------------------------

cargo_ver="$(toml_version read "$root/$cargo_toml")" || die "$cargo_toml: no single [workspace.package] version found"
tauri_ver="$(json_version read "$root/$tauri_conf")" || die "$tauri_conf: no single top-level \"version\" found"
package_ver="$(json_version read "$root/$package_json")" || die "$package_json: no single top-level \"version\" found"
lock_version read "$root/$cargo_lock" > /dev/null || die "$cargo_lock: the workspace members voicen and voicen-core not found once each"

version_ok "$cargo_ver" || die "$cargo_toml: [workspace.package] version '$cargo_ver' is not X.Y.Z (no leading zeros, each part <= 65535)"
for pair in "$tauri_conf=$tauri_ver" "$package_json=$package_ver"; do
  f="${pair%%=*}" v="${pair#*=}"
  version_ok "$v" || die "$f: version '$v' is not X.Y.Z (no leading zeros, each part <= 65535)"
  [ "$v" = "$cargo_ver" ] || die "$f: version '$v' differs from $cargo_toml [workspace.package] version '$cargo_ver'; the three must agree"
done

# ---- compute V -------------------------------------------------------------------------------

if [ "$ref_type" = tag ]; then
  if [[ "$ref_name" =~ ^v(.*)$ ]] && version_ok "${BASH_REMATCH[1]}"; then
    V="${ref_name#v}"
  else
    die "tag '$ref_name' is not vX.Y.Z (no leading zeros, each part <= 65535)"
  fi
else
  [ -n "$run_number" ] || die "run number is empty"
  part_ok "$run_number" && [ "$run_number" != 0 ] ||
    die "run number '$run_number' is not a decimal number from 1 to 65535"
  V="${cargo_ver%.*}.$run_number"
fi

# ---- write: build every new file first, then replace ----------------------------------------

tmp="$(mktemp -d)" || die "mktemp failed"
trap 'rm -rf "$tmp"' EXIT
toml_version write "$root/$cargo_toml" "$V" > "$tmp/cargo_toml" || die "$cargo_toml: cannot stamp"
json_version write "$root/$tauri_conf" "$V" > "$tmp/tauri_conf" || die "$tauri_conf: cannot stamp"
json_version write "$root/$package_json" "$V" > "$tmp/package_json" || die "$package_json: cannot stamp"
lock_version write "$root/$cargo_lock" "$V" > "$tmp/cargo_lock" || die "$cargo_lock: cannot stamp"
cat "$tmp/cargo_toml" > "$root/$cargo_toml" &&
  cat "$tmp/tauri_conf" > "$root/$tauri_conf" &&
  cat "$tmp/package_json" > "$root/$package_json" &&
  cat "$tmp/cargo_lock" > "$root/$cargo_lock" || die "cannot write the stamped files under $root"

# ---- re-read the five places -----------------------------------------------------------------

got="$(toml_version read "$root/$cargo_toml")" && [ "$got" = "$V" ] || die "$cargo_toml: reads '$got' after stamping, want '$V'"
for f in "$tauri_conf" "$package_json"; do
  got="$(json_version read "$root/$f")" && [ "$got" = "$V" ] || die "$f: reads '$got' after stamping, want '$V'"
done
got="$(lock_version read "$root/$cargo_lock" | sort)" &&
  [ "$got" = "$(printf 'voicen %s\nvoicen-core %s' "$V" "$V")" ] ||
  die "$cargo_lock: workspace members read '$(tr '\n' ' ' <<<"$got")' after stamping, want '$V'"

echo "version: $V"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "VOICEN_VERSION=$V" >> "$GITHUB_ENV" || die "cannot append VOICEN_VERSION to \$GITHUB_ENV"
fi
