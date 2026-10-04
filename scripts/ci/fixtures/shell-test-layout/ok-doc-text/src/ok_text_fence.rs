// Folded from the T-035/T-036 fixture ok-text-fence (removed by T-038).
//! Crate docs with a text block, which rustdoc never compiles:
//!
//! ```text
//! %LOCALAPPDATA%\Voicen\logs\voicen.log
//! ```

/// Output looks like this:
///
/// ```text
/// voicen 0.1.0 (abc1234) started
/// ```
pub fn log_line() {}
