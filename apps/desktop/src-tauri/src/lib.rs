mod commands;
pub mod settings;
mod state;

use tauri::Manager;

use state::DesktopState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let db_path = app_data_dir.join("exagent.sqlite");
            let settings = settings::DesktopSettingsStore::new(app_data_dir.join("settings.json"));
            let index = tauri::async_runtime::block_on(exagent::index_db::IndexDb::open(db_path))?;
            let facade = tauri::async_runtime::block_on(state::desktop_facade_from_settings(
                index.clone(),
                settings.clone(),
            ))?;
            app.manage(DesktopState::new(facade, index, settings));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project_add,
            commands::project_list,
            commands::project_reindex,
            commands::thread_list,
            commands::thread_start,
            commands::thread_read,
            commands::thread_resume,
            commands::thread_rename,
            commands::thread_pin,
            commands::thread_archive,
            commands::thread_unarchive,
            commands::turn_start,
            commands::turn_interrupt,
            commands::approval_decision,
            commands::events_replay,
            commands::events_subscribe,
            commands::provider_settings_get,
            commands::provider_settings_save,
            commands::runtime_settings_get,
            commands::runtime_settings_save,
            commands::provider_connection_test,
            commands::provider_models_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ExAgent Desktop");
}
