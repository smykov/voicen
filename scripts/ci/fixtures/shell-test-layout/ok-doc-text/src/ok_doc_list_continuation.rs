// Folded from the T-035/T-036 fixture ok-doc-list-continuation (removed by T-038).
/// List continuations and nested lists indented by the marker width (below 4 columns):
///
/// - Read the Run value.
///
///   If it is missing, there is nothing to do (2 columns under `- `).
///
///   - a nested item at 2 columns
///   - another nested item
///
/// 1. First step.
///
///    Its continuation at 3 columns under `1. `.
///
/// 2. Second step.
pub fn steps() {}
