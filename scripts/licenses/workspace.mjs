// T-063: about.hbs leaves every Cargo.lock package without `source` out of
// THIRD-PARTY-NOTICES.txt, taking it to be one of the workspace's own crates. These helpers
// let a test hold that assumption: every sourceless package must be a workspace member.
import { readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * The package names of the workspace members listed in `<root>/Cargo.toml` `members`.
 * Throws, naming the entry, on a glob member (it is not expanded here) and on a member
 * manifest with no `[package]` name.
 * @param {string} root directory holding the workspace Cargo.toml
 * @returns {string[]}
 */
export function workspaceMemberNames(root) {
  const workspace = readFileSync(join(root, "Cargo.toml"), "utf8");
  const members = workspace.match(/^members\s*=\s*\[([^\]]*)\]/m);
  if (!members) throw new Error(`${join(root, "Cargo.toml")} has no [workspace] members list`);
  return [...members[1].matchAll(/"([^"]+)"/g)].map(([, dir]) => {
    if (/[*?[\]]/.test(dir)) {
      throw new Error(`workspace member "${dir}" is a glob; list the member directories explicitly`);
    }
    const manifest = readFileSync(join(root, dir, "Cargo.toml"), "utf8");
    const at = manifest.indexOf("[package]");
    const pkg = at < 0 ? null : manifest.slice(at).match(/^name\s*=\s*"([^"]+)"/m);
    if (!pkg) throw new Error(`workspace member ${dir}/Cargo.toml has no [package] name`);
    return pkg[1];
  });
}

/**
 * Names of the Cargo.lock `[[package]]` entries without a `source` (path crates, including
 * `[patch]` paths) whose name is not exactly one of `memberNames`, in lock order, each once.
 * @param {string} lockText contents of Cargo.lock
 * @param {Iterable<string>} memberNames package names of the workspace members
 * @returns {string[]}
 */
export function pathCratesOutsideWorkspace(lockText, memberNames) {
  const members = new Set(memberNames);
  const outside = new Set();
  for (const block of lockText.replace(/\r\n/g, "\n").split(/^\[\[package\]\]\s*$/m).slice(1)) {
    // The block ends at the next table header ([metadata], [[patch.unused]], ...).
    const body = block.split(/^\[/m)[0];
    const name = body.match(/^name\s*=\s*"([^"]+)"/m);
    if (!name || /^source\s*=/m.test(body)) continue;
    if (!members.has(name[1])) outside.add(name[1]);
  }
  return [...outside];
}
