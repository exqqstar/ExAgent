use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ToolResult, ToolStatus};

const DEFAULT_COUNT: usize = 5;
const MAX_COUNT: usize = 10;
const REQUEST_TIMEOUT_SECS: u64 = 15;
const DEFAULT_BRAVE_BASE_URL: &str = "https://api.search.brave.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchResult>, String>;
    fn provider_name(&self) -> &'static str {
        "brave"
    }
}

#[derive(Clone)]
pub struct BraveSearchProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl BraveSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, DEFAULT_BRAVE_BASE_URL.to_string())
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .build()
                .expect("build web_search reqwest client"),
        }
    }
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
        let response = self
            .client
            .get(format!("{}/res/v1/web/search", self.base_url))
            .header("X-Subscription-Token", &self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await
            .map_err(|err| format!("web search request failed: {err}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("web search API key rejected".to_string());
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err("web search rate limited, retry later".to_string());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "web search provider returned HTTP {}: {}",
                status.as_u16(),
                truncate(&body, 200)
            ));
        }

        let body = response
            .json::<BraveSearchResponse>()
            .await
            .map_err(|err| format!("web search response was not valid JSON: {err}"))?;
        Ok(search_results_from_brave(body))
    }
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

fn search_results_from_brave(response: BraveSearchResponse) -> Vec<SearchResult> {
    response
        .web
        .map(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .filter(|result| !result.url.trim().is_empty())
        .map(|result| SearchResult {
            title: strip_html_tags(&result.title),
            url: result.url.trim().to_string(),
            snippet: strip_html_tags(&result.description),
        })
        .collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebSearchArgs {
    pub query: String,
    pub count: Option<usize>,
}

pub struct WebSearchTool {
    provider: Arc<dyn SearchProvider>,
}

impl WebSearchTool {
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl ToolHandler for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "web_search",
            "Search the web; returns result titles, URLs, and snippets",
            serde_json::to_value(schemars::schema_for!(WebSearchArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, _ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<WebSearchArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return web_search_error(call.id, call.name, err.to_string()),
        };

        match execute_search(args, self.provider.as_ref()).await {
            Ok(outcome) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: outcome.content,
                meta: Some(outcome.meta),
                parts: Vec::new(),
            }),
            Err(err) => web_search_error(call.id, call.name, err),
        }
    }
}

#[derive(Debug)]
struct WebSearchOutcome {
    content: String,
    meta: Value,
}

async fn execute_search(
    args: WebSearchArgs,
    provider: &dyn SearchProvider,
) -> Result<WebSearchOutcome, String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    let count = args.count.unwrap_or(DEFAULT_COUNT).clamp(1, MAX_COUNT);
    let results = provider.search(query, count).await?;
    let content = format_results(query, &results);
    Ok(WebSearchOutcome {
        content,
        meta: json!({
            "query": query,
            "provider": provider.provider_name(),
            "result_count": results.len(),
        }),
    })
}

fn format_results(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No results for: {query}");
    }

    results
        .iter()
        .enumerate()
        .map(|(index, result)| {
            format!(
                "{}. {}\n   {}\n   {}",
                index + 1,
                result.title,
                result.url,
                result.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn strip_html_tags(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

fn web_search_error(tool_call_id: String, tool_name: String, content: String) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content,
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use axum::http::StatusCode;
    use axum::{routing::get, Router};
    use serde_json::json;
    use tokio::net::TcpListener;

    struct StubSearchProvider {
        results: Vec<SearchResult>,
        error: Option<String>,
        seen_counts: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl SearchProvider for StubSearchProvider {
        async fn search(&self, _query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
            self.seen_counts.lock().unwrap().push(count);
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(self.results.clone())
        }
    }

    #[test]
    fn brave_parser_extracts_web_results_and_strips_html() {
        let response = serde_json::from_value::<BraveSearchResponse>(json!({
            "web": {
                "results": [
                    {
                        "title": "<b>Rust</b> docs",
                        "url": "https://doc.rust-lang.org/",
                        "description": "The <em>Rust</em> documentation"
                    }
                ]
            }
        }))
        .unwrap();

        assert_eq!(
            search_results_from_brave(response),
            vec![SearchResult {
                title: "Rust docs".to_string(),
                url: "https://doc.rust-lang.org/".to_string(),
                snippet: "The Rust documentation".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn web_search_formats_results_and_clamps_count() {
        let provider = StubSearchProvider {
            results: vec![SearchResult {
                title: "Example".to_string(),
                url: "https://example.com".to_string(),
                snippet: "A result".to_string(),
            }],
            error: None,
            seen_counts: Mutex::new(Vec::new()),
        };
        let outcome = execute_search(
            WebSearchArgs {
                query: "  docs  ".to_string(),
                count: Some(99),
            },
            &provider,
        )
        .await
        .unwrap();

        assert_eq!(
            provider.seen_counts.lock().unwrap().as_slice(),
            &[MAX_COUNT]
        );
        assert_eq!(
            outcome.content,
            "1. Example\n   https://example.com\n   A result"
        );
        assert_eq!(outcome.meta["query"], "docs");
        assert_eq!(outcome.meta["result_count"], 1);
    }

    #[tokio::test]
    async fn web_search_reports_empty_results_as_success() {
        let provider = StubSearchProvider {
            results: vec![],
            error: None,
            seen_counts: Mutex::new(Vec::new()),
        };
        let outcome = execute_search(
            WebSearchArgs {
                query: "missing".to_string(),
                count: None,
            },
            &provider,
        )
        .await
        .unwrap();

        assert_eq!(outcome.content, "No results for: missing");
        assert_eq!(
            provider.seen_counts.lock().unwrap().as_slice(),
            &[DEFAULT_COUNT]
        );
    }

    #[tokio::test]
    async fn web_search_passthroughs_provider_errors() {
        let provider = StubSearchProvider {
            results: vec![],
            error: Some("provider failed".to_string()),
            seen_counts: Mutex::new(Vec::new()),
        };

        let err = execute_search(
            WebSearchArgs {
                query: "docs".to_string(),
                count: None,
            },
            &provider,
        )
        .await
        .unwrap_err();

        assert_eq!(err, "provider failed");
    }

    #[tokio::test]
    async fn brave_provider_maps_error_statuses() {
        let unauthorized = spawn_search_server(StatusCode::UNAUTHORIZED, "bad key").await;
        let rate_limited = spawn_search_server(StatusCode::TOO_MANY_REQUESTS, "slow down").await;
        let server_error = spawn_search_server(StatusCode::INTERNAL_SERVER_ERROR, "broken").await;

        let err = BraveSearchProvider::with_base_url("key".to_string(), unauthorized)
            .search("docs", 3)
            .await
            .unwrap_err();
        assert_eq!(err, "web search API key rejected");

        let err = BraveSearchProvider::with_base_url("key".to_string(), rate_limited)
            .search("docs", 3)
            .await
            .unwrap_err();
        assert_eq!(err, "web search rate limited, retry later");

        let err = BraveSearchProvider::with_base_url("key".to_string(), server_error)
            .search("docs", 3)
            .await
            .unwrap_err();
        assert!(err.contains("HTTP 500"));
        assert!(err.contains("broken"));
    }

    #[tokio::test]
    async fn brave_provider_parses_success_response() {
        let base_url = spawn_search_server(
            StatusCode::OK,
            r#"{"web":{"results":[{"title":"One","url":"https://one.example","description":"First"}]}}"#,
        )
        .await;

        let results = BraveSearchProvider::with_base_url("key".to_string(), base_url)
            .search("docs", 3)
            .await
            .unwrap();

        assert_eq!(
            results,
            vec![SearchResult {
                title: "One".to_string(),
                url: "https://one.example".to_string(),
                snippet: "First".to_string(),
            }]
        );
    }

    async fn spawn_search_server(status: StatusCode, body: &'static str) -> String {
        let app = Router::new().route(
            "/res/v1/web/search",
            get(move || async move { (status, body) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }
}
