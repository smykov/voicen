//! T-165 (docs/decisions/core-tests.md, in-time rule; guard
//! `scripts/ci/core-test-deadlines.sh`): the named `Timeouts` constructors of the core
//! tests that are not download ones (those are in [`super::download`]). No test file
//! outside `tests/common` builds a `Timeouts` value itself; it takes one of these, or
//! `os_answer::in_time()` for a leg that expects an answer, or an OS-answer case's own
//! deadlines. Each name states the leg's kind; no function here takes a `Duration`.
//! The values are the ones the test files held before T-165; their per-file pins
//! (`*_keep_the_in_time_margin_m`, `in_time_deadlines_of_this_file_*`, ...) read them
//! from here.

use std::time::Duration;

use super::os_answer::{in_time, OsAnswer, OS_ANSWER_BUDGET};
use voicen_core::settings::{defaults, TimeoutSettings};
use voicen_core::timeouts::Timeouts;

/// The short limit of the timeout legs (a whole-request or post-processing total);
/// their server answers `timing::late_answer(SHORT_LIMIT)`, at least `M` later.
pub const SHORT_LIMIT: Duration = Duration::from_millis(300);

/// The whole-request limit of the mid-body stall leg (`openai_client`); its server
/// holds the body back `timing::late_answer(STALL_LIMIT)`.
pub const STALL_LIMIT: Duration = Duration::from_millis(500);

/// The built-in engine's limit of the `local_engine` legs.
pub const BUILTIN_LIMIT: Duration = Duration::from_secs(45);

/// The connect limit of the held-lookup legs (T-079): the call must end by it while
/// the lookup is still held.
pub const HELD_LOOKUP_CONNECT: Duration = Duration::from_millis(300);

// ---- the production values -------------------------------------------------------

/// The production defaults (`Timeouts::default()`): for a leg whose subject is the
/// default itself (SC-003's 15 s post-processing limit) or a call that never reaches
/// a timer.
pub fn production_defaults() -> Timeouts {
    Timeouts::default()
}

/// The one conversion of the settings route, `Timeouts::from_settings`: only the
/// expected value of a test whose subject is the settings conversion itself (the
/// settings route, a named T1 residual by decision #258, which keeps the forms where
/// they are). Not a deadline for an in-time leg: take `os_answer::in_time()` or a
/// named constructor here for that.
pub fn from_settings(form: &TimeoutSettings) -> Timeouts {
    Timeouts::from_settings(form)
}

/// The default settings' timeouts through [`from_settings`] (the production request
/// deadlines of `final_pass_http`).
pub fn from_default_settings() -> Timeouts {
    from_settings(&defaults(None).timeouts)
}

/// The production defaults with a 60 s post-processing limit: the control leg of
/// `default_deadline_skips_a_silent_server_at_15_s`, whose server answers at 15.5 s.
pub fn generous_post_processing() -> Timeouts {
    Timeouts {
        post_processing: Duration::from_secs(60),
        ..Timeouts::default()
    }
}

/// The production defaults with [`BUILTIN_LIMIT`] for the built-in engine.
pub fn builtin_limit() -> Timeouts {
    Timeouts {
        builtin: BUILTIN_LIMIT,
        ..Timeouts::default()
    }
}

/// The production defaults with a 10 s API limit (`local_no_network`'s pipeline,
/// which never reaches the network).
pub fn api_ten_seconds() -> Timeouts {
    Timeouts {
        api_transcription: Duration::from_secs(10),
        ..Timeouts::default()
    }
}

// ---- in-time legs (the deadline is not the subject) --------------------------------

/// The deadlines of a scenario whose mock answers at once (pasted, copied only,
/// clipboard fails, 401, 500, bad body, a quick connection test): the deadline is not
/// its subject, so a host stall must not decide it. `os_answer::in_time()` (T-111).
pub fn immediate_answer() -> Timeouts {
    in_time()
}

/// The deadlines of the post-processing outcome scenarios but the timeout one
/// ([`short_post_processing_total`] only shortens `post_processing`):
/// `os_answer::in_time()` (T-111).
pub fn post_processing_outcome() -> Timeouts {
    in_time()
}

// ---- timeout legs (kind (1): the answer comes at least M after the limit) ----------

/// Only the API whole-request limit is short ([`SHORT_LIMIT`]); every other timer is
/// in time, so none lies within `M` of it (T-111). Also the local-server leg that
/// proves the engine reads `local_server`, not `api_transcription`.
pub fn short_api_total() -> Timeouts {
    Timeouts {
        api_transcription: SHORT_LIMIT,
        ..in_time()
    }
}

/// Only the post-processing limit is short ([`SHORT_LIMIT`]), the rest in time.
pub fn short_post_processing_total() -> Timeouts {
    Timeouts {
        post_processing: SHORT_LIMIT,
        ..in_time()
    }
}

/// Only the API whole-request limit is the mid-body stall's [`STALL_LIMIT`].
pub fn stall_mid_body_api_total() -> Timeouts {
    Timeouts {
        api_transcription: STALL_LIMIT,
        ..in_time()
    }
}

/// The local-server limit short ([`SHORT_LIMIT`]), the API limit 10 s, so a client
/// that reads the API field instead gets the late answer.
pub fn short_local_server_total() -> Timeouts {
    Timeouts {
        api_transcription: Duration::from_secs(10),
        local_server: SHORT_LIMIT,
        ..in_time()
    }
}

// ---- held-lookup legs (T-079) ----------------------------------------------------

/// The API job or call under a held lookup: connect [`HELD_LOOKUP_CONNECT`], the API
/// limit the budget, the rest the production defaults.
pub fn held_lookup_api() -> Timeouts {
    Timeouts {
        connect: HELD_LOOKUP_CONNECT,
        api_transcription: OS_ANSWER_BUDGET,
        ..Timeouts::default()
    }
}

/// The connection test under a held lookup: connect [`HELD_LOOKUP_CONNECT`], both
/// engines' limits the budget, the rest the production defaults.
pub fn held_lookup_connection_test() -> Timeouts {
    Timeouts {
        connect: HELD_LOOKUP_CONNECT,
        api_transcription: OS_ANSWER_BUDGET,
        local_server: OS_ANSWER_BUDGET,
        ..Timeouts::default()
    }
}

/// Post-processing under a held lookup: connect [`HELD_LOOKUP_CONNECT`], the
/// post-processing limit the budget, the rest the production defaults.
pub fn held_lookup_post_processing() -> Timeouts {
    Timeouts {
        connect: HELD_LOOKUP_CONNECT,
        post_processing: OS_ANSWER_BUDGET,
        ..Timeouts::default()
    }
}

/// A held lookup cut by a whole-request limit ([`SHORT_LIMIT`]) far below the
/// connect limit (the budget); the rest the production defaults (decision #113).
pub fn held_lookup_total_below_connect() -> Timeouts {
    Timeouts {
        connect: OS_ANSWER_BUDGET,
        api_transcription: SHORT_LIMIT,
        ..Timeouts::default()
    }
}

// ---- OS-answer legs --------------------------------------------------------------

/// A refused case's deadlines with the whole-request limit (the budget) below the
/// connect limit (twice the budget): the refusal ends the connect as CannotReach; a
/// job that waits for the connect timer hits the request limit first (Timeout).
pub fn refused_connect_past_total(case: &OsAnswer) -> Timeouts {
    Timeouts {
        connect: 2 * OS_ANSWER_BUDGET,
        api_transcription: OS_ANSWER_BUDGET,
        ..case.timeouts
    }
}
