//! T-111 (docs/decisions/core-tests.md): the parts of `common::timing` that the
//! in-time work adds: the margin constant `M`, the one skip writer `skip`, and
//! `with_premise` (moved from `dictation_session.rs`, the budget now a parameter).
//!
//! Not a module of `tests/common`: included by `tests/common_in_time.rs` with `#[path]`.

use std::cell::Cell;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use crate::common::timing::{eventually_progressing, late_answer, skip, with_premise, M};

/// Set by the parent tests below; the two `child_*` tests do nothing without it, so
/// a plain run of this binary only checks that they pass.
const CHILD_ENV: &str = "VOICEN_T111_SKIP_CHILD";

#[test]
fn m_is_five_seconds() {
    // #200 D1: the one minimum in-time margin of a bite-bound deadline. Every
    // widened test states its margin as a multiple of this constant, so it is one
    // named value, not a literal per file. Bite: another value, a constant per file.
    assert_eq!(M, Duration::from_secs(5));
}

#[test]
fn a_late_answer_comes_m_after_the_deadline() {
    // T-111 review 1 #2, kind (1) of the invariant. Bite: an answer closer than M, or
    // one that ignores the deadline.
    let deadline = Duration::from_millis(300);
    assert_eq!(late_answer(deadline), deadline + M);
    assert_eq!(late_answer(Duration::ZERO), M);
}

/// Runs the named `child_*` test of this binary as a separate process, under
/// libtest's default output capture (no `--nocapture`), and returns its stderr.
fn child_stderr(mode: &str, test: &str) -> String {
    let exe = std::env::current_exe().expect("this test binary's path");
    let out = Command::new(exe)
        .args([
            &format!("timing_seam_tests::{test}"),
            "--exact",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, mode)
        .output()
        .expect("run the child test process");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "the child test failed ({:?}); stderr: {stderr}; stdout: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
    // Positive control: the child really ran its test (libtest says so on stdout).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 passed"),
        "the child did not run exactly its test: {stdout}"
    );
    stderr
}

#[test]
fn child_writes_one_skip_line() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("skip") {
        skip("T111-SKIP-LINE-MARK");
    }
}

#[test]
fn child_with_premise_misses_every_run() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("premise") {
        with_premise("T111-WHAT-MARK", Duration::from_millis(50), || false);
    }
}

#[test]
fn a_skip_line_reaches_the_real_stderr_under_libtest_capture() {
    // T-110 r2#1: `eprintln!` is captured by libtest for a passing test, so a skip
    // looked exactly like a pass in the gate and in CI. The skip writer writes to
    // `std::io::stderr()` directly. Checked in a child process: inside this process
    // the captured and the real stderr look the same to the test. Bite: the writer
    // using `eprintln!` / `print!` / stdout, the line dropped, no newline.
    let stderr = child_stderr("skip", "child_writes_one_skip_line");
    assert!(
        stderr.lines().any(|l| l.contains("T111-SKIP-LINE-MARK")),
        "the skip line is not on the child's stderr: {stderr:?}"
    );
}

#[test]
fn with_premise_reports_its_skip_through_the_same_writer() {
    // The skip of `with_premise` (every run missed its premise within the budget) and
    // the skip of `the_session_timer_stops_a_recording_at_its_max_length` share one
    // writer (seam d). Bite: `with_premise` keeping its own `eprintln!`; the reason
    // text without the test's `what`.
    let stderr = child_stderr("premise", "child_with_premise_misses_every_run");
    assert!(
        stderr
            .lines()
            .any(|l| l.contains("skipped") && l.contains("T111-WHAT-MARK")),
        "the with_premise skip line is not on the child's stderr: {stderr:?}"
    );
}

#[test]
fn with_premise_stops_at_the_first_run_whose_premise_held() {
    // Bite: a loop that runs again after a held premise, a run never made.
    let runs = Cell::new(0);
    with_premise("held", Duration::from_secs(60), || {
        runs.set(runs.get() + 1);
        true
    });
    assert_eq!(runs.get(), 1);
}

#[test]
fn with_premise_reruns_a_missed_premise_until_it_holds() {
    // A host stall made the first two runs miss the premise; the third holds. The
    // scenario is re-run on a fresh rig, not skipped and not failed. Bite: a
    // skip after the first miss, a run count that stops at two.
    let runs = Cell::new(0);
    with_premise("rerun", Duration::from_secs(60), || {
        runs.set(runs.get() + 1);
        runs.get() == 3
    });
    assert_eq!(runs.get(), 3);
}

#[test]
fn with_premise_gives_up_when_the_budget_has_passed_since_the_first_start() {
    // The budget is a parameter now (dictation_session's is 10 s, local_download
    // needs more). Every run misses; each takes 300 ms; the budget is 1 s (a first run
    // starved past a short budget would end the loop after one run: a verdict on host
    // load). The call returns after a few runs (no panic: a missed premise is no verdict, never
    // a red) and not after a single one (it re-runs while the budget allows). Bite:
    // a budget fixed inside the helper (it would loop for seconds, the hang
    // ends the test run), the first miss skipped.
    let runs = Cell::new(0);
    with_premise("budget", Duration::from_secs(1), || {
        runs.set(runs.get() + 1);
        std::thread::sleep(Duration::from_millis(300));
        false
    });
    assert!(
        runs.get() >= 2,
        "re-run while the budget allows: {} run(s)",
        runs.get()
    );
}

#[test]
fn with_premise_budget_is_the_parameter_not_a_constant_of_the_helper() {
    // A zero budget has passed at once: a premise that never holds is run once (twice
    // at most, if the clock has not ticked), never in a loop. Bite: a budget fixed in
    // the helper (dictation_session's 10 s): thousands of runs of a body that returns
    // at once.
    let runs = Cell::new(0u32);
    with_premise("zero budget", Duration::ZERO, || {
        runs.set(runs.get() + 1);
        false
    });
    assert!(
        (1..=2).contains(&runs.get()),
        "{} run(s) under a zero budget",
        runs.get()
    );
}

/// A backlog that moves one step every `every` readings of `progress` (the wait's own
/// polls, 5 ms apart) for `steps` steps, then stops. No second thread: the count
/// advances only when the wait reads it, so no verdict depends on a thread being
/// scheduled in time (F-033). Returns the `progress` reading and the count itself.
fn polled_counter(every: usize, steps: usize) -> (impl FnMut() -> usize, Rc<Cell<usize>>) {
    let count = Rc::new(Cell::new(0usize));
    let reads = Cell::new(0usize);
    let c = Rc::clone(&count);
    let progress = move || {
        reads.set(reads.get() + 1);
        if reads.get().is_multiple_of(every) && c.get() < steps {
            c.set(c.get() + 1);
        }
        c.get()
    };
    (progress, count)
}

#[test]
fn a_progress_wait_passes_when_the_count_keeps_advancing_past_the_budget() {
    // T-239 slow-progress leg: a step every 20 polls (about 100 ms), 30 steps, about
    // 3 s in total, more than the 2 s stall budget, but the count grows throughout, so
    // the wait is not over: the verdict must not depend on how long the total work
    // takes. The count moves on the wait's own polls (no counter thread), and the gap
    // between steps is 20x smaller than the budget. Bite: a total-time wait
    // (`eventually`), or a budget that is not reset by progress.
    let (progress, count) = polled_counter(20, 30);
    let ok = eventually_progressing(Duration::from_secs(2), progress, || count.get() >= 30);
    assert!(ok, "the count advanced to the end, so the wait must pass");
}

#[test]
fn a_progress_wait_fails_when_the_count_stops_before_done() {
    // T-239 stopped-counter leg: the count advances a few steps, then stops short of
    // `done`. The wait must end with `false` (a hang is still caught) and only after
    // the budget has passed with no progress (a lower bound, never an upper one).
    // Host load can only delay the end, never turn this verdict. Bite: a wait that
    // never fails (no budget), or one that stops reading progress.
    let (progress, count) = polled_counter(2, 5);
    let budget = Duration::from_millis(300);
    let (ok, took) = crate::common::timing::measure(|| {
        eventually_progressing(budget, progress, || count.get() >= 60)
    });
    assert!(!ok, "the count stopped at 5 of 60, so the wait must fail");
    assert_eq!(count.get(), 5, "the count advanced to its stop first");
    crate::common::timing::at_least(took, budget);
}

#[test]
fn a_progress_wait_fails_when_the_count_never_moves() {
    // T-239: a hung consumer from the start (no step at all) fails as well. Bite: a
    // wait that treats the first reading as progress forever.
    let ok = eventually_progressing(Duration::from_millis(100), || 0, || false);
    assert!(!ok, "no progress at all must fail");
}

#[test]
fn a_progress_wait_passes_at_once_when_done_already_holds() {
    // T-239: `done` is checked before any budget matters, so a zero budget with a
    // finished backlog passes. Bite: a wait that fails first on an empty budget.
    assert!(eventually_progressing(Duration::ZERO, || 7, || true));
}
