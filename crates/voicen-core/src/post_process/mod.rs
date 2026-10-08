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
    fn process(&self, text: String, _settings: &Settings) -> PostProcessed {
        PostProcessed { text, notice: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::DeliveryResult;
    use crate::failure::FailureReason;
    use crate::i18n::{self, text, UiLanguage, MESSAGE_IDS};
    use crate::secrets::FakeCredentialStore;
    use crate::secrets::KeySlot;
    use crate::timeouts::Timeouts;

    const HOST: &str = "api.example.com:8443";

    fn every_reason() -> Vec<SkipReason> {
        vec![
            SkipReason::Timeout,
            SkipReason::Unreachable {
                host: HOST.to_string(),
            },
            SkipReason::InvalidKey,
            SkipReason::Http { status: 500 },
            SkipReason::InvalidResponse,
            SkipReason::NotConfigured,
        ]
    }

    #[test]
    fn pass_through_is_not_run_and_reads_no_key() {
        // PassThrough stays what src-tauri installs until T-074: NotRun (the raw
        // text is delivered) and no credential read, even with post-processing on
        // and a key stored. Bite: Applied(raw) or a skip from the inert stage, a
        // Credential Manager read on every dictation.
        let mut s = settings::defaults();
        s.enabled = true;
        s.base_url = "https://llm.example.com/v1".to_string();
        let creds = FakeCredentialStore::new().with_key(KeySlot::PostProcessing, "sk-test-pp");
        let t = Timeouts::default();
        let input = PostProcessInput {
            settings: &s,
            credentials: &creds,
            timeouts: &t,
        };
        for raw in ["hello world", "  spaced  ", "TRANSCRIPT-MARKER\nline 2"] {
            assert_eq!(
                PassThrough.process(raw, &input),
                PostProcessOutcome::NotRun,
                "{raw:?}"
            );
        }
        assert_eq!(creds.calls(), vec![]);
    }

    #[test]
    fn final_text_is_the_applied_text_else_the_raw() {
        // data-model "PostProcessOutcome" / FR-016: Applied(text) -> text;
        // NotRun and every Skipped -> the raw transcript, the same bytes. Bite:
        // "" or the reason for a skip, the raw text for Applied.
        let raw = "  привет  как\tдела ";
        assert_eq!(
            PostProcessOutcome::Applied("Привет, как дела?".to_string()).final_text(raw),
            "Привет, как дела?"
        );
        assert_eq!(PostProcessOutcome::NotRun.final_text(raw), raw);
        for r in every_reason() {
            assert_eq!(
                PostProcessOutcome::Skipped(r.clone()).final_text(raw),
                raw,
                "{r:?}"
            );
        }
    }

    #[test]
    fn skip_reasons_have_code_id_params_and_contract_text() {
        // contracts/ipc.md "Message catalog entries" + #91(2): each of the six
        // reasons has its wire code, its own notice.post_processing_skipped.* id
        // declared with messages! (so the catalog tests cover it), and that id
        // renders the contract text in en and ru with its params (`host`,
        // `status`). Bite: a shared id, an id not in MESSAGE_IDS, a missing
        // catalog entry (text() falls back to the id), host/status not passed.
        let prefix_ru = "Постобработка пропущена — ";
        let rows: Vec<(SkipReason, &str, &str, &str, Option<&str>)> = vec![
            (
                SkipReason::Timeout,
                "timeout",
                "notice.post_processing_skipped.timeout",
                "Post-processing skipped — timeout",
                Some("Постобработка пропущена — превышено время ожидания"),
            ),
            (
                SkipReason::Unreachable {
                    host: HOST.to_string(),
                },
                "unreachable",
                "notice.post_processing_skipped.unreachable",
                "Post-processing skipped — cannot reach api.example.com:8443",
                Some("Постобработка пропущена — нет связи с api.example.com:8443"),
            ),
            (
                SkipReason::InvalidKey,
                "invalid_key",
                "notice.post_processing_skipped.invalid_key",
                "Post-processing skipped — invalid API key",
                Some("Постобработка пропущена — неверный ключ API"),
            ),
            (
                SkipReason::Http { status: 500 },
                "http",
                "notice.post_processing_skipped.http",
                "Post-processing skipped — HTTP 500",
                Some("Постобработка пропущена — HTTP 500"),
            ),
            (
                SkipReason::InvalidResponse,
                "invalid_response",
                "notice.post_processing_skipped.invalid_response",
                "Post-processing skipped — empty or invalid response",
                Some("Постобработка пропущена — пустой или неверный ответ"),
            ),
            (
                SkipReason::NotConfigured,
                "not_configured",
                "notice.post_processing_skipped.not_configured",
                "Post-processing skipped — not set up. Check the Post-processing settings.",
                // #91(2) fixes the English text only; the Russian one is the
                // developer's, reviewed: checked for the common prefix.
                None,
            ),
        ];
        assert_eq!(rows.len(), every_reason().len());
        let mut wrong = Vec::new();
        for (r, code, id, en, ru) in rows {
            if r.code() != code {
                wrong.push(format!("{r:?}: code {:?}, expected {code:?}", r.code()));
            }
            let got_id = serde_json::to_value(r.message_id()).unwrap_or_default();
            if got_id != serde_json::Value::String(id.to_string()) {
                wrong.push(format!("{r:?}: message id {got_id}, expected {id:?}"));
            }
            if !MESSAGE_IDS.contains(&r.message_id()) {
                wrong.push(format!("{r:?}: message id not declared with messages!"));
            }
            let params = r.message_params();
            let args: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let got_en = text(UiLanguage::En, r.message_id(), &args);
            if got_en != en {
                wrong.push(format!("{r:?} En: {got_en:?}, expected {en:?}"));
            }
            let got_ru = text(UiLanguage::Ru, r.message_id(), &args);
            let ru_ok = match ru {
                Some(want) => got_ru == want,
                None => {
                    got_ru.starts_with(prefix_ru)
                        && got_ru.chars().count() > prefix_ru.chars().count()
                }
            };
            if !ru_ok {
                wrong.push(format!("{r:?} Ru: {got_ru:?}, expected {ru:?}"));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    #[test]
    fn skip_reason_params_are_host_and_status_only() {
        // FR-014: the only params are `host` (Unreachable) and `status` (Http);
        // nothing else can carry text into a notice. Bite: an extra param, the
        // status as a host, params on a param-less reason.
        for r in every_reason() {
            let want: Vec<(&str, String)> = match &r {
                SkipReason::Unreachable { host } => vec![("host", host.clone())],
                SkipReason::Http { status } => vec![("status", status.to_string())],
                _ => Vec::new(),
            };
            assert_eq!(r.message_params(), want, "{r:?}");
        }
    }

    #[test]
    fn failure_reasons_map_to_skip_reasons() {
        // research R4 over failure::classify's results: 401/403 or an unusable key
        // -> InvalidKey; DNS, network/host unreachable and connect failures ->
        // Unreachable{host of the configured base URL}; Timeout -> Timeout; any
        // other status -> Http{status}; a bad body -> InvalidResponse. Bite:
        // NetworkUnavailable to anything but Unreachable (spec US2-2), the status
        // lost, the host taken from anywhere but the argument.
        let rows = [
            (FailureReason::InvalidApiKey, SkipReason::InvalidKey),
            (
                FailureReason::NetworkUnavailable,
                SkipReason::Unreachable {
                    host: HOST.to_string(),
                },
            ),
            (
                FailureReason::CannotReach {
                    host: HOST.to_string(),
                },
                SkipReason::Unreachable {
                    host: HOST.to_string(),
                },
            ),
            (FailureReason::Timeout, SkipReason::Timeout),
            (
                FailureReason::ServerError { status: 429 },
                SkipReason::Http { status: 429 },
            ),
            (
                FailureReason::ServerError { status: 503 },
                SkipReason::Http { status: 503 },
            ),
            (
                FailureReason::UnexpectedResponse,
                SkipReason::InvalidResponse,
            ),
        ];
        for (failure, want) in rows {
            assert_eq!(
                SkipReason::from_failure(&failure, HOST),
                want,
                "{failure:?}"
            );
        }
    }

    #[test]
    fn skip_message_follows_decision_91_3() {
        // #91(3): copied_paste_manually wins over the skip (the user must act);
        // the skip wins over `copied` and is shown after a paste, with its
        // params. 6 reasons x 3 delivery results. Bite: the delivery notice for
        // CopiedOnly, the skip for CopyManual, params dropped, nothing for Pasted.
        let mut wrong = Vec::new();
        for r in every_reason() {
            for delivery in [
                DeliveryResult::Pasted,
                DeliveryResult::CopiedOnly,
                DeliveryResult::CopyManual,
            ] {
                let want = match delivery {
                    DeliveryResult::CopyManual => (i18n::NOTICE_COPIED_PASTE_MANUALLY, Vec::new()),
                    DeliveryResult::Pasted | DeliveryResult::CopiedOnly => {
                        (r.message_id(), r.message_params())
                    }
                };
                let got = skip_message(&r, delivery);
                if got != want {
                    wrong.push(format!("{r:?} {delivery:?}: {got:?}, expected {want:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }
}
