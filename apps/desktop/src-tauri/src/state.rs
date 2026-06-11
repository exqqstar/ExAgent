use std::collections::HashMap;
use std::sync::Arc;

use exagent::app_server::desktop_facade::DesktopFacade;
use exagent::app_server::AppServerService;
use exagent::index_db::IndexDb;
use exagent::model::factory::DefaultLlmClientFactory;
use tauri::async_runtime::JoinHandle;
use tokio::sync::{Mutex, RwLock};

use crate::settings::DesktopSettingsStore;
use crate::settings::{RuntimeSettingsResponse, RuntimeSettingsSaveRequest};

pub struct DesktopState {
    pub facade: RwLock<DesktopFacade>,
    pub index: IndexDb,
    pub settings: DesktopSettingsStore,
    pub event_subscriptions: DesktopEventSubscriptions,
}

impl DesktopState {
    pub fn new(facade: DesktopFacade, index: IndexDb, settings: DesktopSettingsStore) -> Self {
        Self {
            facade: RwLock::new(facade),
            index,
            settings,
            event_subscriptions: DesktopEventSubscriptions::default(),
        }
    }

    pub async fn rebuild_facade_from_settings(&self) -> anyhow::Result<()> {
        let facade =
            desktop_facade_from_settings(self.index.clone(), self.settings.clone()).await?;
        *self.facade.write().await = facade;
        Ok(())
    }

    pub async fn save_runtime_settings(
        &self,
        request: RuntimeSettingsSaveRequest,
    ) -> anyhow::Result<RuntimeSettingsResponse> {
        let response = self.settings.save_runtime_settings(request).await?;
        self.rebuild_facade_from_settings().await?;
        Ok(response)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DesktopEventSubscriptionKey {
    window_label: String,
    project_id: String,
    thread_id: String,
}

#[derive(Default)]
pub struct DesktopEventSubscriptions {
    tasks: Mutex<HashMap<DesktopEventSubscriptionKey, JoinHandle<()>>>,
}

impl DesktopEventSubscriptions {
    pub async fn replace(
        &self,
        window_label: String,
        project_id: String,
        thread_id: String,
        handle: JoinHandle<()>,
    ) {
        let key = DesktopEventSubscriptionKey {
            window_label,
            project_id,
            thread_id,
        };
        let mut tasks = self.tasks.lock().await;
        let stale_keys = tasks
            .keys()
            .filter(|candidate| {
                candidate.window_label == key.window_label
                    && candidate.project_id == key.project_id
                    && candidate.thread_id != key.thread_id
            })
            .cloned()
            .collect::<Vec<_>>();
        for stale_key in stale_keys {
            if let Some(previous) = tasks.remove(&stale_key) {
                previous.abort();
            }
        }
        if let Some(previous) = tasks.insert(key, handle) {
            previous.abort();
        }
    }

    pub async fn unsubscribe(&self, window_label: &str, project_id: &str, thread_id: &str) -> bool {
        let key = DesktopEventSubscriptionKey {
            window_label: window_label.to_string(),
            project_id: project_id.to_string(),
            thread_id: thread_id.to_string(),
        };
        if let Some(handle) = self.tasks.lock().await.remove(&key) {
            handle.abort();
            return true;
        }
        false
    }

    #[cfg(test)]
    pub async fn active_count(&self) -> usize {
        self.tasks.lock().await.len()
    }
}

pub async fn desktop_facade_from_settings(
    index: IndexDb,
    settings: DesktopSettingsStore,
) -> anyhow::Result<DesktopFacade> {
    let config = settings.runtime_config().await?;
    let goal_store = index.clone();
    let llm_factory = Arc::new(DefaultLlmClientFactory::with_chatgpt_token_refresh_sink(
        Arc::new(settings.clone()),
    ));
    Ok(DesktopFacade::new(
        AppServerService::with_config_llm_factory_model_resolver_and_goal_store(
            config,
            llm_factory,
            Arc::new(settings),
            goal_store,
        ),
        index,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };

    use axum::{http::header::AUTHORIZATION, http::HeaderMap, routing::post, Json, Router};
    use exagent::app_server::protocol::{TurnContextOverrides, TurnStartParams};
    use exagent::app_server::{desktop_facade::NewProjectRequest, AppServerService};
    use exagent::resolved::ModelRef;
    use serde_json::json;
    use tempfile::tempdir;

    use super::DesktopState;
    use crate::settings::{
        DesktopSettingsStore, McpServerSettings, ProviderSettingsSaveRequest,
        RuntimeSettingsSaveRequest, SecretStore, SkillRootSettings,
    };

    #[derive(Default)]
    struct MemorySecrets {
        values: Mutex<HashMap<String, String>>,
    }

    impl SecretStore for MemorySecrets {
        fn get_secret(&self, account: &str) -> anyhow::Result<Option<String>> {
            Ok(self.values.lock().unwrap().get(account).cloned())
        }

        fn set_secret(&self, account: &str, secret: &str) -> anyhow::Result<()> {
            self.values
                .lock()
                .unwrap()
                .insert(account.to_string(), secret.to_string());
            Ok(())
        }

        fn delete_secret(&self, account: &str) -> anyhow::Result<()> {
            self.values.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[tokio::test]
    async fn rebuilt_facade_keeps_desktop_settings_model_resolver() {
        let saw_saved_key = Arc::new(AtomicBool::new(false));
        let base_url = spawn_chat_server(saw_saved_key.clone()).await;
        let dir = tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let settings =
            DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);
        settings
            .save_provider_settings(ProviderSettingsSaveRequest {
                provider_id: "openai_compatible".into(),
                base_url,
                model: "settings-model".into(),
                api_key: Some("sk-settings".into()),
                clear_api_key: false,
                credential_id: None,
                create_credential: false,
                model_options: Vec::new(),
            })
            .await
            .unwrap();
        let index = exagent::index_db::IndexDb::open(dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let initial_config = settings.runtime_config().await.unwrap();
        let state = DesktopState::new(
            exagent::app_server::desktop_facade::DesktopFacade::new(
                AppServerService::with_config(initial_config),
                index.clone(),
            ),
            index,
            settings,
        );

        state.rebuild_facade_from_settings().await.unwrap();
        let project = state
            .facade
            .read()
            .await
            .add_project(NewProjectRequest {
                name: "Project".into(),
                path: dir.path().to_path_buf(),
            })
            .await
            .unwrap();
        let thread = state
            .facade
            .read()
            .await
            .start_thread(&project.id)
            .await
            .unwrap()
            .thread;

        state
            .facade
            .read()
            .await
            .start_turn(
                &project.id,
                TurnStartParams {
                    thread_id: thread.id,
                    prompt: "hello".into(),
                    input: vec![],
                    workspace_root: None,
                    turn_mode: Default::default(),
                    turn_context: Some(TurnContextOverrides {
                        cwd: None,
                        model: Some(ModelRef::new("openai_compatible", "settings-model")),
                        thinking_mode: None,
                        clear_thinking_mode: false,
                    }),
                },
            )
            .await
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !saw_saved_key.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("turn should use the saved provider key");
    }

    #[tokio::test]
    async fn runtime_settings_save_rebuilds_facade_with_saved_mcp_servers() {
        let dir = tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let settings =
            DesktopSettingsStore::with_secret_store(dir.path().join("settings.json"), secrets);
        let initial_index = exagent::index_db::IndexDb::open(dir.path().join("initial.sqlite"))
            .await
            .unwrap();
        let rebuilt_index = exagent::index_db::IndexDb::open(dir.path().join("rebuilt.sqlite"))
            .await
            .unwrap();
        let initial_config = settings.runtime_config().await.unwrap();
        let state = DesktopState::new(
            exagent::app_server::desktop_facade::DesktopFacade::new(
                AppServerService::with_config(initial_config),
                initial_index.clone(),
            ),
            rebuilt_index.clone(),
            settings,
        );

        let response = state
            .save_runtime_settings(RuntimeSettingsSaveRequest {
                default_model: "gpt-4.1-mini".into(),
                default_thinking_mode: None,
                presets: Vec::new(),
                mcp_servers: vec![McpServerSettings {
                    id: "records".into(),
                    name: "Records".into(),
                    enabled: true,
                    command: "node".into(),
                    args: vec!["server.js".into()],
                    env: vec![("MCP_LOG_LEVEL".into(), "debug".into())],
                    working_directory: None,
                }],
                skill_roots: vec![SkillRootSettings {
                    id: "local-skills".into(),
                    name: "Local skills".into(),
                    enabled: true,
                    path: dir.path().display().to_string(),
                    scope: "global".into(),
                }],
            })
            .await
            .unwrap();

        assert_eq!(response.mcp_servers[0].id, "records");
        state
            .facade
            .read()
            .await
            .add_project(NewProjectRequest {
                name: "Rebuilt".into(),
                path: dir.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert!(initial_index.list_projects().await.unwrap().is_empty());
        assert_eq!(rebuilt_index.list_projects().await.unwrap().len(), 1);

        let config = state.settings.runtime_config().await.unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].id, "records");
        assert_eq!(config.mcp_servers[0].command, "node");
    }

    #[tokio::test]
    async fn event_subscriptions_replace_and_unsubscribe_abort_tasks() {
        let subscriptions = super::DesktopEventSubscriptions::default();
        let (first_started_tx, first_started_rx) = tokio::sync::oneshot::channel();
        let (first_dropped_tx, first_dropped_rx) = tokio::sync::oneshot::channel();
        let first_handle = tauri::async_runtime::spawn(drop_signalled_pending_task(
            first_started_tx,
            first_dropped_tx,
        ));

        subscriptions
            .replace(
                "main".into(),
                "project-a".into(),
                "thread-a".into(),
                first_handle,
            )
            .await;
        first_started_rx.await.expect("first task should start");

        let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
        let (second_dropped_tx, second_dropped_rx) = tokio::sync::oneshot::channel();
        let second_handle = tauri::async_runtime::spawn(drop_signalled_pending_task(
            second_started_tx,
            second_dropped_tx,
        ));
        subscriptions
            .replace(
                "main".into(),
                "project-a".into(),
                "thread-a".into(),
                second_handle,
            )
            .await;
        second_started_rx.await.expect("second task should start");

        tokio::time::timeout(std::time::Duration::from_secs(1), first_dropped_rx)
            .await
            .expect("replaced subscription task should be aborted")
            .expect("drop signal should be sent");
        assert_eq!(subscriptions.active_count().await, 1);

        assert!(
            subscriptions
                .unsubscribe("main", "project-a", "thread-a")
                .await
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), second_dropped_rx)
            .await
            .expect("unsubscribed task should be aborted")
            .expect("drop signal should be sent");
        assert_eq!(subscriptions.active_count().await, 0);
    }

    #[tokio::test]
    async fn event_subscriptions_keep_one_thread_per_window_project() {
        let subscriptions = super::DesktopEventSubscriptions::default();
        let (first_started_tx, first_started_rx) = tokio::sync::oneshot::channel();
        let (first_dropped_tx, first_dropped_rx) = tokio::sync::oneshot::channel();
        let first_handle = tauri::async_runtime::spawn(drop_signalled_pending_task(
            first_started_tx,
            first_dropped_tx,
        ));
        subscriptions
            .replace(
                "main".into(),
                "project-a".into(),
                "thread-a".into(),
                first_handle,
            )
            .await;
        first_started_rx.await.expect("first task should start");

        let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
        let (second_dropped_tx, second_dropped_rx) = tokio::sync::oneshot::channel();
        let second_handle = tauri::async_runtime::spawn(drop_signalled_pending_task(
            second_started_tx,
            second_dropped_tx,
        ));
        subscriptions
            .replace(
                "main".into(),
                "project-a".into(),
                "thread-b".into(),
                second_handle,
            )
            .await;
        second_started_rx.await.expect("second task should start");

        tokio::time::timeout(std::time::Duration::from_secs(1), first_dropped_rx)
            .await
            .expect("switching threads should abort the previous subscription")
            .expect("drop signal should be sent");
        assert_eq!(subscriptions.active_count().await, 1);

        assert!(
            subscriptions
                .unsubscribe("main", "project-a", "thread-b")
                .await
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), second_dropped_rx)
            .await
            .expect("new subscription task should be aborted")
            .expect("drop signal should be sent");
        assert_eq!(subscriptions.active_count().await, 0);
    }

    struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    async fn drop_signalled_pending_task(
        started_sender: tokio::sync::oneshot::Sender<()>,
        drop_sender: tokio::sync::oneshot::Sender<()>,
    ) {
        let _drop_signal = DropSignal(Some(drop_sender));
        let _ = started_sender.send(());
        std::future::pending::<()>().await;
    }

    async fn spawn_chat_server(saw_saved_key: Arc<AtomicBool>) -> String {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap| {
                let saw_saved_key = saw_saved_key.clone();
                async move {
                    if headers
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        == Some("Bearer sk-settings")
                    {
                        saw_saved_key.store(true, Ordering::SeqCst);
                    }
                    Json(json!({
                        "choices": [{
                            "message": {
                                "content": "ok"
                            }
                        }]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/v1")
    }
}
