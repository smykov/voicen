//! T-057: the install smoke's "shown window" predicate
//! (`scripts/ci/visible-windows.ps1`, `Get-ShownWindows`), run through `pwsh` on
//! windows this test exe creates. Windows CI only (decision #5); analysis red-test
//! table row 7 and Acceptance 2 ("a tool window shown at start makes the install
//! smoke fail").
//!
//! The smoke's loaded-launch step fails when `Get-ShownWindows` returns any window
//! (and the first-launch step wants exactly the settings window), so "the smoke
//! fails on a tool window shown at start" is exactly "`Get-ShownWindows` returns a
//! visible, unowned, top-level tool window". F-003: the predicate decides on raw
//! window facts (visible, owner, top-level, the class of tao's event-target window);
//! no "it is our overlay" exception by class or title. The windows here are EDIT
//! windows with an obviously fake title, so an exception keyed to the overlay's
//! class or title cannot make the red test pass.
//!
//! Precondition: `pwsh` (PowerShell 7) on PATH, as on the windows-latest runner the
//! smoke itself runs on. tao's event-target window (still excluded) needs a tauri
//! process; the smoke's loaded-launch step pins that exclusion (a release voicen.exe
//! always has that window, visible).
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, TranslateMessage, MSG,
    PM_REMOVE, WINDOW_EX_STYLE, WINDOW_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};

/// How long pwsh may take (its `Add-Type` compiles C# on every start; F-005).
const PWSH_BUDGET: Duration = Duration::from_secs(120);

/// One window to create: extended style, style, and whether the first window of
/// the set owns it.
#[derive(Clone, Copy)]
struct Spec {
    ex: WINDOW_EX_STYLE,
    style: WINDOW_STYLE,
    owned: bool,
}

/// Windows of `specs`, created and pumped on their own thread until dropped.
struct Windows {
    hwnds: Vec<isize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Windows {
    #[track_caller]
    fn open(specs: Vec<Spec>) -> Windows {
        let (tx, rx) = mpsc::channel::<Result<Vec<isize>, String>>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut made: Vec<HWND> = Vec::new();
            for spec in &specs {
                let owner = if spec.owned {
                    made.first().copied()
                } else {
                    None
                };
                // SAFETY: the system EDIT class, a static title, an owner (if any) of this
                // thread.
                let created = unsafe {
                    CreateWindowExW(
                        spec.ex,
                        w!("EDIT"),
                        w!("t057 smoke predicate probe (fake)"),
                        spec.style,
                        10,
                        10,
                        200,
                        60,
                        owner,
                        None,
                        None,
                        None,
                    )
                };
                match created {
                    Ok(h) => made.push(h),
                    Err(err) => {
                        for h in made.iter().rev() {
                            // SAFETY: windows this thread created.
                            let _ = unsafe { DestroyWindow(*h) };
                        }
                        let _ = tx.send(Err(format!("CreateWindowExW: {err}")));
                        return;
                    }
                }
            }
            let _ = tx.send(Ok(made.iter().map(|h| h.0 as isize).collect()));
            while !stop_thread.load(Ordering::SeqCst) {
                let mut msg = MSG::default();
                // SAFETY: `msg` is valid and writable; this thread's own queue.
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
            for h in made.into_iter().rev() {
                // SAFETY: windows this thread created; owned ones first.
                let _ = unsafe { DestroyWindow(h) };
            }
        });
        let hwnds = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(hwnds)) => hwnds,
            Ok(Err(err)) => panic!("premise: test windows not created: {err}"),
            Err(err) => panic!("premise: the window thread did not answer: {err}"),
        };
        Windows {
            hwnds,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("ci")
        .join("visible-windows.ps1")
}

/// The handles `Get-ShownWindows <this pid>` returns, as the smoke's steps call it
/// (dot-sourced script, one call).
#[track_caller]
fn shown_by_smoke() -> Vec<isize> {
    let path = script();
    assert!(path.is_file(), "premise: {} exists", path.display());
    let command = format!(
        "$ErrorActionPreference = 'Stop'; . '{}'; $s = Get-ShownWindows {}; \
         foreach ($w in $s) {{ [Console]::Out.WriteLine($w.Handle.ToInt64()) }}; \
         [Console]::Out.WriteLine('end')",
        path.display(),
        std::process::id()
    );
    let child = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(err) => panic!(
            "precondition `pwsh` does not hold: PowerShell 7 is not on PATH ({err}); the \
             windows-latest runner ships it and the install smoke runs under it"
        ),
    };
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(PWSH_BUDGET) {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => panic!("pwsh failed to run: {err}"),
        Err(_) => panic!("pwsh did not finish within {PWSH_BUDGET:?}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success() && stdout.lines().last() == Some("end"),
        "premise: Get-ShownWindows ran ({:?} after {:?}); stdout: {stdout}; stderr: {stderr}",
        output.status,
        started.elapsed()
    );
    stdout
        .lines()
        .filter(|l| *l != "end")
        .map(|l| {
            l.trim()
                .parse::<i64>()
                .unwrap_or_else(|e| panic!("not a handle `{l}`: {e}")) as isize
        })
        .collect()
}

fn plain() -> Spec {
    Spec {
        ex: WINDOW_EX_STYLE(0),
        style: WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        owned: false,
    }
}

#[test]
fn a_visible_unowned_tool_window_counts_as_shown() {
    // Acceptance 2, F-003, analysis invariant 5: tool windows are counted. A visible,
    // unowned, top-level WS_EX_TOOLWINDOW popup (and one styled like the overlay:
    // tool window, no-activate, topmost) is a shown window, so an overlay (or any tool
    // window) visible at start fails the loaded-launch step. Red today:
    // visible-windows.ps1 drops every WS_EX_TOOLWINDOW window. Bite: the tool-window
    // clause kept, or replaced by an exception keyed to a class or title.
    let windows = Windows::open(vec![
        plain(),
        Spec {
            ex: WS_EX_TOOLWINDOW,
            style: WS_POPUP | WS_VISIBLE,
            owned: false,
        },
        Spec {
            ex: WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            style: WS_POPUP | WS_VISIBLE,
            owned: false,
        },
    ]);
    let shown = shown_by_smoke();
    assert!(
        shown.contains(&windows.hwnds[0]),
        "premise: the plain visible window is shown: {shown:?} of {:?}",
        windows.hwnds
    );
    for (i, what) in [(1, "tool window"), (2, "overlay-styled tool window")] {
        assert!(
            shown.contains(&windows.hwnds[i]),
            "the smoke predicate does not count a visible, unowned, top-level {what} \
             ({:#x}); shown: {shown:?}",
            windows.hwnds[i]
        );
    }
}

#[test]
fn hidden_and_owned_windows_are_not_shown_and_a_plain_one_is() {
    // Characterization (green before T-057; must stay green): the predicate's other
    // raw facts. A plain visible top-level window is shown; a hidden one and a
    // visible window owned by another are not (an owned popup is part of its owner).
    // Bite (for the T-057 edit of the script): dropping more than the tool-window
    // clause (the visible or the owner test).
    let windows = Windows::open(vec![
        plain(),
        Spec {
            ex: WINDOW_EX_STYLE(0),
            style: WS_OVERLAPPEDWINDOW,
            owned: false,
        },
        Spec {
            ex: WINDOW_EX_STYLE(0),
            style: WS_POPUP | WS_VISIBLE,
            owned: true,
        },
    ]);
    let shown = shown_by_smoke();
    assert!(
        shown.contains(&windows.hwnds[0]),
        "a plain visible top-level window is not shown: {shown:?} of {:?}",
        windows.hwnds
    );
    assert!(
        !shown.contains(&windows.hwnds[1]),
        "a hidden window is shown: {shown:?}"
    );
    assert!(
        !shown.contains(&windows.hwnds[2]),
        "an owned window is shown: {shown:?}"
    );
}
