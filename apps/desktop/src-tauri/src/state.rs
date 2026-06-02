use std::sync::Arc;

use exagent::app_server::desktop_facade::DesktopFacade;
use exagent::app_server::AppServerService;
use exagent::index_db::IndexDb;
use tokio::sync::RwLock;

use crate::settings::DesktopSettingsStore;

pub struct DesktopState {
    pub facade: RwLock<DesktopFacade>,
    pub index: IndexDb,
    pub settings: DesktopSettingsStore,
}

impl DesktopState {
    pub fn new(facade: DesktopFacade, index: IndexDb, settings: DesktopSettingsStore) -> Self {
        Self {
            facade: RwLock::new(facade),
            index,
            settings,
        }
    }

    pub async fn rebuild_facade_from_settings(&self) -> anyhow::Result<()> {
        let facade =
            desktop_facade_from_settings(self.index.clone(), self.settings.clone()).await?;
        *self.facade.write().await = facade;
        Ok(())
    }
}

pub async fn desktop_facade_from_settings(
    index: IndexDb,
    settings: DesktopSettingsStore,
) -> anyhow::Result<DesktopFacade> {
    let config = settings.runtime_config().await?;
    Ok(DesktopFacade::new(
        AppServerService::with_config_and_model_resolver(config, Arc::new(settings)),
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
    use crate::settings::{DesktopSettingsStore, ProviderSettingsSaveRequest, SecretStore};

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
                    workspace_root: None,
                    turn_context: Some(TurnContextOverrides {
                        cwd: None,
                        model: Some(ModelRef::new("openai_compatible", "settings-model")),
                        thinking_mode: None,
                    }),
                },
            )
            .await
            .unwrap();

        assert!(saw_saved_key.load(Ordering::SeqCst));
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
