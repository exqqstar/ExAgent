use std::error::Error;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::AgentConfig;
use crate::policy::PolicyMode;
use crate::tools::web::web_fetch::{WebFetchService, WebFetchServiceOutput};
use crate::tools::web::web_search::SearchProvider;
use crate::types::ToolStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSearchRequest {
    pub query: String,
    pub max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSearchResponse {
    pub query: String,
    pub provider: String,
    pub results: Vec<WorkflowSearchResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub rank: usize,
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowSourceError {
    InvalidQuery,
    Provider(String),
}

impl fmt::Display for WorkflowSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuery => formatter.write_str("workflow source query is empty"),
            Self::Provider(error) => write!(formatter, "workflow source provider failed: {error}"),
        }
    }
}

impl Error for WorkflowSourceError {}

#[async_trait]
pub trait WorkflowSourceSearch: Send + Sync {
    async fn search(
        &self,
        request: WorkflowSearchRequest,
    ) -> Result<WorkflowSearchResponse, WorkflowSourceError>;
}

pub struct SearchProviderWorkflowSearchAdapter {
    provider: Arc<dyn SearchProvider>,
}

impl SearchProviderWorkflowSearchAdapter {
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl WorkflowSourceSearch for SearchProviderWorkflowSearchAdapter {
    async fn search(
        &self,
        request: WorkflowSearchRequest,
    ) -> Result<WorkflowSearchResponse, WorkflowSourceError> {
        let query = request.query.trim();
        if query.is_empty() {
            return Err(WorkflowSourceError::InvalidQuery);
        }

        let provider = self.provider.provider_name().to_string();
        let results = self
            .provider
            .search(query, request.max_results)
            .await
            .map_err(WorkflowSourceError::Provider)?
            .into_iter()
            .enumerate()
            .map(|(index, result)| WorkflowSearchResult {
                title: result.title,
                url: result.url,
                snippet: result.snippet,
                rank: index + 1,
                provider: provider.clone(),
            })
            .collect();

        Ok(WorkflowSearchResponse {
            query: query.to_string(),
            provider,
            results,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFetchRequest {
    pub url: String,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFetchStatus {
    Fetched,
    ApprovalBlocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowFetchOutput {
    pub url: String,
    pub final_url: Option<String>,
    pub status: WorkflowFetchStatus,
    pub content: Option<String>,
    pub http_status: Option<u16>,
    pub content_type: Option<String>,
    pub body_bytes: Option<usize>,
    pub body_truncated: Option<bool>,
    pub output_truncated: Option<bool>,
    pub policy_decision: Option<String>,
    pub error: Option<String>,
}

#[async_trait]
pub trait WorkflowSourceFetch: Send + Sync {
    async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput;
}

pub trait WorkflowSourceProvider: WorkflowSourceSearch + WorkflowSourceFetch {}

impl<T> WorkflowSourceProvider for T where T: WorkflowSourceSearch + WorkflowSourceFetch {}

pub struct RuntimeWorkflowSourceProvider {
    search: SearchProviderWorkflowSearchAdapter,
    fetch: WebFetchWorkflowFetchAdapter,
}

impl RuntimeWorkflowSourceProvider {
    pub fn new(search_provider: Arc<dyn SearchProvider>, config: AgentConfig) -> Self {
        Self {
            search: SearchProviderWorkflowSearchAdapter::new(search_provider),
            fetch: WebFetchWorkflowFetchAdapter::new(config),
        }
    }
}

#[async_trait]
impl WorkflowSourceSearch for RuntimeWorkflowSourceProvider {
    async fn search(
        &self,
        request: WorkflowSearchRequest,
    ) -> Result<WorkflowSearchResponse, WorkflowSourceError> {
        self.search.search(request).await
    }
}

#[async_trait]
impl WorkflowSourceFetch for RuntimeWorkflowSourceProvider {
    async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput {
        self.fetch.fetch(request).await
    }
}

pub struct UnavailableWorkflowSourceProvider {
    reason: String,
}

impl UnavailableWorkflowSourceProvider {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl WorkflowSourceSearch for UnavailableWorkflowSourceProvider {
    async fn search(
        &self,
        _request: WorkflowSearchRequest,
    ) -> Result<WorkflowSearchResponse, WorkflowSourceError> {
        Err(WorkflowSourceError::Provider(self.reason.clone()))
    }
}

#[async_trait]
impl WorkflowSourceFetch for UnavailableWorkflowSourceProvider {
    async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput {
        WorkflowFetchOutput {
            url: request.url,
            final_url: None,
            status: WorkflowFetchStatus::Failed,
            content: None,
            http_status: None,
            content_type: None,
            body_bytes: None,
            body_truncated: None,
            output_truncated: None,
            policy_decision: Some("deny".to_string()),
            error: Some(self.reason.clone()),
        }
    }
}

pub struct WebFetchWorkflowFetchAdapter {
    service: WebFetchService,
    config: AgentConfig,
}

impl WebFetchWorkflowFetchAdapter {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            service: WebFetchService,
            config,
        }
    }
}

#[async_trait]
impl WorkflowSourceFetch for WebFetchWorkflowFetchAdapter {
    async fn fetch(&self, request: WorkflowFetchRequest) -> WorkflowFetchOutput {
        if !matches!(self.config.policy_mode, PolicyMode::Off) {
            let prepared = match self
                .service
                .validate_request(&request.url, request.timeout_secs)
            {
                Ok(prepared) => prepared,
                Err(error) => return failed_fetch_output(request.url, error, "deny"),
            };
            return WorkflowFetchOutput {
                url: prepared.url().to_string(),
                final_url: None,
                status: WorkflowFetchStatus::ApprovalBlocked,
                content: None,
                http_status: None,
                content_type: None,
                body_bytes: None,
                body_truncated: None,
                output_truncated: None,
                policy_decision: Some("review_required".to_string()),
                error: Some("network fetch requires approval".to_string()),
            };
        }

        let prepared = match self
            .service
            .prepare_request(&request.url, request.timeout_secs)
        {
            Ok(prepared) => prepared,
            Err(error) => return failed_fetch_output(request.url, error, "deny"),
        };

        match self
            .service
            .fetch_prepared_readable_text(
                &prepared,
                self.config.max_output_bytes,
                self.config.permission_profile,
            )
            .await
        {
            Ok(output) => fetch_output_from_service(prepared.url().to_string(), output),
            Err(error) => failed_fetch_output(prepared.url().to_string(), error, "deny"),
        }
    }
}

fn fetch_output_from_service(url: String, output: WebFetchServiceOutput) -> WorkflowFetchOutput {
    let status = if output.status == ToolStatus::Success {
        WorkflowFetchStatus::Fetched
    } else {
        WorkflowFetchStatus::Failed
    };
    let error = if status == WorkflowFetchStatus::Failed {
        Some(output.content.clone())
    } else {
        None
    };

    WorkflowFetchOutput {
        url,
        final_url: string_meta(&output.meta, "final_url"),
        status,
        content: (status == WorkflowFetchStatus::Fetched).then_some(output.content),
        http_status: output
            .meta
            .get("http_status")
            .and_then(|value| value.as_u64())
            .and_then(|value| u16::try_from(value).ok()),
        content_type: string_meta(&output.meta, "content_type"),
        body_bytes: output
            .meta
            .get("body_bytes")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok()),
        body_truncated: output
            .meta
            .get("body_truncated")
            .and_then(|value| value.as_bool()),
        output_truncated: output
            .meta
            .get("output_truncated")
            .and_then(|value| value.as_bool()),
        policy_decision: Some("allow".to_string()),
        error,
    }
}

fn failed_fetch_output(url: String, error: String, policy_decision: &str) -> WorkflowFetchOutput {
    WorkflowFetchOutput {
        url,
        final_url: None,
        status: WorkflowFetchStatus::Failed,
        content: None,
        http_status: None,
        content_type: None,
        body_bytes: None,
        body_truncated: None,
        output_truncated: None,
        policy_decision: Some(policy_decision.to_string()),
        error: Some(error),
    }
}

fn string_meta(meta: &serde_json::Value, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::http::header::CONTENT_TYPE;
    use axum::{routing::get, Router};

    use crate::config::AgentConfig;
    use crate::policy::PolicyMode;
    use crate::tools::web::web_search::{SearchProvider, SearchResult};

    use super::{
        SearchProviderWorkflowSearchAdapter, WebFetchWorkflowFetchAdapter, WorkflowFetchRequest,
        WorkflowFetchStatus, WorkflowSearchRequest, WorkflowSourceError, WorkflowSourceFetch,
        WorkflowSourceSearch,
    };

    #[derive(Debug)]
    struct StubSearchProvider {
        provider_name: &'static str,
        results: Vec<SearchResult>,
        error: Option<String>,
        calls: Mutex<Vec<(String, usize)>>,
    }

    #[async_trait]
    impl SearchProvider for StubSearchProvider {
        async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
            self.calls.lock().unwrap().push((query.to_string(), count));
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(self.results.clone())
        }

        fn provider_name(&self) -> &'static str {
            self.provider_name
        }
    }

    #[tokio::test]
    async fn search_adapter_returns_structured_results() {
        let provider = Arc::new(StubSearchProvider {
            provider_name: "stub",
            results: vec![SearchResult {
                title: "Rust docs".to_string(),
                url: "https://doc.rust-lang.org/".to_string(),
                snippet: "The Rust documentation".to_string(),
            }],
            error: None,
            calls: Mutex::new(Vec::new()),
        });
        let adapter = SearchProviderWorkflowSearchAdapter::new(provider.clone());

        let response = adapter
            .search(WorkflowSearchRequest {
                query: "  rust docs  ".to_string(),
                max_results: 3,
            })
            .await
            .unwrap();

        assert_eq!(response.query, "rust docs");
        assert_eq!(response.provider, "stub");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title, "Rust docs");
        assert_eq!(response.results[0].url, "https://doc.rust-lang.org/");
        assert_eq!(response.results[0].snippet, "The Rust documentation");
        assert_eq!(response.results[0].rank, 1);
        assert_eq!(response.results[0].provider, "stub");
        assert_eq!(
            provider.calls.lock().unwrap().as_slice(),
            &[("rust docs".to_string(), 3)]
        );
    }

    #[tokio::test]
    async fn search_adapter_returns_empty_results_as_success() {
        let provider = Arc::new(StubSearchProvider {
            provider_name: "stub",
            results: Vec::new(),
            error: None,
            calls: Mutex::new(Vec::new()),
        });
        let adapter = SearchProviderWorkflowSearchAdapter::new(provider);

        let response = adapter
            .search(WorkflowSearchRequest {
                query: "missing".to_string(),
                max_results: 5,
            })
            .await
            .unwrap();

        assert_eq!(response.query, "missing");
        assert_eq!(response.provider, "stub");
        assert!(response.results.is_empty());
    }

    #[tokio::test]
    async fn search_adapter_rejects_blank_query_before_provider_call() {
        let provider = Arc::new(StubSearchProvider {
            provider_name: "stub",
            results: Vec::new(),
            error: None,
            calls: Mutex::new(Vec::new()),
        });
        let adapter = SearchProviderWorkflowSearchAdapter::new(provider.clone());

        let error = adapter
            .search(WorkflowSearchRequest {
                query: "   ".to_string(),
                max_results: 5,
            })
            .await
            .unwrap_err();

        assert_eq!(error, WorkflowSourceError::InvalidQuery);
        assert!(provider.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_adapter_surfaces_provider_errors() {
        let provider = Arc::new(StubSearchProvider {
            provider_name: "stub",
            results: Vec::new(),
            error: Some("provider failed".to_string()),
            calls: Mutex::new(Vec::new()),
        });
        let adapter = SearchProviderWorkflowSearchAdapter::new(provider);

        let error = adapter
            .search(WorkflowSearchRequest {
                query: "docs".to_string(),
                max_results: 5,
            })
            .await
            .unwrap_err();

        assert_eq!(
            error,
            WorkflowSourceError::Provider("provider failed".to_string())
        );
    }

    #[tokio::test]
    async fn fetch_adapter_fetches_readable_text_when_policy_is_off() {
        let base_url = spawn_server(Router::new().route(
            "/html",
            get(|| async {
                (
                    [(CONTENT_TYPE, "text/html; charset=utf-8")],
                    "<html><body><h1>Hello</h1><p>Workflow source</p></body></html>",
                )
            }),
        ))
        .await;
        let adapter = WebFetchWorkflowFetchAdapter::new(config_with_policy(PolicyMode::Off));

        let output = adapter
            .fetch(WorkflowFetchRequest {
                url: format!("{base_url}/html"),
                timeout_secs: Some(5),
            })
            .await;

        assert_eq!(output.status, WorkflowFetchStatus::Fetched);
        assert!(output.content.unwrap().contains("# Hello"));
        assert_eq!(output.http_status, Some(200));
        assert_eq!(
            output.content_type.as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(output.policy_decision, Some("allow".to_string()));
        assert!(output.error.is_none());
    }

    #[tokio::test]
    async fn fetch_adapter_blocks_network_when_policy_requires_approval() {
        for policy_mode in [PolicyMode::Advisory, PolicyMode::Enforced] {
            let adapter = WebFetchWorkflowFetchAdapter::new(config_with_policy(policy_mode));

            let output = adapter
                .fetch(WorkflowFetchRequest {
                    url: "https://example.com/docs".to_string(),
                    timeout_secs: None,
                })
                .await;

            assert_eq!(output.status, WorkflowFetchStatus::ApprovalBlocked);
            assert_eq!(output.url, "https://example.com/docs");
            assert_eq!(output.policy_decision, Some("review_required".to_string()));
            assert_eq!(
                output.error,
                Some("network fetch requires approval".to_string())
            );
            assert!(output.content.is_none());
        }
    }

    #[tokio::test]
    async fn fetch_adapter_approval_block_does_not_charge_rate_limiter() {
        let adapter = WebFetchWorkflowFetchAdapter::new(config_with_policy(PolicyMode::Enforced));

        for _ in 0..20 {
            let output = adapter
                .fetch(WorkflowFetchRequest {
                    url: "https://approval-blocked.example.com/docs".to_string(),
                    timeout_secs: None,
                })
                .await;

            assert_eq!(output.status, WorkflowFetchStatus::ApprovalBlocked);
            assert_eq!(output.policy_decision, Some("review_required".to_string()));
        }
    }

    #[tokio::test]
    async fn fetch_adapter_fails_invalid_scheme_without_approval_block() {
        let adapter = WebFetchWorkflowFetchAdapter::new(config_with_policy(PolicyMode::Enforced));

        let output = adapter
            .fetch(WorkflowFetchRequest {
                url: "file:///etc/hosts".to_string(),
                timeout_secs: None,
            })
            .await;

        assert_eq!(output.status, WorkflowFetchStatus::Failed);
        assert_eq!(output.policy_decision, Some("deny".to_string()));
        assert!(output.error.unwrap().contains("http or https"));
        assert!(output.content.is_none());
    }

    fn config_with_policy(policy_mode: PolicyMode) -> AgentConfig {
        AgentConfig {
            policy_mode,
            ..AgentConfig::default()
        }
    }

    async fn spawn_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }
}
