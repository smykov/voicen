//! T-128 (spec 007 T029): the pure mapping from a call refusal and a `CallNotice` to a
//! catalog `MessageId`, which the shell hands to the shared `Notifier`.
//!
//! The API these tests pin (the developer builds it in `call/events.rs`):
//!
//! ```text
//! voicen_core::call::events::refused_notice(RefusedPart, RefusedReason) -> MessageId
//! CallNotice::message(&self) -> (MessageId, Vec<(&'static str, String)>)
//! ```
//!
//! Both are total: every part x reason and every variant has an id from the i18n catalog.

use voicen_core::call::events::refused_notice;
use voicen_core::call::{CallId, CallNotice, FailReason, RefusedPart, RefusedReason, StopReason};
use voicen_core::i18n::{MessageId, MESSAGE_IDS};
use voicen_core::platform::ToastAction;

fn id_text(id: MessageId) -> String {
    serde_json::to_string(&id)
        .unwrap_or_else(|e| panic!("serialize: {e}"))
        .trim_matches('"')
        .to_string()
}

fn in_catalog(id: MessageId) {
    assert!(
        MESSAGE_IDS.contains(&id),
        "{} is not a declared catalog id",
        id_text(id)
    );
}

#[test]
fn refused_notice_maps_every_part_and_reason_to_its_catalog_id() {
    // Bite: a constant, a swapped part or reason, a missing arm mapped to a default.
    let parts = [
        (RefusedPart::Mic, "mic"),
        (RefusedPart::System, "system"),
        (RefusedPart::Root, "root"),
    ];
    let reasons = [
        (RefusedReason::NoDevice, "no_device"),
        (RefusedReason::AccessDenied, "access_denied"),
        (RefusedReason::Busy, "busy"),
        (RefusedReason::Unwritable, "unwritable"),
        (RefusedReason::Missing, "missing"),
        (RefusedReason::Other, "other"),
    ];
    let mut seen = Vec::new();
    for (part, p) in parts {
        for (reason, r) in reasons {
            let id = refused_notice(part, reason);
            in_catalog(id);
            assert_eq!(
                id_text(id),
                format!("notice.call_refused.{p}.{r}"),
                "{part:?} x {reason:?}"
            );
            seen.push(id_text(id));
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 18, "eighteen distinct ids");
}

fn message_id(notice: &CallNotice) -> String {
    let (id, _params) = notice.message();
    in_catalog(id);
    id_text(id)
}

#[test]
fn only_the_transcript_ready_notice_carries_a_toast_action() {
    // T-213 (decisions #298, #312): `CallNotice::action(&self) -> Option<ToastAction>` is
    // `Some(ToastAction::Open { call, root })` for `TranscriptReady`, with the notice's own
    // call and root (not a settings root), and `None` for every other variant. Bite: a
    // wildcard `Some`, the root dropped or replaced, a button on a refusal or a stop.
    let call = CallId::parse("2026-11-14_101500").unwrap_or_else(|| panic!("call id"));
    let root = std::path::PathBuf::from("calls-root");
    assert_eq!(
        CallNotice::TranscriptReady {
            call: call.clone(),
            root: root.clone()
        }
        .action(),
        Some(ToastAction::Open { call, root })
    );
    let mut others = vec![CallNotice::CallSavedTranscribing];
    for reason in [
        FailReason::Key,
        FailReason::Network,
        FailReason::Timeout,
        FailReason::Provider,
        FailReason::Storage,
        FailReason::EngineNotSetUp,
    ] {
        others.push(CallNotice::PassFailed { reason });
    }
    for reason in [
        StopReason::User,
        StopReason::Hotkey,
        StopReason::Exit,
        StopReason::SystemEnd,
        StopReason::Sleep,
        StopReason::MaxLength,
        StopReason::LowDisk,
        StopReason::StorageError,
    ] {
        others.push(CallNotice::Stopped { reason });
    }
    for notice in others {
        assert_eq!(notice.action(), None, "{notice:?}");
    }
}

#[test]
fn call_notice_message_is_total() {
    // Bite: a variant falling into a wildcard arm, a wrong id for one stop reason, a
    // reason suffix that is not the catalog's (`storage_error` instead of `storage`).
    assert_eq!(
        message_id(&CallNotice::CallSavedTranscribing),
        "notice.call_saved"
    );
    let call = CallId::parse("2026-11-14_101500").unwrap_or_else(|| panic!("call id"));
    assert_eq!(
        message_id(&CallNotice::TranscriptReady {
            call,
            root: std::path::PathBuf::from("calls-root")
        }),
        "notice.transcript_ready"
    );
    let fails = [
        (FailReason::Key, "key"),
        (FailReason::Network, "network"),
        (FailReason::Timeout, "timeout"),
        (FailReason::Provider, "provider"),
        (FailReason::Storage, "storage"),
        (FailReason::EngineNotSetUp, "engine_not_set_up"),
    ];
    for (reason, name) in fails {
        assert_eq!(
            message_id(&CallNotice::PassFailed { reason }),
            format!("notice.pass_failed.{name}"),
            "{reason:?}"
        );
    }
    let stops = [
        (StopReason::MaxLength, "notice.stopped.max_length"),
        (StopReason::LowDisk, "notice.stopped.low_disk"),
        (StopReason::StorageError, "notice.stopped.storage"),
        (StopReason::Sleep, "notice.stopped.sleep"),
        (StopReason::User, "notice.call_saved"),
        (StopReason::Hotkey, "notice.call_saved"),
        (StopReason::Exit, "notice.call_saved"),
        (StopReason::SystemEnd, "notice.call_saved"),
    ];
    for (reason, id) in stops {
        assert_eq!(
            message_id(&CallNotice::Stopped { reason }),
            id,
            "Stopped {reason:?}"
        );
    }
}
