// Folded from the T-035/T-036 fixture v-doc-list-continuation (removed by T-038).
/// A list continuation indented 4 columns (W = 2 for `- `): rustdoc reads it as part of
/// the item, not as a code block, yet the guard refuses it (it cannot track lists):
///
/// - Read the Run value.
///
///     If it is missing, there is nothing to do.
pub fn continuation() {}

/// A nested list indented 4 columns after a blank doc line:
///
/// - outer
///
///     - inner
pub fn nested() {}
