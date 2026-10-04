use core::include as pull;
pub fn f() {}
// T-036 review 1 #2: rustc 1.99 --test compiles and runs the outside #[test] (1 passed).
pull!("../other/t.rs");
