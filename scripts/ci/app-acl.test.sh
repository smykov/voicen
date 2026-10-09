#!/usr/bin/env bash
# T-049 guard of the guard: scripts/ci/app-acl.sh must give its documented exit code on each
# case below (a shell dir built in a temp dir, so no committed fixture tree) and on the real
# src-tauri. Every case starts from one consistent shell (two commands, one per label) and
# changes one thing.
#   ok-*  exit 0 and "ok:";  v-*  exit 1 and the listing has the needle;  c-*  exit 3, "cannot run".
# Usage: scripts/ci/app-acl.test.sh   (host bash, python3)
# Exit 0: every case as expected. Exit 1: a case differs (listed). Exit 3: cannot run.
set -uo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root" || exit 3
guard=scripts/ci/app-acl.sh
[ -x "$guard" ] || { echo "app-acl.test: cannot run: $guard not found or not executable" >&2; exit 3; }
tmp="$(mktemp -d)" || { echo "app-acl.test: cannot run: mktemp failed" >&2; exit 3; }
trap 'rm -rf "$tmp"' EXIT

failed=0
passed=0

good_lib='/// A doc comment naming #[tauri::command] fn doc_only() is not a command.
#[tauri::command]
fn get_build_info() -> u32 { 1 }

// #[tauri::command] fn commented_out() {}
fn commands<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        get_build_info,
        settings_ipc::settings_save,
        overlay::overlay_ready
    ])
}'
good_settings='/// `settings_save { request }`.
#[tauri::command(async)]
pub fn settings_save(request: String) -> String { request }
const TEXT: &str = "#[tauri::command] fn in_a_string() {}";'
good_overlay='#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub async fn overlay_ready<R: Runtime>(app: AppHandle<R>) -> u32 { 0 }'
good_build='const APP_COMMANDS: [&str; 3] = [
    "get_build_info",
    "settings_save",
    // "removed_command",
    "overlay_ready",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&APP_COMMANDS)),
    )
    .expect("tauri-build");
}'
good_default='{
  "identifier": "default",
  "windows": ["settings"],
  "permissions": ["core:event:allow-listen", "core:window:allow-destroy",
                  "allow-get-build-info", { "identifier": "allow-settings-save" }]
}'
good_overlay_cap='{
  "identifier": "overlay",
  "windows": ["overlay"],
  "permissions": ["core:event:allow-listen", "allow-overlay-ready"]
}'
good_conf='{ "app": { "windows": [], "security": { "csp": null } } }'

# mk <case>: a consistent shell dir at $tmp/<case>; the case then edits its files.
mk() {
  local d="$tmp/$1"
  mkdir -p "$d/src" "$d/capabilities"
  printf '%s\n' "$good_lib" >"$d/src/lib.rs"
  printf '%s\n' "$good_settings" >"$d/src/settings_ipc.rs"
  printf '%s\n' "$good_overlay" >"$d/src/overlay.rs"
  printf '%s\n' "$good_build" >"$d/build.rs"
  printf '%s\n' "$good_default" >"$d/capabilities/default.json"
  printf '%s\n' "$good_overlay_cap" >"$d/capabilities/overlay.json"
  printf '%s\n' "$good_conf" >"$d/tauri.conf.json"
}
# sub <case> <file> <python regex> <replacement>: one substitution that must match.
sub() {
  python3 -I - "$tmp/$1/$2" "$3" "$4" <<'PY' || { echo "app-acl.test: cannot run: fixture edit $1/$2 failed" >&2; exit 3; }
import re, sys
path, pat, rep = sys.argv[1:4]
text = open(path, encoding="utf-8").read()
new, n = re.subn(pat, rep, text, count=1, flags=re.S)
if n != 1:
    sys.exit(f"pattern {pat!r} not found in {path}")
open(path, "w", encoding="utf-8").write(new)
PY
}
# check <case> <want exit> <needle> [dir]
check() {
  local c="$1" want="$2" needle="$3" dir="${4:-$tmp/$1}" out got
  out="$("$guard" "$dir" 2>&1)"
  got=$?
  if [ "$got" -ne "$want" ] || ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $c: exit $got (want $want), needle '$needle'" >&2
    sed 's/^/       | /' <<<"$out" >&2
    failed=$((failed + 1))
  else
    echo "ok   $c: exit $got"
    passed=$((passed + 1))
  fi
}

mk ok-consistent
check ok-consistent 0 "ok: 3 app commands"

# 1. C != H
mk v-command-not-registered
sub v-command-not-registered src/lib.rs 'fn get_build_info\(\) -> u32 \{ 1 \}' 'fn get_build_info() -> u32 { 1 }
#[tauri::command]
pub fn local_model_delete() {}'
check v-command-not-registered 1 "local_model_delete is not in generate_handler!"
mk v-registered-not-a-command
sub v-registered-not-a-command src/overlay.rs '#\[tauri::command\]\n' ''
check v-registered-not-a-command 1 "generate_handler! names overlay_ready, which is no #[tauri::command]"

# 2. build.rs does not hand APP_COMMANDS to tauri-build
mk v-plain-build
printf '%s\n' 'fn main() { tauri_build::build(); }' >"$tmp/v-plain-build/build.rs"
check v-plain-build 1 "no \`APP_COMMANDS\` array"
mk v-const-unused
sub v-const-unused build.rs 'tauri_build::try_build\(.*?\.expect\("tauri-build"\);' 'tauri_build::build();'
check v-const-unused 1 "no \`.app_manifest(..)\`"
mk v-const-in-comment-only
sub v-const-in-comment-only build.rs '\.commands\(&APP_COMMANDS\)' '.commands(&[]) /* .commands(&APP_COMMANDS) */'
check v-const-in-comment-only 1 "no \`.commands(&APP_COMMANDS)\`"

# 3. A != H
mk v-missing-from-app-commands
sub v-missing-from-app-commands build.rs '    "settings_save",\n' ''
check v-missing-from-app-commands 1 "registered command settings_save is not in APP_COMMANDS"
mk v-commented-entry-is-not-an-entry
sub v-commented-entry-is-not-an-entry build.rs '    "settings_save",\n' '    // "settings_save",\n'
check v-commented-entry-is-not-an-entry 1 "registered command settings_save is not in APP_COMMANDS"
mk v-app-commands-extra
sub v-app-commands-extra build.rs '"overlay_ready",' '"overlay_ready",
    "settings_gone",'
check v-app-commands-extra 1 "APP_COMMANDS names settings_gone, which generate_handler! does not register"

# 4. grants
mk v-not-granted
sub v-not-granted capabilities/default.json '"allow-get-build-info", ' ''
check v-not-granted 1 "registered command get_build_info is granted to no window (allow-get-build-info)"
mk v-object-grant-dropped
sub v-object-grant-dropped capabilities/default.json ', \{ "identifier": "allow-settings-save" \}' ''
check v-object-grant-dropped 1 "registered command settings_save is granted to no window"
mk v-orphan-grant
sub v-orphan-grant capabilities/overlay.json '"allow-overlay-ready"' '"allow-overlay-ready", "allow-settings-gone"'
check v-orphan-grant 1 "grants allow-settings-gone, but no registered command settings_gone exists"
mk ok-granted-twice
sub ok-granted-twice capabilities/overlay.json '"allow-overlay-ready"' '"allow-overlay-ready", "allow-get-build-info"'
check ok-granted-twice 0 "ok:"

# 5. deny-* and non-allow app permissions
mk v-deny
sub v-deny capabilities/overlay.json '"allow-overlay-ready"' '"allow-overlay-ready", "deny-settings-save"'
check v-deny 1 "app permission deny-settings-save: a deny ignores window labels"
mk v-app-set
sub v-app-set capabilities/default.json '"allow-get-build-info", ' '"allow-get-build-info", "settings-window", '
check v-app-set 1 "app permission settings-window is not allow-<command>"
mk ok-plugin-deny-is-not-app
sub ok-plugin-deny-is-not-app capabilities/overlay.json '"allow-overlay-ready"' '"allow-overlay-ready", "core:window:deny-destroy"'
check ok-plugin-deny-is-not-app 0 "ok:"

# 6. wildcard labels and remote contexts
mk v-window-wildcard
sub v-window-wildcard capabilities/default.json '\["settings"\]' '["settings", "*"]'
check v-window-wildcard 1 "grants app permissions to windows pattern '*'"
mk v-webview-wildcard
sub v-webview-wildcard capabilities/overlay.json '"windows": \["overlay"\],' '"windows": ["overlay"], "webviews": ["over*"],'
check v-webview-wildcard 1 "grants app permissions to webviews pattern 'over*'"
mk v-remote
sub v-remote capabilities/overlay.json '"windows": \["overlay"\],' '"windows": ["overlay"], "remote": { "urls": ["https://example.com"] },'
check v-remote 1 "grants app permissions to a remote context"

# 7. inline capabilities, hand-written permission files
mk v-inline-capability
printf '%s\n' '{ "app": { "security": { "capabilities": ["default", { "identifier": "x", "windows": ["*"], "permissions": ["allow-settings-save"] }] } } }' >"$tmp/v-inline-capability/tauri.conf.json"
check v-inline-capability 1 "an inline capability in app.security.capabilities"
mk v-hand-written-permission
mkdir -p "$tmp/v-hand-written-permission/permissions"
printf '%s\n' '[[set]]' 'identifier = "settings-window"' 'permissions = ["allow-settings-save"]' >"$tmp/v-hand-written-permission/permissions/sets.toml"
check v-hand-written-permission 1 "permissions/sets.toml: a hand-written permission file"
mk ok-autogenerated-permissions
mkdir -p "$tmp/ok-autogenerated-permissions/permissions/autogenerated"
printf '%s\n' '[[permission]]' 'identifier = "allow-settings-save"' 'commands.allow = ["settings_save"]' >"$tmp/ok-autogenerated-permissions/permissions/autogenerated/settings_save.toml"
check ok-autogenerated-permissions 0 "ok:"

# 8. malformed capability
mk v-bad-json
printf '%s\n' '{ "identifier": "default", ' >"$tmp/v-bad-json/capabilities/default.json"
check v-bad-json 1 "default.json: not valid JSON"
mk v-no-permissions
printf '%s\n' '{ "identifier": "default", "windows": ["settings"] }' >"$tmp/v-no-permissions/capabilities/default.json"
check v-no-permissions 1 "default.json: no \`permissions\` array"

# cannot run
check c-no-dir 3 "cannot run" "$tmp/c-no-dir/absent"
mk c-no-build
rm "$tmp/c-no-build/build.rs"
check c-no-build 3 "cannot run"

# The real shell with the guard's default.
out="$("$guard" 2>&1)"
got=$?
if [ "$got" -eq 0 ] && grep -qF "ok:" <<<"$out"; then
  echo "ok   real repo: exit 0"
  passed=$((passed + 1))
else
  echo "FAIL real repo: exit $got (want 0)" >&2
  sed 's/^/       | /' <<<"$out" >&2
  failed=$((failed + 1))
fi

echo "app-acl.test: $passed passed, $failed failed"
[ "$failed" -eq 0 ] || exit 1
