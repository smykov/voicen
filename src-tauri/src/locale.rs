//! The OS UI language passed to `SettingsService::load_or_init` (spec 004 T049,
//! FR-011; decision #34). Core's `defaults(os_tag)` maps the tag to `ru` / `en`.

/// The first entry of `GetUserPreferredUILanguages(MUI_LANGUAGE_NAME)` (`"ru-RU"`);
/// `None` on any error or an empty list.
#[cfg(windows)]
pub fn os_language() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};

    let mut count = 0u32;
    let mut len = 0u32;
    // SAFETY: a size query (no buffer); both out-pointers are valid locals.
    unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len) }.ok()?;
    if len == 0 {
        return None;
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` holds `len` UTF-16 units, the size the query asked for; the
    // call writes at most `len` units and updates `len`.
    unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut len,
        )
    }
    .ok()?;
    buffer.truncate(len as usize);
    first_language_tag(&buffer)
}

/// Not Windows: no OS language is read (the shell runs on Windows only, decision #5).
#[cfg(not(windows))]
pub fn os_language() -> Option<String> {
    None
}

/// The first tag of a `GetUserPreferredUILanguages` buffer: UTF-16 tags, each
/// NUL-terminated, the list ended by an extra NUL (`ru-RU\0en-US\0\0`). `None` when
/// the list is empty or the first tag is not valid UTF-16.
pub fn first_language_tag(multi_sz: &[u16]) -> Option<String> {
    let end = multi_sz
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(multi_sz.len());
    let first = &multi_sz[..end];
    if first.is_empty() {
        return None;
    }
    String::from_utf16(first).ok()
}
