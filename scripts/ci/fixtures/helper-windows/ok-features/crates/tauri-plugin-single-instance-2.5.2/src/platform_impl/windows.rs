// Fixture (T-065): tauri-plugin-single-instance 2.5.2 src/platform_impl/windows.rs. The class
// gets a version suffix only under `feature = "semver"`, so its window facts depend on the
// resolved features, not only on crate@version.
fn init() {
    #[cfg(feature = "semver")]
    let id = format!("{id}-{}", semver_compat_string(&app.package_info().version));
    let class_name = encode_wide(format!("{id}-sic"));
}
fn create_event_target_window() {
    let hwnd = unsafe { CreateWindowExW(ex, class_name, window_name, WS_OVERLAPPED, 0, 0, 0, 0, None, None, instance, None) };
}
