// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Режим askpass для ssh (спец. P1 §5.1): Tauri не запускается.
    if std::env::var_os("GLAFIRA_ASKPASS").is_some() {
        std::process::exit(glafira_pg_lib::askpass_main());
    }
    glafira_pg_lib::run()
}
