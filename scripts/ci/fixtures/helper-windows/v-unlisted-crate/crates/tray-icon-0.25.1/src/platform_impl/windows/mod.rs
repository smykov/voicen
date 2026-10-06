fn new() {
    let hwnd = unsafe { CreateWindowExW(ex, class, title, style, 0, 0, 0, 0, None, None, instance, None) };
}
