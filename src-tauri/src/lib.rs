#[macro_use]
pub mod i18n;
#[macro_use]
pub mod product;
pub mod actions;
pub mod agent;
pub mod catalog;
pub mod commands;
pub mod conn;
pub mod diag;
pub mod diagnostics;
pub mod error;
pub mod explain;
pub mod guard;
pub mod logging;
pub mod mcp;
pub mod migrate;
pub mod net;
pub mod policy;
pub mod readiness;
pub mod sampler;
pub mod secrets;
pub mod session;
pub mod settings;
pub mod snapshots;
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

/// Режим `glafira-pg mcp` (спец. P4 §3.3): шим MCP без Tauri и окна — stdio клиента ↔ сокет запущенной Glafira.
/// Возвращает код выхода процесса.
pub fn mcp_main() -> i32 {
    crate::mcp::shim::run()
}

/// Сколько ждать остановки MCP-сервера при выходе: сессии закрываются сразу, ожидание — страховка от зависшего
/// соединения.
const MCP_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// MCP при выходе из приложения (`RunEvent::Exit`): клиенты отключаются, `mcp.json` удаляется (шим поймёт, что
/// Glafira закрыта). Не уложились — файл удаляется и сессии закрываются без ожидания.
fn shutdown_mcp(st: &AppState) {
    let mcp = st.mcp.clone();
    let stopped = tauri::async_runtime::block_on(async move {
        tokio::time::timeout(MCP_EXIT_GRACE, mcp.shutdown())
            .await
            .is_ok()
    });
    if !stopped {
        tracing::warn!(
            "mcp server did not stop within {MCP_EXIT_GRACE:?}: exiting without waiting"
        );
        st.mcp.exit_now();
    }
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
            // Шим `glafira-pg mcp` ищет `mcp.json` без Tauri (`dirs::data_dir()` + идентификатор): расхождение с
            // каталогом приложения — предупреждение в журнал (спец. P4 §3.3).
            let shim_dir = crate::mcp::files::data_dir();
            if shim_dir.as_deref() != Some(dir.as_path()) {
                tracing::warn!(
                    shim = ?shim_dir,
                    app = %dir.display(),
                    "mcp: the shim looks for mcp.json in another data directory"
                );
            }
            st.mcp.set_app_handle(app.handle().clone());
            st.llm.set_app_handle(app.handle().clone());
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
            // MCP-сервер — если он включён в «Настройках» и политика разрешает.
            let (mcp, for_mcp) = (st.mcp.clone(), st.clone());
            tauri::async_runtime::spawn(async move { mcp.start_if_enabled(for_mcp).await });
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
            commands::queries::query_texts,
            commands::queries::start_statements_sampler,
            commands::queries::query_detail,
            commands::queries::query_llm_bundle,
            commands::explain::explain,
            commands::explain::parse_plan,
            commands::explain::hypo_check,
            commands::llm::create_optimization_request,
            commands::llm::cancel_optimization_request,
            commands::llm::list_optimization_requests,
            commands::llm::list_llm_proposals,
            commands::llm::answer_llm_proposal,
            commands::llm::compare_llm_proposal,
            commands::actions::list_actions,
            commands::actions::preview_session_action,
            commands::actions::execute_session_action,
            commands::actions::preview_drop_slot,
            commands::actions::drop_slot,
            commands::actions::preview_patroni_action,
            commands::actions::execute_patroni_action,
            commands::folders::list_folders,
            commands::folders::create_folder,
            commands::folders::rename_folder,
            commands::folders::delete_folder,
            commands::folders::move_items,
            commands::folders::export_connections,
            commands::folders::preview_import,
            commands::folders::apply_import,
            commands::snapshots::snapshot_take,
            commands::snapshots::snapshot_list,
            commands::snapshots::snapshot_delete,
            commands::snapshots::snapshot_compare,
            commands::settings::get_policy_status,
            commands::settings::get_language_settings,
            commands::settings::set_language,
            commands::settings::get_ui_settings,
            commands::settings::set_ui_settings,
            commands::mcp::get_mcp_status,
            commands::mcp::set_mcp_enabled,
            commands::mcp::answer_mcp_approval,
            commands::mcp::revoke_mcp_client,
            commands::mcp::disconnect_mcp_sessions,
            commands::mcp::get_mcp_audit,
            commands::mcp::get_mcp_client_configs,
            commands::mcp::reveal_mcp_audit,
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
                // MCP — первым: клиенты отключаются до закрытия подключений, `mcp.json` удаляется.
                shutdown_mcp(&st);
                // Длительные сессии (сэмплеры), затем опросы и туннели; процессы ssh завершаются без рантайма —
                // по PID из `tunnels.json`.
                st.sessions.stop_all();
                st.runtimes.shutdown();
                st.tunnels.close_all_blocking();
            }
        });
}
