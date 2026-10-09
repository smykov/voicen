//! LLM post-processing (spec 003, T-020; decisions #91, #99). T-003 created the
//! settings data type; T-001 added the pipeline step's port; T-020 widened it to
//! [`PostProcessInput`] -> [`PostProcessOutcome`] and added the real processor
//! ([`chat::ChatPostProcessor`]), which the shell installs since T-074
//! ([`PassThrough`] stays for tests); T-021 added `settings::validate`.
//!
//! The stage never fails a dictation: the delivered text is the trimmed non-empty
//! chat reply ([`PostProcessOutcome::Applied`]) or else the raw transcript byte
//! for byte ([`PostProcessOutcome::final_text`]). A skip carries exactly one
//! [`SkipReason`], built only from a status, the transport classification and the
//! base URL's `host[:port]`: never a key, prompt, transcript, URL query or body.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod chat;
pub mod settings;

use crate::delivery::DeliveryResult;
use crate::events::SkipKind;
use crate::failure::FailureReason;
use crate::i18n::{self, MessageId};
use crate::secrets::CredentialStore;
use crate::timeouts::Timeouts;

use settings::PostProcessingSettings;

/// What the stage gets for one job: the post-processing settings of the job's
/// press snapshot, the one credential store, and the job's [`Timeouts`] (the same
/// value `Pipeline::process` hands to the engine; `post_processing` is the whole
/// request's deadline, `connect` the connect limit).
#[derive(Clone, Copy)]
pub struct PostProcessInput<'a> {
    pub settings: &'a PostProcessingSettings,
    pub credentials: &'a dyn CredentialStore,
    pub timeouts: &'a Timeouts,
}

/// How the stage ended (spec 003 data-model "PostProcessOutcome").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostProcessOutcome {
    /// The stage did not run (post-processing off, or [`PassThrough`]): the raw
    /// transcript is delivered, nothing is shown.
    NotRun,
    /// The trimmed reply text.
    Applied(String),
    /// The stage ran and failed: the raw transcript is delivered, the reason shown.
    Skipped(SkipReason),
}

impl PostProcessOutcome {
    /// The text to deliver: the applied text, else `raw` unchanged.
    pub fn final_text<'a>(&'a self, raw: &'a str) -> &'a str {
        match self {
            PostProcessOutcome::Applied(text) => text,
            PostProcessOutcome::NotRun | PostProcessOutcome::Skipped(_) => raw,
        }
    }
}

/// Why post-processing was skipped (spec 003 Q1 plus #91(2)'s `NotConfigured`).
/// `host` is the configured base URL's `host[:port]` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The whole request exceeded `Timeouts::post_processing`.
    Timeout,
    /// DNS failure, network/host unreachable, refused or failed connect.
    Unreachable { host: String },
    /// HTTP 401/403, or a stored key that cannot be sent (nothing was sent).
    InvalidKey,
    /// Any other non-2xx status.
    Http { status: u16 },
    /// A 2xx body without a non-blank string `choices[0].message.content`, over
    /// 1 MiB, not UTF-8 or cut off.
    InvalidResponse,
    /// Enabled, but the stored base URL fails `check_base_url` (decision #91(2)).
    NotConfigured,
}

impl SkipReason {
    /// Stable code for events and logs.
    pub fn code(&self) -> &'static str {
        match self {
            SkipReason::Timeout => "timeout",
            SkipReason::Unreachable { .. } => "unreachable",
            SkipReason::InvalidKey => "invalid_key",
            SkipReason::Http { .. } => "http",
            SkipReason::InvalidResponse => "invalid_response",
            SkipReason::NotConfigured => "not_configured",
        }
    }

    /// The closed kind for the log (T-076): the reason without its host or
    /// status.
    pub fn kind(&self) -> SkipKind {
        match self {
            SkipReason::Timeout => SkipKind::Timeout,
            SkipReason::Unreachable { .. } => SkipKind::Unreachable,
            SkipReason::InvalidKey => SkipKind::InvalidKey,
            SkipReason::Http { .. } => SkipKind::Http,
            SkipReason::InvalidResponse => SkipKind::InvalidResponse,
            SkipReason::NotConfigured => SkipKind::NotConfigured,
        }
    }

    /// The catalog message (`notice.post_processing_skipped.*`).
    pub fn message_id(&self) -> MessageId {
        match self {
            SkipReason::Timeout => i18n::NOTICE_POST_PROCESSING_SKIPPED_TIMEOUT,
            SkipReason::Unreachable { .. } => i18n::NOTICE_POST_PROCESSING_SKIPPED_UNREACHABLE,
            SkipReason::InvalidKey => i18n::NOTICE_POST_PROCESSING_SKIPPED_INVALID_KEY,
            SkipReason::Http { .. } => i18n::NOTICE_POST_PROCESSING_SKIPPED_HTTP,
            SkipReason::InvalidResponse => i18n::NOTICE_POST_PROCESSING_SKIPPED_INVALID_RESPONSE,
            SkipReason::NotConfigured => i18n::NOTICE_POST_PROCESSING_SKIPPED_NOT_CONFIGURED,
        }
    }

    /// Placeholder values for [`message_id`](Self::message_id): `host` for
    /// `Unreachable`, `status` for `Http`, none otherwise.
    pub fn message_params(&self) -> Vec<(&'static str, String)> {
        match self {
            SkipReason::Unreachable { host } => vec![("host", host.clone())],
            SkipReason::Http { status } => vec![("status", status.to_string())],
            SkipReason::Timeout
            | SkipReason::InvalidKey
            | SkipReason::InvalidResponse
            | SkipReason::NotConfigured => Vec::new(),
        }
    }

    /// The skip reason of a transport failure classified by `failure::classify`
    /// (research R4); `host` is the configured base URL's `host[:port]`, used for
    /// both unreachable kinds (spec US2-2: a DNS failure is "cannot reach").
    /// `classify` never yields the key-store, engine, clipboard or microphone
    /// reasons; they map to the nearest skip so the function stays total.
    pub fn from_failure(reason: &FailureReason, host: &str) -> SkipReason {
        match reason {
            FailureReason::InvalidApiKey | FailureReason::KeyStoreUnavailable => {
                SkipReason::InvalidKey
            }
            FailureReason::NetworkUnavailable | FailureReason::CannotReach { .. } => {
                SkipReason::Unreachable {
                    host: host.to_string(),
                }
            }
            FailureReason::Timeout => SkipReason::Timeout,
            FailureReason::ServerError { status } => SkipReason::Http { status: *status },
            FailureReason::EngineNotConfigured => SkipReason::NotConfigured,
            FailureReason::UnexpectedResponse
            | FailureReason::ClipboardUnavailable
            | FailureReason::MicrophoneUnavailable { .. } => SkipReason::InvalidResponse,
        }
    }
}

/// The one overlay message of a delivered job whose post-processing was skipped
/// (decision #91(3)): after a failed paste (`CopyManual`) the user must act, so
/// `notice.copied_paste_manually` wins; otherwise (pasted, or copied with
/// auto-paste off) the skip message with its params. Tray `Error` marks the skip
/// in every case (`RecordingController::job_finished`).
pub fn skip_message(
    reason: &SkipReason,
    delivery: DeliveryResult,
) -> (MessageId, Vec<(&'static str, String)>) {
    match delivery {
        DeliveryResult::CopyManual => (i18n::NOTICE_COPIED_PASTE_MANUALLY, Vec::new()),
        DeliveryResult::Pasted | DeliveryResult::CopiedOnly => {
            (reason.message_id(), reason.message_params())
        }
    }
}

/// The pipeline step after transcription (spec 003 contracts/core-post-process.md,
/// in the synchronous form of decision #42). Called on the job thread only for a
/// non-blank transcript; must not fail the dictation and must return within
/// `input.timeouts.post_processing` (+ setup).
pub trait PostProcessor: Send + Sync {
    fn process(&self, raw: &str, input: &PostProcessInput<'_>) -> PostProcessOutcome;
}

/// The inert stage: [`PostProcessOutcome::NotRun`], no credential read, no request.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassThrough;

impl PostProcessor for PassThrough {
    fn process(&self, _raw: &str, _input: &PostProcessInput<'_>) -> PostProcessOutcome {
        PostProcessOutcome::NotRun
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
    fn skip_kind_is_the_closed_copy_of_the_reason() {
        // T-076: the log gets SkipReason::kind(), a Copy closed enum without the host
        // or the status, one kind per reason, spelled like code() (the pp_reason
        // literals). Bite: a shared kind, a kind that depends on the host or the
        // status, a kind that disagrees with code().
        use crate::events::SkipKind;
        fn literal(kind: SkipKind) -> &'static str {
            match kind {
                SkipKind::Timeout => "timeout",
                SkipKind::Unreachable => "unreachable",
                SkipKind::InvalidKey => "invalid_key",
                SkipKind::Http => "http",
                SkipKind::InvalidResponse => "invalid_response",
                SkipKind::NotConfigured => "not_configured",
            }
        }
        fn assert_copy<T: Copy>() {}
        assert_copy::<SkipKind>();
        let mut kinds = Vec::new();
        for r in every_reason() {
            let kind = r.kind();
            assert_eq!(literal(kind), r.code(), "{r:?}");
            assert!(!kinds.contains(&kind), "{r:?}: {kind:?} used twice");
            kinds.push(kind);
        }
        assert_eq!(kinds.len(), 6);
        for host in ["", "llm.example.com", "192.0.2.10:8000"] {
            let r = SkipReason::Unreachable {
                host: host.to_string(),
            };
            assert_eq!(r.kind(), SkipKind::Unreachable, "{host:?}");
        }
        for status in [100, 404, 500, u16::MAX] {
            assert_eq!(
                SkipReason::Http { status }.kind(),
                SkipKind::Http,
                "{status}"
            );
        }
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
