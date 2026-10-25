#[macro_use]
pub mod i18n;
pub mod actions;
pub mod catalog;
pub mod commands;
pub mod conn;
pub mod diag;
pub mod diagnostics;
pub mod error;
pub mod guard;
pub mod logging;
pub mod migrate;
pub mod net;
pub mod policy;
pub mod product;
pub mod readiness;
pub mod sampler;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod state;
pub mod storage;
pub mod sync;
pub mod topology;

use std::sync::Arc;

use tauri::Manager;
use tauri::webview::PageLoadEvent;

use crate::secrets::KeyringStore;
use crate::state::AppState;

/// Режим askpass для ssh (спец. P1 §5.1): тот же бинарь, запущенный с `GLAFIRA_ASKPASS`, отвечает на запрос
/// ssh и завершается. Возвращает код выхода процесса.
pub fn askpass_main() -> i32 {
    // `args_os`, а не `args`: `args` паникует на аргументе не в UTF-8, а путь к ключу в запросе ssh может быть
    // в любой кодировке.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    crate::net::askpass::run(&args)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Один провайдер TLS на процесс: плагин updater при первой проверке ставит `ring`, а reqwest
    // (клиенты Patroni) без провайдера берёт `aws-lc-rs`. Иначе поведение TLS зависело бы от того,
    // была ли проверка обновлений. Повторная установка (например, в тестах) безвредна.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Перезагрузка страницы (F5, горячая замена Vite) теряет JS-состояние: id длительных сессий забыты, а
        // канал сэмплера в Tauri 2.12 после перезагрузки принимает отправку без ошибки, так что «закрытый
        // канал» не наступит. Начало загрузки страницы останавливает все сессии: сэмплеры не работают вхолостую
        // до закрытия подключения. Окно одно, а новая страница начинает с `start_sampler` уже после `Started`;
        // при первой загрузке сессий нет, вызов безвреден.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started
                && let Some(st) = webview.app_handle().try_state::<AppState>()
            {
                st.sessions.stop_all();
            }
        })
        .setup(|app| {
            crate::logging::init(&app.path().app_log_dir()?);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
            let dir = app.path().app_data_dir()?;
            let policy = crate::policy::sources::load();
            tracing::info!(
                managed = policy.status.managed,
                updates = policy.status.updates_enabled,
                "policy"
            );
            let st = AppState::new_with_logs(
                &dir,
                &app.path().app_log_dir()?,
                Arc::new(KeyringStore::default()),
            )?
            .with_policy(policy);
            st.runtimes
                .set_sink(Arc::new(crate::commands::topology::TauriSink(
                    app.handle().clone(),
                )));
            // Сироты прошлого запуска (`ps`/PowerShell на каждый PID), логи и секреты его проверок подключения
            // (keychain) — в фоне; первый туннель и первый секрет проверки дождутся уборки.
            let tunnels = st.tunnels.clone();
            tauri::async_runtime::spawn_blocking(move || {
                tracing::info!(reaped = tunnels.reap_orphans(), "ssh orphans");
            });
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::export_diagnostics,
            commands::app::check_update,
            commands::app::install_update,
            commands::catalog::list_databases,
            commands::catalog::list_schemas,
            commands::catalog::list_objects,
            commands::catalog::list_partitions,
            commands::catalog::list_extensions,
            commands::catalog::list_roles,
            commands::catalog::get_table,
            commands::catalog::get_view,
            commands::catalog::get_function,
            commands::catalog::get_sequence,
            commands::connections::list_connections,
            commands::connections::save_connection,
            commands::connections::delete_connection,
            commands::connections::connection_secrets,
            commands::connections::parse_connection_string,
            commands::connections::test_connection,
            commands::connections::trust_ssh_host_key,
            commands::diag::run_diagnostic,
            commands::diag::start_sampler,
            commands::diag::stop_session,
            commands::actions::list_actions,
            commands::folders::list_folders,
            commands::folders::create_folder,
            commands::folders::rename_folder,
            commands::folders::delete_folder,
            commands::folders::move_items,
            commands::folders::export_connections,
            commands::folders::preview_import,
            commands::folders::apply_import,
            commands::settings::get_policy_status,
            commands::settings::get_language_settings,
            commands::settings::set_language,
            commands::settings::get_ui_settings,
            commands::settings::set_ui_settings,
            commands::topology::connect,
            commands::topology::disconnect,
            commands::topology::reconnect,
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
                // Длительные сессии (сэмплеры), затем опросы и туннели; процессы ssh завершаются без рантайма —
                // по PID из `tunnels.json`.
                st.sessions.stop_all();
                st.runtimes.shutdown();
                st.tunnels.close_all_blocking();
            }
        });
}
