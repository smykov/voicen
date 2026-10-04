// Folded from the T-035/T-036 fixture v-doc-star-break-code (removed by T-038).
// T-036 review 1 #1 (g): text, a `***` thematic break, then a 4-column line: a doctest.
/// Text.
/// ***
///     compile_error!("a doctest rustdoc would run");
pub fn f() {}
