// Folded from the T-035/T-036 fixture v-doc-thematic-break-code (removed by T-038).
// T-036 review 1 #1 (e): a thematic break `---`, then a 4-column line: a doctest.
/// Intro.
///
/// ---
///     compile_error!("a doctest rustdoc would run");
pub fn f() {}
