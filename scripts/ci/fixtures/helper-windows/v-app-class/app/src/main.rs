// Fixture (T-091): the app's own source. The product classes it names are in
// tauri.conf.json (windowClassname, read by tauri-runtime-wry's WindowBuilderWrapper::with_config)
// and in src/tests/windows.rs (a .window_classname(...) call).
fn main() {
    voicen::run();
}
