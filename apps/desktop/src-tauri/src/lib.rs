mod commands;
mod model_catalog;
mod model_metadata;
pub mod provider_auth;
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
            if let Some(window) = app.get_webview_window("main") {
                window.set_title("")?;
                #[cfg(target_os = "macos")]
                window.set_title_bar_style(tauri::TitleBarStyle::Overlay)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::image_attachments_import,
            commands::image_attachments_import_bytes,
            commands::project_add,
            commands::project_archive,
            commands::project_archive_conversations,
            commands::project_create_worktree,
            commands::project_list,
            commands::project_personal_get_or_create,
            commands::project_pin,
            commands::project_reindex,
            commands::project_remove,
            commands::project_rename,
            commands::project_reveal_in_file_manager,
            commands::thread_list,
            commands::thread_start,
            commands::thread_read,
            commands::thread_resume,
            commands::thread_fork,
            commands::thread_compact,
            commands::thread_goal_set,
            commands::thread_goal_get,
            commands::thread_goal_clear,
            commands::agent_tree,
            commands::thread_rename,
            commands::thread_pin,
            commands::thread_archive,
            commands::thread_unarchive,
            commands::turn_start,
            commands::turn_interrupt,
            commands::approval_decision,
            commands::submit_user_input,
            commands::approvals_list,
            commands::open_question_resolve,
            commands::checkpoint_restore,
            commands::events_replay,
            commands::events_subscribe,
            commands::events_unsubscribe,
            commands::memory_search,
            commands::memory_save,
            commands::memory_update,
            commands::memory_forget,
            commands::memory_audit,
            commands::memory_list_candidates,
            commands::memory_promote,
            commands::provider_settings_get,
            commands::provider_settings_save,
            commands::provider_chatgpt_oauth_device_start,
            commands::provider_chatgpt_oauth_device_complete,
            commands::provider_github_copilot_oauth_device_start,
            commands::provider_github_copilot_oauth_device_complete,
            commands::open_external_url,
            commands::runtime_settings_get,
            commands::runtime_settings_save,
            commands::skill_catalog_scan,
            commands::provider_connection_test,
            commands::provider_models_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ExAgent");
}
