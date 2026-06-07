use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_API_URL_ENV: &str = "EXAGENT_MODELS_DEV_API_URL";
const MODELS_DEV_DISABLED_ENV: &str = "EXAGENT_MODELS_DEV_DISABLED";
const MODELS_DEV_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const MODELS_DEV_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsDevCatalog {
    providers: HashMap<String, HashMap<String, ModelsDevModelMetadata>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelsDevModelMetadata {
    pub context_window: Option<i64>,
    pub supports_tools: Option<bool>,
    pub reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelsDevCacheFile {
    fetched_at_ms: u64,
    catalog: Value,
}

impl ModelsDevCatalog {
    pub fn metadata_for(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<&ModelsDevModelMetadata> {
        for &provider_key in provider_aliases(provider_id) {
            if let Some(metadata) = self
                .providers
                .get(provider_key)
                .and_then(|models| models.get(model_id))
            {
                return Some(metadata);
            }
        }
        None
    }
}

pub fn has_models_dev_provider_alias(provider_id: &str) -> bool {
    !provider_aliases(provider_id).is_empty()
}

pub async fn load_models_dev_catalog(cache_path: &Path) -> Option<ModelsDevCatalog> {
    if models_dev_disabled() {
        return None;
    }

    let cached = read_cache(cache_path).await;
    if let Some(cache) = cached.as_ref().filter(|cache| cache_is_fresh(cache)) {
        return Some(parse_models_dev_catalog(&cache.catalog));
    }

    let stale_catalog = cached
        .as_ref()
        .map(|cache| parse_models_dev_catalog(&cache.catalog));

    match fetch_models_dev_catalog().await {
        Some(catalog_value) => {
            let catalog = parse_models_dev_catalog(&catalog_value);
            write_cache(cache_path, catalog_value).await;
            Some(catalog)
        }
        None => stale_catalog,
    }
}

pub async fn load_cached_models_dev_catalog(cache_path: &Path) -> Option<ModelsDevCatalog> {
    if models_dev_disabled() {
        return None;
    }

    read_cache(cache_path)
        .await
        .map(|cache| parse_models_dev_catalog(&cache.catalog))
}

fn provider_aliases(provider_id: &str) -> &'static [&'static str] {
    match provider_id {
        "openai" => &["openai"],
        "anthropic" => &["anthropic"],
        "google" => &["google"],
        "deepseek" => &["deepseek"],
        "kimi" => &["moonshotai", "moonshotai-cn"],
        "glm" => &["zai", "zhipuai"],
        _ => &[],
    }
}

fn models_dev_disabled() -> bool {
    std::env::var(MODELS_DEV_DISABLED_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

async fn fetch_models_dev_catalog() -> Option<Value> {
    let url = std::env::var(MODELS_DEV_API_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| MODELS_DEV_API_URL.to_string());
    let client = reqwest::Client::builder()
        .timeout(MODELS_DEV_TIMEOUT)
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

async fn read_cache(cache_path: &Path) -> Option<ModelsDevCacheFile> {
    let contents = tokio::fs::read_to_string(cache_path).await.ok()?;
    serde_json::from_str(&contents).ok()
}

async fn write_cache(cache_path: &Path, catalog: Value) {
    let Some(parent) = cache_path.parent() else {
        return;
    };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    let cache = ModelsDevCacheFile {
        fetched_at_ms: unix_timestamp_millis(),
        catalog,
    };
    let Ok(contents) = serde_json::to_string(&cache) else {
        return;
    };
    let _ = tokio::fs::write(cache_path, contents).await;
}

fn cache_is_fresh(cache: &ModelsDevCacheFile) -> bool {
    unix_timestamp_millis()
        .checked_sub(cache.fetched_at_ms)
        .is_some_and(|age| age <= MODELS_DEV_CACHE_TTL_MS)
}

fn parse_models_dev_catalog(value: &Value) -> ModelsDevCatalog {
    let mut providers = HashMap::new();
    let Some(provider_values) = value.as_object() else {
        return ModelsDevCatalog::default();
    };

    for (provider_key, provider_value) in provider_values {
        let Some(model_values) = provider_value.get("models").and_then(Value::as_object) else {
            continue;
        };
        let mut models = HashMap::new();
        for (model_key, model_value) in model_values {
            models.insert(model_key.clone(), parse_model_metadata(model_value));
        }
        if !models.is_empty() {
            providers.insert(provider_key.clone(), models);
        }
    }

    ModelsDevCatalog { providers }
}

fn parse_model_metadata(value: &Value) -> ModelsDevModelMetadata {
    ModelsDevModelMetadata {
        context_window: value
            .get("limit")
            .and_then(|limit| limit.get("context"))
            .and_then(Value::as_i64),
        supports_tools: value.get("tool_call").and_then(Value::as_bool),
        reasoning: value.get("reasoning").and_then(Value::as_bool),
    }
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}
