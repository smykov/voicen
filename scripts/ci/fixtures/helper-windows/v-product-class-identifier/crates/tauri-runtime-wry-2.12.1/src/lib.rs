// Fixture (T-091): the runtime names the class {identifier}-main with the app's identifier
// (com.example.voicen-fixture, the app dir's tauri.conf.json): a visible-helper class is
// resolved before it is searched in the product-window crate.
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
        builder = builder.window_classname("com.example.voicen-fixture-main");
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
