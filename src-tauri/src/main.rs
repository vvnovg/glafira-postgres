// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Режим askpass для ssh (спец. P1 §5.1): Tauri не запускается.
    if std::env::var_os("GLAFIRA_ASKPASS").is_some() {
        std::process::exit(glafira_pg_lib::askpass_main());
    }
    // Шим MCP (спец. P4 §3.3): `glafira-pg mcp` — без окна и Tauri, stdio клиента ↔ сокет запущенной Glafira.
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        std::process::exit(glafira_pg_lib::mcp_main());
    }
    glafira_pg_lib::run()
}
