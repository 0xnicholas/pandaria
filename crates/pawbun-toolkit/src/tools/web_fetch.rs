use std::borrow::Cow;

use async_trait::async_trait;
use serde_json::json;

use crate::{AsyncTool, Tool, ToolError, ToolParameter, ToolResult};
use crate::tools::url_utils;

/// 抓取网页内容的工具。
///
/// 输入为 JSON 字符串，包含 `url` 字段。可选 `max_length` 限制返回内容长度。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, WebFetchTool};
///
/// let tool = WebFetchTool::default();
/// assert_eq!(tool.name(), "web_fetch");
/// ```
pub struct WebFetchTool {
    /// 最大返回内容长度（字符数），超出部分截断。
    /// `None` 表示不限制。
    pub max_length: Option<usize>,
    client: reqwest::Client,
    #[cfg(test)]
    skip_url_validation: bool,
}

impl std::fmt::Debug for WebFetchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("WebFetchTool");
        d.field("max_length", &self.max_length);
        #[cfg(test)]
        d.field("skip_url_validation", &self.skip_url_validation);
        d.finish_non_exhaustive()
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self {
            max_length: None,
            client: url_utils::build_safe_client()
                .expect("reqwest client build should not fail with default config"),
            #[cfg(test)]
            skip_url_validation: false,
        }
    }
}

impl WebFetchTool {
    /// 创建一个新的 `WebFetchTool`，可指定最大内容长度。
    pub fn with_max_length(max_length: usize) -> Self {
        Self {
            max_length: Some(max_length),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn with_client_for_test(client: reqwest::Client) -> Self {
        Self {
            max_length: None,
            client,
            skip_url_validation: true,
        }
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a web page. Input should be JSON with a 'url' field."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "url".into(),
                description: "URL of the web page to fetch".into(),
                required: true,
                schema: json!({"type": "string", "format": "uri"}),
            },
            ToolParameter {
                name: "max_length".into(),
                description: "Maximum content length to return (chars)".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, _input: &str) -> Result<ToolResult, ToolError> {
        // 同步上下文下，WebFetchTool 不支持阻塞式 HTTP 请求。
        // 调用方应通过 AsyncToolExecutor 在异步上下文中使用此工具。
        Err(ToolError::execution_failed(
            "WebFetchTool requires async execution. Use execute_async instead.",
        ))
    }

    fn as_async(&self) -> Option<&dyn AsyncTool> {
        Some(self)
    }
}

#[async_trait]
impl AsyncTool for WebFetchTool {
    async fn execute_async(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let url = parsed
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'url' field"))?;

        let max_length = parsed
            .get("max_length")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .or(self.max_length);

        #[cfg(not(test))]
        url_utils::validate_url(url).map_err(ToolError::invalid_input)?;
        #[cfg(test)]
        if !self.skip_url_validation {
            url_utils::validate_url(url).map_err(ToolError::invalid_input)?;
        }

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ToolError::execution_failed(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            ToolError::execution_failed(format!("failed to read response body: {e}"))
        })?;

        let content = match max_length {
            Some(limit) if body.chars().nth(limit).is_some() => {
                body.chars().take(limit).collect::<String>() + "... [truncated]"
            }
            _ => body,
        };

        Ok(ToolResult {
            success: status.is_success(),
            content,
            metadata: Some(json!({
                "url": url,
                "status": status.as_u16(),
            })),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_fetch_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(ResponseTemplate::new(200).set_body_string("Hello, web!"))
            .mount(&mock_server)
            .await;

        let tool = WebFetchTool::with_client_for_test(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        );
        let url = format!("{}/page", mock_server.uri());
        let input = serde_json::json!({"url": url}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(result.success);
        assert_eq!(result.content, "Hello, web!");
        assert_eq!(result.metadata.as_ref().unwrap()["status"], 200);
    }

    #[tokio::test]
    async fn test_fetch_max_length() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/long"))
            .respond_with(ResponseTemplate::new(200).set_body_string("abcdefghij"))
            .mount(&mock_server)
            .await;

        let tool = {
            let mut t = WebFetchTool::with_client_for_test(
                reqwest::Client::builder()
                .no_proxy()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .unwrap(),
            );
            t.max_length = Some(5);
            t
        };
        let url = format!("{}/long", mock_server.uri());
        let input = serde_json::json!({"url": url}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(result.success);
        assert_eq!(result.content, "abcde... [truncated]");
    }

    #[tokio::test]
    async fn test_fetch_not_found() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let tool = WebFetchTool::with_client_for_test(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        );
        let url = format!("{}/missing", mock_server.uri());
        let input = serde_json::json!({"url": url}).to_string();
        let result = tool.execute_async(&input).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.metadata.as_ref().unwrap()["status"], 404);
    }

    #[test]
    fn test_sync_execute_fails() {
        let tool = WebFetchTool::default();
        let err = tool
            .execute(r#"{"url": "http://example.com"}"#)
            .unwrap_err();
        assert!(err.to_string().contains("requires async execution"));
    }

    #[tokio::test]
    async fn test_missing_url_field() {
        let tool = WebFetchTool::default();
        let err = tool.execute_async(r#"{}"#).await.unwrap_err();
        assert!(err.to_string().contains("missing 'url' field"));
    }
}
