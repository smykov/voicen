fn drag_resize_borders() {
    let hwnd = unsafe { CreateWindowExW(ex, class, title, WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, Some(parent), None, instance, None) };
}
