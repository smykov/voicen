#![cfg(not(test))]
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::process::exit(if voicen_lib::run() == 7 { 0 } else { 1 });
}
