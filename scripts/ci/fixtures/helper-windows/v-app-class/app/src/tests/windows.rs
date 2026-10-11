// Fixture (T-091): product code under src/ (src-tauri/src holds no #[cfg(test)]), in a module
// dir that happens to be named tests: only the app dir's top-level tests/ is excluded. The class
// is {identifier}-main with the identifier of this app dir's tauri.conf.json.
pub fn overlay(app: &AppHandle) {
    let _w = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::default())
        .window_classname("com.example.voicen-fixture-main")
        .build();
}
