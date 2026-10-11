// Fixture (T-091): the app names tao's class only in another case. Classes compare ordinally
// (VoicenWindows.Shown, helper_windows::is_helper), so this is not the listed class.
const NOT_THE_HELPER: &str = "tao thread event target";
fn main() {
    voicen::run();
}
