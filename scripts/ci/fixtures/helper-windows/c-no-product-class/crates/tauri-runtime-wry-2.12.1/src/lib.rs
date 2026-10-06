// Fixture (T-065): a runtime source that passes no class literal to window_classname: the
// product window class cannot be read from the graph, so the product-class rule cannot run.
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
