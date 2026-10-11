// Fixture (T-091, probe p1 of the T-086 audit): the product window class reaches
// window_classname through a const, and another class is passed as a literal. The class is
// still a raw token of the product-window crate: listing it as a visible helper is refused.
fn new() -> Self {
    let mut builder = Self::default().focused(true);
    builder = builder.title("Tauri App");
    #[cfg(windows)]
    {
        const PRODUCT_CLASS: &str = "Tauri Window"; builder = builder.window_classname(PRODUCT_CLASS); let _o = builder.clone().window_classname("Tauri Overlay");
    }
    builder
}
fn window_classname<S: Into<String>>(mut self, window_classname: S) -> Self {
    self.inner = self.inner.with_window_classname(window_classname);
    self
}
