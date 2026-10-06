// Fixture (T-065): the product window class every tauri window gets on Windows
// (tauri-runtime-wry 2.12.1 src/lib.rs WindowBuilderWrapper::new).
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
        builder = builder.window_classname("Tauri Window");
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
