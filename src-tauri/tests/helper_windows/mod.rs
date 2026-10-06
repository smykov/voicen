//! T-065 (rca smoke-window-predicate, F-008): the install smoke's helper-window rule for
//! the shell tests, read from the same manifest the smoke reads,
//! `scripts/ci/helper-windows.txt` (format and verdicts in its header; checked against
//! the shell's Windows dependency graph by `scripts/ci/helper-windows.sh` in make check).
//! `scripts/ci/visible-windows.ps1` (`Get-ShownWindows`) applies the same rule to the
//! installed voicen.exe; `smoke_predicate.rs` pins that both agree.
//!
//! A window is a helper only when its class is a `visible-helper` class of the manifest
//! (`{identifier}` = `src-tauri/tauri.conf.json` `identifier`) AND its extended style has
//! all four helper bits, `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE |
//! WS_EX_TOOLWINDOW`. Never by style alone (a click-through overlay has the same bits),
//! by size or by title; `hidden-helper`, `child` and `binding` entries exclude nothing.

use std::path::PathBuf;
use std::sync::OnceLock;

use windows::Win32::UI::WindowsAndMessaging::{
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// The four extended-style bits every visible helper of the manifest is created with.
const HELPER_BITS: u32 =
    WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;

/// Whether the smoke's predicate excludes a visible, unowned, top-level window of class
/// `class` with extended style `ex_style` as a framework helper window.
pub fn is_helper(class: &str, ex_style: u32) -> bool {
    ex_style & HELPER_BITS == HELPER_BITS && visible_helper_classes().iter().any(|c| c == class)
}

/// The repository root (`src-tauri/..`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The `visible-helper` classes of `scripts/ci/helper-windows.txt`, `{identifier}`
/// resolved; read once. Panics on a missing or malformed manifest (a list that cannot
/// be read never reads as "no helpers").
fn visible_helper_classes() -> &'static [String] {
    static CLASSES: OnceLock<Vec<String>> = OnceLock::new();
    CLASSES.get_or_init(|| {
        let path = repo_root()
            .join("scripts")
            .join("ci")
            .join("helper-windows.txt");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("premise: {} is readable: {e}", path.display()));
        let mut classes = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('|').map(str::trim).collect();
            let [_, verdict, class, _] = fields[..] else {
                panic!(
                    "{} line {}: want <crate>@<version>[<features>] | <verdict> | <class or -> | \
                     <citation>: {line}",
                    path.display(),
                    i + 1
                );
            };
            match verdict {
                "visible-helper" => {}
                "hidden-helper" | "child" | "binding" => continue,
                other => panic!(
                    "{} line {}: verdict `{other}` is not visible-helper, hidden-helper, \
                     child or binding",
                    path.display(),
                    i + 1
                ),
            }
            assert!(
                !class.is_empty() && class != "-",
                "{} line {}: a visible-helper entry names its class",
                path.display(),
                i + 1
            );
            classes.push(class.replace("{identifier}", &app_identifier()));
        }
        classes
    })
}

/// `identifier` of `src-tauri/tauri.conf.json`.
fn app_identifier() -> String {
    let path = repo_root().join("src-tauri").join("tauri.conf.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("premise: {} is readable: {e}", path.display()));
    let config: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("premise: {} is JSON: {e}", path.display()));
    config["identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("premise: {} has a string `identifier`", path.display()))
        .to_owned()
}
