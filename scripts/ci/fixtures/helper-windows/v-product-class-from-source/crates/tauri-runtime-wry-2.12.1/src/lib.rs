// Fixture (T-065): the product window class every tauri window gets on Windows
// (here a runtime whose default product class is "Voicen Main Window": read from the source, not hard-coded).
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
        builder = builder.window_classname("Voicen Main Window");
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
