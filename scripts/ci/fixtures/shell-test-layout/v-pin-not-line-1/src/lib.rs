// a comment before the pin
#![cfg(not(any(test, doctest)))]
// T-092 fixture: the shell's lib crate root. Line 1 is the crate-level cfg that empties the
// crate under test and doctest (scripts/ci/shell-test-layout.sh compares it byte-exactly).

/// A doc block: the pin keeps it out of every doctest run.
pub fn run() -> u32 {
    7
}
