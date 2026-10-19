#[macro_use]
pub mod i18n;
pub mod catalog;
pub mod commands;
pub mod conn;
pub mod diagnostics;
pub mod error;
pub mod license;
pub mod logging;
pub mod policy;
pub mod product;
pub mod readiness;
pub mod secrets;
pub mod settings;
pub mod state;
pub mod storage;
pub mod sync;
pub mod topology;

use std::sync::Arc;

use tauri::Manager;

use crate::secrets::KeyringStore;
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            crate::logging::init(&app.path().app_log_dir()?);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
            let dir = app.path().app_data_dir()?;
            let policy = crate::policy::sources::load();
            tracing::info!(managed = policy.status.managed, "policy");
            let st = AppState::new(&dir, Arc::new(KeyringStore::default()))?.with_policy(policy);
            tracing::info!(tier = ?st.license.status().tier, "license");
            st.runtimes
                .set_sink(Arc::new(crate::commands::topology::TauriSink(
                    app.handle().clone(),
                )));
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::export_diagnostics,
            commands::catalog::list_databases,
            commands::catalog::list_schemas,
            commands::catalog::list_objects,
            commands::catalog::list_partitions,
            commands::catalog::list_extensions,
            commands::catalog::list_roles,
            commands::connections::list_connections,
            commands::connections::save_connection,
            commands::connections::delete_connection,
            commands::connections::parse_connection_string,
            commands::connections::test_connection,
            commands::folders::list_folders,
            commands::folders::create_folder,
            commands::folders::rename_folder,
            commands::folders::delete_folder,
            commands::folders::move_items,
            commands::folders::export_connections,
            commands::folders::preview_import,
            commands::folders::apply_import,
            commands::license::get_policy_status,
            commands::license::get_license_status,
            commands::license::set_license_key,
            commands::license::remove_license_key,
            commands::settings::get_language_settings,
            commands::settings::set_language,
            commands::settings::get_ui_settings,
            commands::settings::set_ui_settings,
            commands::topology::connect,
            commands::topology::disconnect,
            commands::topology::get_topology,
            commands::topology::list_topology_events,
            commands::topology::set_catalog_member,
            commands::topology::get_member_facts,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(st) = app.try_state::<AppState>()
            {
                st.runtimes.shutdown();
            }
        });
}
