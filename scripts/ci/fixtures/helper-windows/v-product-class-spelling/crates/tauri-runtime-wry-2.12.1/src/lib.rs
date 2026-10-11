// Fixture (T-091, probes p3-p6 of the T-091 investigation): the product window class in four
// spellings no window_classname("<class>") reader captures (a space after the paren, a raw
// string, String::from, the literal on the next line); another class is passed as a plain
// literal. Each is a raw token of the product-window crate: listing it as a visible helper is
// refused.
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
        builder = builder.window_classname( "Tauri Window" );
        builder = builder.window_classname(r"Tauri Window");
        builder = builder.window_classname(String::from("Tauri Window"));
        builder = builder.window_classname(
            "Tauri Window");
        let _o = builder.clone().window_classname("Tauri Overlay");
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
