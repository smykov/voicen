#!/usr/bin/env bash
# T-089 (P-020, decides_on: token + literal): host test of scripts/ci/clippy-lines.sh, the
# token guard that keeps the voicen-core call bans (crates/voicen-core/clippy.toml, the
# [lints.clippy] deny of crates/voicen-core/Cargo.toml, proven by make check-ban-canary) from
# being lowered in the source. Every way left to lower a denied lint spells the token clippy
# (allow / expect of the lint, of clippy::all or a group, a renamed lint, a -A flag in a cargo
# config, a config dir set by a build script; docs/tasks/T-089.md Investigation §4, §12), so:
#
# Contract of clippy-lines.sh [root]   (default root: the repository)
#   Scanned: every file `git -C <root> ls-files` lists whose name ends in .rs, whose base name is
#     Cargo.toml, or that is config or config.toml in a .cargo directory. Comments are lines like
#     any other (no // skip); the guard reads no Rust or TOML syntax.
#   Token: a scanned line holds the token when it contains clippy in any case.
#   Owned: <root>/scripts/ci/clippy-lines.txt; lines starting with # and blank lines are not
#     entries; an entry is <path> TAB <text>. A token line is owned when, with leading and
#     trailing blanks removed, it is byte-equal to the <text> of an entry whose <path> is the
#     line's file (relative to root) or *.
#     - An entry bound to a path owns one line: a file must hold exactly as many lines with that
#       text as there are entries for it (one more line is not owned, one fewer is a stale entry).
#     - A * entry owns the text in any file, any number of times, but must match at least one
#       line, and its text must not contain disallowed in any case (an exemption from the bans
#       is always bound to its file).
#   Exit 0: "clippy-lines: ok: <n> line(s) owned"; n pins that lines were read.
#   Exit 1: one "<file>:<line>: clippy: ..." per line that is not owned, one
#     "scripts/ci/clippy-lines.txt:<n>: ..." per stale or refused entry.
#   Exit 3: "cannot run": root not a git work tree, no owned list, no scanned file, a file
#     unreadable. Never a pass.
#
# Cases are trees built in a temp dir from one base shaped like the repository (each a fresh git
# repository with every file added): ok-* exit 0 and "ok:"; v-* exit 1 and every needle (a
# needle starting with / is <file>:<line> relative to the case tree's root); c-* exit 3 and
# "cannot run". Then the real repository must pass (exit 0): red until T-089 lands the owned
# list (the three exemption lines, the [lints.clippy] line, the existing deny and allow lines).
# Usage: scripts/ci/clippy-lines.test.sh   (host bash, git)
# Exit 0: every case as expected. Exit 1: a case differs (listed), or the guard is missing.
# Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard="$root/scripts/ci/clippy-lines.sh"
command -v git >/dev/null 2>&1 || { echo "clippy-lines.test: cannot run: git not found" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "clippy-lines.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0

deny='#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]'
tab="$(printf '\t')"

# base <dir>: the repository's shape, every token line owned.
base() {
  local d="$1"
  mkdir -p "$d/scripts/ci" "$d/.cargo" "$d/crates/voicen-core/src/engine" \
    "$d/crates/voicen-core/tests/common" "$d/src-tauri/src" "$d/docs"
  cat >"$d/scripts/ci/clippy-lines.txt" <<EOF
# Owned lines holding the token clippy (T-089). <path> TAB <text>; * = any file.
crates/voicen-core/src/engine/http.rs${tab}#![allow(clippy::disallowed_types)]
crates/voicen-core/src/clock.rs${tab}#[allow(clippy::disallowed_methods)]
crates/voicen-core/src/clock.rs${tab}#[allow(clippy::disallowed_methods)]
crates/voicen-core/tests/common/mod.rs${tab}#![allow(dead_code, clippy::disallowed_methods)]
crates/voicen-core/Cargo.toml${tab}[lints.clippy]

*${tab}${deny}
*${tab}#[allow(clippy::too_many_arguments)]
EOF
  cat >"$d/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/voicen-core", "src-tauri"]
EOF
  cat >"$d/.cargo/config.toml" <<'EOF'
[env]
AWS_LC_SYS_PREBUILT_NASM = "1"
EOF
  cat >"$d/crates/voicen-core/Cargo.toml" <<'EOF'
[package]
name = "voicen-core"

[lints.clippy]
disallowed_methods = "deny"
disallowed_types = "deny"
EOF
  cat >"$d/crates/voicen-core/src/lib.rs" <<'EOF'
//! Platform-independent core of Voicen.
pub mod clock;
pub mod engine;
pub mod pipeline;
EOF
  cat >"$d/crates/voicen-core/src/engine/http.rs" <<EOF
//! The one place a reqwest client is built.
$deny
#![allow(clippy::disallowed_types)]

pub fn client() {}
EOF
  cat >"$d/crates/voicen-core/src/engine/openai.rs" <<EOF
//! The transcription engine.
$deny

pub fn transcribe() {}
EOF
  cat >"$d/crates/voicen-core/src/clock.rs" <<'EOF'
use std::time::{Duration, Instant};

#[allow(clippy::disallowed_methods)]
pub(crate) fn instant() -> Instant {
    Instant::now()
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn since(t: Instant) -> Duration {
    t.elapsed()
}
EOF
  cat >"$d/crates/voicen-core/src/pipeline.rs" <<EOF
//! The dictation pipeline.
$deny

pub fn run() {}
EOF
  cat >"$d/crates/voicen-core/tests/common/mod.rs" <<'EOF'
//! Shared test helpers.
#![allow(dead_code, clippy::disallowed_methods)]

pub mod timing;
EOF
  cat >"$d/crates/voicen-core/tests/diag_observer.rs" <<'EOF'
mod common;

    #[allow(clippy::too_many_arguments)]
    fn record(_a: u8, _b: u8, _c: u8, _d: u8, _e: u8, _f: u8, _g: u8, _h: u8) {}
EOF
  cat >"$d/src-tauri/Cargo.toml" <<'EOF'
[package]
name = "voicen"
EOF
  printf '#![cfg(windows)]\npub fn run() {}\n' >"$d/src-tauri/src/lib.rs"
  printf 'The gate runs clippy with -D warnings.\n' >"$d/docs/README.md"
}

# mk <case>: a fresh base tree under $tmp/<case>; the caller edits it, then commit_tree adds it.
mk() { base "$tmp/$1"; }
commit_tree() { git -C "$tmp/$1" init -q && git -C "$tmp/$1" add -A; }

# run <label> <expected exit> <dir> <needle>...: every needle must be in the output.
run() {
  local label="$1" want="$2" dir="$3" out got n miss=""
  shift 3
  if [ ! -x "$guard" ]; then
    echo "FAIL $label: $guard not found or not executable (the guard does not exist)" >&2
    failed=$((failed + 1))
    return
  fi
  out="$("$guard" "$dir" 2>&1)"
  got=$?
  for n in "$@"; do
    grep -qF -- "$n" <<<"$out" || miss="$miss [$n]"
  done
  if [ "$got" -ne "$want" ]; then
    echo "FAIL $label: exit $got, want $want" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  elif [ -n "$miss" ]; then
    echo "FAIL $label: exit $got as wanted, but the output lacks:$miss" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $label: exit $got"
    passed=$((passed + 1))
  fi
}

# check <case> <expected exit> <needle>...: a needle starting with / is <file>:<line> under the
# case root, followed by ": clippy" unless it names the owned list.
check() {
  local c="$1" want="$2" n
  local needles=()
  shift 2
  for n in "$@"; do
    case "$n" in
      /scripts/ci/clippy-lines.txt:*) needles+=("${n#/}") ;;
      /*) needles+=("${n#/}: clippy") ;;
      *) needles+=("$n") ;;
    esac
  done
  commit_tree "$c" || { echo "FAIL $c: could not build the case tree" >&2; failed=$((failed + 1)); return; }
  run "$c" "$want" "$tmp/$c" "${needles[@]}"
}

V=crates/voicen-core

# Allowed.
mk ok-base
check ok-base 0 "ok: 9 line(s) owned"   # 4 exemption allows, [lints.clippy], 3 deny lines, 1 allow

mk ok-general-line-in-new-file   # a * line (deny) in a new module needs no list edit
printf '//! Settings.\n%s\n\npub fn load() {}\n' "$deny" >"$tmp/ok-general-line-in-new-file/$V/src/settings.rs"
check ok-general-line-in-new-file 0 "ok: 10 line(s) owned"

mk ok-token-outside-scanned-files   # not .rs, Cargo.toml or a .cargo config: not read
printf 'cargo clippy -- -A clippy::all\n' >"$tmp/ok-token-outside-scanned-files/scripts/ci/lint.sh"
printf 'avoid-breaking-exported-api = false # clippy\n' >"$tmp/ok-token-outside-scanned-files/$V/clippy.toml"
printf '[clippy]\nall = "allow"\n' >"$tmp/ok-token-outside-scanned-files/$V/Cargo.toml.orig"
check ok-token-outside-scanned-files 0 "ok: 9 line(s) owned"

mk ok-indented-owned-line   # leading and trailing blanks are not part of the text
sed -i 's/^#!\[allow(clippy::disallowed_types)\]$/    #![allow(clippy::disallowed_types)]  /' \
  "$tmp/ok-indented-owned-line/$V/src/engine/http.rs"
check ok-indented-owned-line 0 "ok: 9 line(s) owned"

# The token on a line that is not owned.
mk v-allow-all-in-test-file   # silences every clippy lint of the test crate, the bans included
printf 'mod common;\n\n#[allow(clippy::all)]\nfn probe() {}\n' >"$tmp/v-allow-all-in-test-file/$V/tests/local_server.rs"
check v-allow-all-in-test-file 1 "/$V/tests/local_server.rs:3"

mk v-exemption-in-non-owned-src   # the clock's own allow, in a file that does not own it
printf '//! The pipeline.\n#![allow(clippy::disallowed_methods)]\n' >"$tmp/v-exemption-in-non-owned-src/$V/src/pipeline.rs"
check v-exemption-in-non-owned-src 1 "/$V/src/pipeline.rs:2"

mk v-exemption-in-comment   # comments are lines like any other
printf '//! The pipeline.\n// #![allow(clippy::disallowed_methods)]\npub fn run() {}\n' \
  >"$tmp/v-exemption-in-comment/$V/src/pipeline.rs"
check v-exemption-in-comment 1 "/$V/src/pipeline.rs:2"

mk v-comment-in-owned-file   # an owned file owns its listed text, not every line naming the lint
printf '//! Built here only; see #![allow(clippy::disallowed_types)] below.\n' >"$tmp/x.rs"
cat "$tmp/v-comment-in-owned-file/$V/src/engine/http.rs" >>"$tmp/x.rs"
mv "$tmp/x.rs" "$tmp/v-comment-in-owned-file/$V/src/engine/http.rs"
check v-comment-in-owned-file 1 "/$V/src/engine/http.rs:1"

mk v-lint-name-case   # an unknown lint to rustc, and still the token
printf '//! The pipeline.\n#![allow(clippy::Disallowed_Methods)]\n' >"$tmp/v-lint-name-case/$V/src/pipeline.rs"
check v-lint-name-case 1 "/$V/src/pipeline.rs:2"

mk v-token-upper-case   # any case
printf '//! The pipeline.\n// CLIPPY: lower the ban here later\n' >"$tmp/v-token-upper-case/$V/src/pipeline.rs"
check v-token-upper-case 1 "/$V/src/pipeline.rs:2"

mk v-extra-copy-in-owned-file   # clock.rs owns two allow lines, not three
printf '\n#[allow(clippy::disallowed_methods)]\npub fn third() {}\n' >>"$tmp/v-extra-copy-in-owned-file/$V/src/clock.rs"
check v-extra-copy-in-owned-file 1 "$V/src/clock.rs:"

mk v-owned-line-moved   # http.rs's exemption moved into the engine: not owned there, stale here
sed -i '/^#!\[allow(clippy::disallowed_types)\]$/d' "$tmp/v-owned-line-moved/$V/src/engine/http.rs"
printf '#![allow(clippy::disallowed_types)]\n' >>"$tmp/v-owned-line-moved/$V/src/engine/openai.rs"
check v-owned-line-moved 1 "/$V/src/engine/openai.rs:5" "/scripts/ci/clippy-lines.txt:2"

mk v-owned-line-in-a-same-named-file   # bound to the path, not to the file name: moved to another http.rs
sed -i '/^#!\[allow(clippy::disallowed_types)\]$/d' "$tmp/v-owned-line-in-a-same-named-file/$V/src/engine/http.rs"
mkdir -p "$tmp/v-owned-line-in-a-same-named-file/$V/src/local_models"
printf '//! Model download.\n#![allow(clippy::disallowed_types)]\n' \
  >"$tmp/v-owned-line-in-a-same-named-file/$V/src/local_models/http.rs"
check v-owned-line-in-a-same-named-file 1 "/$V/src/local_models/http.rs:2" "/scripts/ci/clippy-lines.txt:2"

mk v-cargo-toml-of-another-package   # [lints.clippy] is owned in voicen-core's manifest only
printf '\n[lints.clippy]\nall = "allow"\n' >>"$tmp/v-cargo-toml-of-another-package/src-tauri/Cargo.toml"
check v-cargo-toml-of-another-package 1 "/src-tauri/Cargo.toml:4"

mk v-cargo-toml-lint-key   # a lint set through the dotted key of [lints]
printf '\n[lints]\nclippy.disallowed_methods = "allow"\n' >>"$tmp/v-cargo-toml-lint-key/Cargo.toml"
check v-cargo-toml-lint-key 1 "/Cargo.toml:5"

mk v-cargo-config-rustflags   # a flag for every crate the workspace builds
printf '\n[build]\nrustflags = ["-Aclippy::disallowed_methods"]\n' >>"$tmp/v-cargo-config-rustflags/.cargo/config.toml"
check v-cargo-config-rustflags 1 "/.cargo/config.toml:5"

mk v-cargo-config-legacy-name   # cargo still reads .cargo/config without .toml
mkdir -p "$tmp/v-cargo-config-legacy-name/$V/.cargo"
printf '[target.x86_64-unknown-linux-gnu]\nrustflags = ["-A", "clippy::all"]\n' \
  >"$tmp/v-cargo-config-legacy-name/$V/.cargo/config"
check v-cargo-config-legacy-name 1 "/$V/.cargo/config:2"

mk v-build-script-config-dir   # a build script pointing the lint run at another config
printf 'fn main() {\n    println!("cargo::rustc-env=CLIPPY_CONF_DIR=/tmp");\n}\n' >"$tmp/v-build-script-config-dir/$V/build.rs"
check v-build-script-config-dir 1 "/$V/build.rs:2"

mk v-shell-file   # the whole repository is read, not only voicen-core
printf '#![cfg(windows)]\n#![allow(clippy::all)]\npub fn run() {}\n' >"$tmp/v-shell-file/src-tauri/src/lib.rs"
check v-shell-file 1 "/src-tauri/src/lib.rs:2"

# The owned list itself.
mk v-stale-path-entry   # an entry for a line that is not there
printf 'crates/voicen-core/src/pipeline.rs\t#[allow(clippy::large_enum_variant)]\n' \
  >>"$tmp/v-stale-path-entry/scripts/ci/clippy-lines.txt"
check v-stale-path-entry 1 "/scripts/ci/clippy-lines.txt:10"

mk v-stale-general-entry   # a * entry no line uses
printf '*\t#[allow(clippy::large_enum_variant)]\n' >>"$tmp/v-stale-general-entry/scripts/ci/clippy-lines.txt"
check v-stale-general-entry 1 "/scripts/ci/clippy-lines.txt:10"

mk v-general-entry-names-a-ban   # an exemption from the bans is always bound to its file
printf '*\t#![allow(clippy::disallowed_methods)]\n' >>"$tmp/v-general-entry-names-a-ban/scripts/ci/clippy-lines.txt"
printf '//! The pipeline.\n#![allow(clippy::disallowed_methods)]\n' >"$tmp/v-general-entry-names-a-ban/$V/src/pipeline.rs"
check v-general-entry-names-a-ban 1 "/scripts/ci/clippy-lines.txt:10"

# Cannot run.
mk c-no-list
rm -f "$tmp/c-no-list/scripts/ci/clippy-lines.txt"
commit_tree c-no-list && run c-no-list 3 "$tmp/c-no-list" "cannot run"

mk c-not-a-git-tree   # tracked files cannot be listed: never a pass
run c-not-a-git-tree 3 "$tmp/c-not-a-git-tree" "cannot run"

mkdir -p "$tmp/c-no-scanned-file/scripts/ci" && cp "$tmp/ok-base/scripts/ci/clippy-lines.txt" "$tmp/c-no-scanned-file/scripts/ci/"
printf 'notes\n' >"$tmp/c-no-scanned-file/README.md"
commit_tree c-no-scanned-file && run c-no-scanned-file 3 "$tmp/c-no-scanned-file" "cannot run"

# The real repository (the guard's default root).
if [ -x "$guard" ]; then
  out="$("$guard" 2>&1)"
  got=$?
  if [ "$got" -eq 0 ] && grep -qF "ok:" <<<"$out"; then
    echo "ok   real repository: exit 0"
    passed=$((passed + 1))
  else
    echo "FAIL real repository: exit $got, want 0" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  fi
else
  echo "FAIL real repository: $guard not found or not executable (the guard does not exist)" >&2
  failed=$((failed + 1))
fi

if [ "$failed" -gt 0 ]; then
  echo "clippy-lines.test: FAIL: $failed case(s) differ, $passed as expected (T-089)" >&2
  exit 1
fi
echo "clippy-lines.test: ok: $passed cases as expected"
