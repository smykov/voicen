pub fn f() {}
// A raw identifier names the same macro: rustc 1.99 --test runs the outside #[test].
r#include!("../other/t.rs");
