#![cfg(not(any(test, doctest)))]
// T-092 fixture: the shell's lib crate root. Line 1 is the crate-level cfg that empties the
// crate under test and doctest (scripts/ci/shell-test-layout.sh compares it byte-exactly).

/// A doc block: the pin keeps it out of every doctest run.
pub fn run() -> u32 {
    7
}

// R1 (T-086 audit): #[cfg(test)] #[test] built from an ident fragment. The guard reads no Rust;
// the pin on line 1 empties the crate under test, so this never reaches a test exe.
macro_rules! r1 {
    ($t:ident) => {
        #[cfg($t)]
        mod r1 {
            #[$t]
            fn planted() {
                panic!("T092-PLANTED-R1");
            }
        }
    };
}
r1!(test);

#[cfg(test)]
mod plain {
    #[test]
    fn planted() {
        panic!("T092-PLANTED-LIB-TEST");
    }
}
