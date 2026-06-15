use std::borrow::Cow;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 使用 JSONPath 查询 JSON 数据的工具。
///
/// 输入为 JSON 字符串，包含 `data`（JSON 数据）和 `query`（JSONPath 表达式）字段。
/// 返回匹配 JSONPath 表达式的结果数组。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, JsonQueryTool};
///
/// let tool = JsonQueryTool;
/// assert_eq!(tool.name(), "json_query");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonQueryTool;

impl Tool for JsonQueryTool {
    fn name(&self) -> &str {
        "json_query"
    }

    fn description(&self) -> &str {
        "Query JSON data using JSONPath. Input should be JSON with 'data' and 'query' fields."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "data".into(),
                description: "JSON data to query (object or array)".into(),
                required: true,
                schema: json!({"type": ["object", "array"]}),
            },
            ToolParameter {
                name: "query".into(),
                description: "JSONPath expression (e.g. '$.store.book[0].title')".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
        ])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let data = parsed
            .get("data")
            .ok_or_else(|| ToolError::invalid_input("missing 'data' field"))?;

        let query = parsed
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'query' field"))?;

        use jsonpath_rust::JsonPath;

        let results: Vec<serde_json::Value> = data
            .query(query)
            .map_err(|e| ToolError::invalid_input(format!("invalid JSONPath: {e}")))?
            .into_iter()
            .cloned()
            .collect();

        Ok(ToolResult {
            success: true,
            content: serde_json::to_string_pretty(&results)
                .map_err(|e| ToolError::serialization(e.to_string()))?,
            metadata: Some(json!({"query": query, "match_count": results.len()})),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonpath_object_field() {
        let tool = JsonQueryTool;
        let input = r#"{"data": {"name": "Alice", "age": 30}, "query": "$.name"}"#;
        let result = tool.execute(input).unwrap();

        assert!(result.success);
        let items: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(items, vec!["Alice"]);
    }

    #[test]
    fn test_jsonpath_array_index() {
        let tool = JsonQueryTool;
        let input = r#"{"data": [10, 20, 30], "query": "$[1]"}"#;
        let result = tool.execute(input).unwrap();

        let items: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(items, vec![20]);
    }

    #[test]
    fn test_jsonpath_wildcard() {
        let tool = JsonQueryTool;
        let input = r#"{"data": {"a": 1, "b": 2, "c": 3}, "query": "$.*"}"#;
        let result = tool.execute(input).unwrap();

        let items: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(items, vec![1, 2, 3]);
    }

    #[test]
    fn test_invalid_jsonpath() {
        let tool = JsonQueryTool;
        let input = r#"{"data": {}, "query": "[[invalid"}"#;
        let result = tool.execute(input);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid JSONPath"));
    }

    #[test]
    fn test_missing_fields() {
        let tool = JsonQueryTool;
        assert!(tool
            .execute(r#"{}"#)
            .unwrap_err()
            .to_string()
            .contains("missing 'data' field"));
        assert!(tool
            .execute(r#"{"data": {}}"#)
            .unwrap_err()
            .to_string()
            .contains("missing 'query' field"));
    }
}
