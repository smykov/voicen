//! LLM post-processing (spec 003). T-003 created the settings data type; T-001 adds
//! the pipeline step's port ([`PostProcessor`], with [`PassThrough`] until T-020);
//! T-020/T-021 add the real processor and `settings::validate`.

pub mod settings;

use crate::i18n::MessageId;
use crate::settings::Settings;

/// The text to deliver and an optional notice (spec 003 defines the notices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostProcessed {
    pub text: String,
    pub notice: Option<MessageId>,
}

/// The pipeline step after transcription (FR-021; contracts/core-traits.md
/// "PostProcessor"). Must not fail the dictation: on its own failure it returns the
/// input text plus a notice. Takes the job's settings snapshot. T-020 widens the
/// arguments (credentials, timeouts).
pub trait PostProcessor: Send + Sync {
    fn process(&self, text: String, settings: &Settings) -> PostProcessed;
}

/// Returns the text unchanged, with no notice.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassThrough;

impl PostProcessor for PassThrough {
    fn process(&self, text: String, settings: &Settings) -> PostProcessed {
        // Skeleton (T-001 red tests): not implemented yet.
        let _ = (text, settings);
        PostProcessed {
            text: String::new(),
            notice: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::defaults;

    #[test]
    fn pass_through_returns_the_text_unchanged_without_notice() {
        // Bite: trimming, an empty text (the pipeline would report NoSpeech), or a
        // notice on every delivery.
        let s = defaults(None);
        for text in ["hello world", "  spaced  ", "TRANSCRIPT-MARKER\nline 2"] {
            assert_eq!(
                PassThrough.process(text.to_string(), &s),
                PostProcessed {
                    text: text.to_string(),
                    notice: None
                },
                "{text:?}"
            );
        }
    }
}
