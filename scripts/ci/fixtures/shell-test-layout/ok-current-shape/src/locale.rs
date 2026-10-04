#[cfg(windows)]
pub fn os_language() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn os_language() -> Option<String> {
    None
}
