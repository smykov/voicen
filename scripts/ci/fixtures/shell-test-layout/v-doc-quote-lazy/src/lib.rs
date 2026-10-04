// T-036 review 1 #1 (a): `> Note`, `>`, then a 4-column line: rustdoc 1.99 runs it as a doctest.
/// > Note
/// >
///     compile_error!("a doctest rustdoc would run");
pub fn f() {}
