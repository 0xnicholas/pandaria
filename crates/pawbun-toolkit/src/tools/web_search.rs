use std::borrow::Cow;

use async_trait::async_trait;
use serde_json::json;

use crate::{AsyncTool, Tool, ToolError, ToolParameter, ToolResult};
use crate::tools::url_utils;

/// 搜索引擎查询工具。
///
/// 通过配置 `endpoint` 和 `api_key` 接入任意兼容的搜索 API。
/// 输入为 JSON 字符串，包含 `query` 字段。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, WebSearchTool};
///
/// let tool = WebSearchTool::new("https://api.search.example/v1");
/// assert_eq!(tool.name(), "web_search");
/// ```
pub struct WebSearchTool {
    /// 搜索 API 端点地址。
    pub endpoint: String,
    /// API 密钥（若搜索服务需要认证）。
    pub api_key: Option<String>,
    /// 最大返回结果数量。
    pub max_results: usize,
    client: reqwest::Client,
    #[cfg(test)]
    skip_url_validation: bool,
}

impl std::fmt::Debug for WebSearchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("WebSearchTool");
        d.field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("max_results", &self.max_results);
        #[cfg(test)]
        d.field("skip_url_validation", &self.skip_url_validation);
        d.finish_non_exhaustive()
    }
}

impl WebSearchTool {
    /// 创建一个新的 `WebSearchTool`。
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: None,
            max_results: 5,
            client: url_utils::build_safe_client()
                .expect("reqwest client build should not fail with default config"),
            #[cfg(test)]
            skip_url_validation: false,
        }
    }

    /// 设置 API 密钥。
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// 设置最大返回结果数量。
    pub fn with_max_results(mut self, n: usize) -> Self {
        self.max_results = n;
        self
    }

    #[cfg(test)]
    fn with_client_for_test(endpoint: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: None,
            max_results: 5,
            client,
            skip_url_validation: true,
        }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Input should be JSON with a 'query' field."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "query".into(),
                description: "Search query string".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "max_results".into(),
                description: "Maximum number of results to return".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        Err(ToolError::execution_failed(
            "WebSearchTool requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for WebSearchTool {
    async fn execute_async(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let query = parsed
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'query' field"))?;

        let max_results = parsed
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(self.max_results);

        #[cfg(not(test))]
        url_utils::validate_url(&self.endpoint).map_err(ToolError::invalid_input)?;
        #[cfg(test)]
        if !self.skip_url_validation {
            url_utils::validate_url(&self.endpoint).map_err(ToolError::invalid_input)?;
        }

        let mut req = self
            .client
            .get(&self.endpoint)
            .query(&[("q", query), ("limit", &max_results.to_string())]);

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("failed to read response body: {e}"))
        })?;

        Ok(ToolResult {
            success: status.is_success(),
            content: body,
            metadata: Some(json!({
                "query": query,
                "status": status.as_u16(),
                "max_results": max_results,
            })),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_search_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .and(query_param("limit", "5"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"results":["Rust Lang"]}"#),
            )
            .mount(&mock_server)
            .await;

        let tool = WebSearchTool::with_client_for_test(
            format!("{}/search", mock_server.uri()),
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        );
        let input = serde_json::json!({"query": "rust"}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Rust Lang"));
    }

    #[tokio::test]
    async fn test_search_with_api_key() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(wiremock::matchers::header(
                "Authorization",
                "Bearer secret123",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&mock_server)
            .await;

        let tool = {
            let mut t = WebSearchTool::with_client_for_test(
                format!("{}/search", mock_server.uri()),
                reqwest::Client::builder()
                .no_proxy()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap(),
            );
            t.api_key = Some("secret123".into());
            t
        };
        let input = serde_json::json!({"query": "test"}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(result.success);
    }

    #[tokio::test]
    async fn test_search_custom_max_results() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "test"))
            .and(query_param("limit", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .mount(&mock_server)
            .await;

        let tool = {
            let mut t = WebSearchTool::with_client_for_test(
                format!("{}/search", mock_server.uri()),
                reqwest::Client::builder()
                .no_proxy()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap(),
            );
            t.max_results = 3;
            t
        };
        let input = serde_json::json!({"query": "test", "max_results": 3}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_sync_execute_fails() {
        let tool = WebSearchTool::new("http://example.com/search");
        let err = tool.execute(r#"{"query": "rust"}"#).unwrap_err();
        assert!(err.to_string().contains("requires async execution"));
    }

    #[tokio::test]
    async fn test_missing_query_field() {
        let tool = WebSearchTool::with_client_for_test(
            "http://example.com/search",
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        );
        let err = tool.execute_async(r#"{}"#).await.unwrap_err();
        assert!(err.to_string().contains("missing 'query' field"));
    }
}
