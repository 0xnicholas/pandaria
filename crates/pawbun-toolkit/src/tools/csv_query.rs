use std::borrow::Cow;
use std::io::Cursor;

use serde_json::json;

use crate::{Tool, ToolError, ToolParameter, ToolResult};

/// 解析和查询 CSV 数据的工具。
///
/// 输入为 JSON 字符串，包含：
/// - `csv`（字符串）：CSV 内容
/// - `has_header`（布尔，可选）：第一行是否为表头，默认 `true`
/// - `columns`（字符串数组，可选）：仅返回指定列
/// - `limit`（整数，可选）：最大返回行数
///
/// 输出为 JSON 数组。若 `has_header=true`，每行是一个对象；否则每行是一个数组。
///
/// # Example
/// ```
/// use pawbun_toolkit::{Tool, CsvQueryTool};
///
/// let tool = CsvQueryTool;
/// assert_eq!(tool.name(), "csv_query");
/// ```
#[derive(Debug, Default, Clone, Copy)]
pub struct CsvQueryTool;

impl Tool for CsvQueryTool {
    fn name(&self) -> &str {
        "csv_query"
    }

    fn description(&self) -> &str {
        "Parse and query CSV data. Input should be JSON with 'csv' field and optional 'has_header', 'columns', 'limit'."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "csv".into(),
                description: "CSV content as a string".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "has_header".into(),
                description: "Whether the first row is a header row".into(),
                required: false,
                schema: json!({"type": "boolean"}),
            },
            ToolParameter {
                name: "columns".into(),
                description: "List of column names to include (only when has_header=true)".into(),
                required: false,
                schema: json!({"type": "array", "items": {"type": "string"}}),
            },
            ToolParameter {
                name: "limit".into(),
                description: "Maximum number of data rows to return".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        let parsed: serde_json::Value = crate::json_utils::parse(input)?;

        let csv_text = parsed
            .get("csv")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("missing 'csv' field"))?;

        let has_header = parsed
            .get("has_header")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let columns_filter: Option<Vec<String>> =
            parsed.get("columns").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let limit = parsed
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(has_header)
            .from_reader(Cursor::new(csv_text));

        let headers: Option<Vec<String>> = if has_header {
            reader
                .headers()
                .map_err(|e| ToolError::execution_failed(format!("CSV header error: {e}")))?
                .iter()
                .map(String::from)
                .collect::<Vec<_>>()
                .into()
        } else {
            None
        };

        let mut rows = Vec::new();
        for (idx, record) in reader.records().enumerate() {
            if let Some(l) = limit {
                if idx >= l {
                    break;
                }
            }

            let record =
                record.map_err(|e| ToolError::execution_failed(format!("CSV parse error: {e}")))?;

            if let Some(ref hdrs) = headers {
                let mut obj = serde_json::Map::new();
                for (i, field) in record.iter().enumerate() {
                    let col_name = hdrs.get(i).map(String::as_str).unwrap_or("");
                    if let Some(ref cols) = columns_filter {
                        if !cols.contains(&col_name.to_string()) {
                            continue;
                        }
                    }
                    obj.insert(col_name.to_string(), json!(field));
                }
                rows.push(serde_json::Value::Object(obj));
            } else {
                let arr: Vec<serde_json::Value> = record.iter().map(|f| json!(f)).collect();
                rows.push(serde_json::Value::Array(arr));
            }
        }

        Ok(ToolResult {
            success: true,
            content: serde_json::to_string_pretty(&rows)
                .map_err(|e| ToolError::serialization(e.to_string()))?,
            metadata: Some(json!({"row_count": rows.len(), "has_header": has_header})),
            elapsed_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_with_header() {
        let tool = CsvQueryTool;
        let input = r#"{"csv": "name,age\nAlice,30\nBob,25"}"#;
        let result = tool.execute(input).unwrap();

        assert!(result.success);
        let rows: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Alice");
        assert_eq!(rows[1]["age"], "25");
    }

    #[test]
    fn test_csv_without_header() {
        let tool = CsvQueryTool;
        let input = r#"{"csv": "a,b\nc,d", "has_header": false}"#;
        let result = tool.execute(input).unwrap();

        let rows: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], json!(["a", "b"]));
        assert_eq!(rows[1], json!(["c", "d"]));
    }

    #[test]
    fn test_csv_column_filter() {
        let tool = CsvQueryTool;
        let input =
            r#"{"csv": "name,age,city\nAlice,30,NYC\nBob,25,LA", "columns": ["name", "city"]}"#;
        let result = tool.execute(input).unwrap();

        let rows: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "Alice");
        assert_eq!(rows[0]["city"], "NYC");
        assert!(rows[0].get("age").is_none());
    }

    #[test]
    fn test_csv_limit() {
        let tool = CsvQueryTool;
        let input = r#"{"csv": "n\n1\n2\n3", "limit": 2}"#;
        let result = tool.execute(input).unwrap();

        let rows: Vec<serde_json::Value> = serde_json::from_str(&result.content).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_missing_csv_field() {
        let tool = CsvQueryTool;
        let result = tool.execute(r#"{}"#);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing 'csv' field"));
    }
}
