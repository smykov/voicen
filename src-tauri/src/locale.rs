//! The OS UI language passed to `SettingsService::load_or_init` (spec 004 T049,
//! FR-011; decision #34).
//!
//! T-030 RED SKELETON (test-writer): the bodies are stubs the developer replaces.

/// The first entry of `GetUserPreferredUILanguages(MUI_LANGUAGE_NAME)` (`"ru-RU"`);
/// `None` on any error or an empty list.
pub fn os_language() -> Option<String> {
    // RED STUB
    None
}

/// The first tag of a `GetUserPreferredUILanguages` buffer: UTF-16 tags, each
/// NUL-terminated, the list ended by an extra NUL (`ru-RU\0en-US\0\0`). `None` when
/// the list is empty or the first tag is not valid UTF-16.
pub fn first_language_tag(multi_sz: &[u16]) -> Option<String> {
    // RED STUB
    let _ = multi_sz;
    None
}
