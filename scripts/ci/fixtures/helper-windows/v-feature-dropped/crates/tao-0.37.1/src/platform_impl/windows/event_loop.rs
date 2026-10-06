fn create_event_target_window() {
    let hwnd = unsafe { CreateWindowExW(ex, class, title, style, 0, 0, 0, 0, None, None, instance, None) };
}
