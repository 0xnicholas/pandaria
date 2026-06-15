//! Bidirectional conversion between MCP input schema and ToolParameter lists.

use serde_json::Value;

use crate::ToolParameter;

/// Converts an MCP `input_schema` (JSON Schema object) into a list of [`ToolParameter`].
///
/// Supports simple object schemas with `properties` and `required` fields.
/// Properties schemas are stored as-is in [`ToolParameter::schema`].
///
/// # Example
/// ```
/// use pawbun_toolkit::mcp::schema_to_parameters;
/// use serde_json::json;
///
/// let schema = json!({
///     "type": "object",
///     "properties": {
///         "path": {"type": "string", "description": "File path"},
///         "recursive": {"type": "boolean"}
///     },
///     "required": ["path"]
/// });
///
/// let params = schema_to_parameters(&schema);
/// assert_eq!(params.len(), 2);
/// assert_eq!(params[0].name, "path");
/// assert!(params[0].required);
/// assert!(!params[1].required);
/// ```
pub fn schema_to_parameters(schema: &Value) -> Vec<ToolParameter> {
    let mut params = Vec::new();

    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return params;
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    for (name, prop_schema) in properties {
        let description = prop_schema
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        let is_required = required.contains(&name.as_str());

        params.push(ToolParameter {
            name: name.clone(),
            description,
            required: is_required,
            schema: prop_schema.clone(),
        });
    }

    params
}

/// Converts a list of [`ToolParameter`] into an MCP `input_schema` JSON Schema object.
///
/// Used by server-side `tools/list` to expose tool parameters in MCP-compliant format.
/// Assembled as `{"type": "object", "properties": {...}, "required": [...]}`.
///
/// # Example
/// ```
/// use pawbun_toolkit::mcp::parameters_to_schema;
/// use pawbun_toolkit::ToolParameter;
/// use serde_json::json;
///
/// let params = vec![
///     ToolParameter {
///         name: "path".into(),
///         description: "File path".into(),
///         required: true,
///         schema: json!({"type": "string"}),
///     },
///     ToolParameter {
///         name: "max_length".into(),
///         description: "Max chars".into(),
///         required: false,
///         schema: json!({"type": "integer"}),
///     },
/// ];
///
/// let schema = parameters_to_schema(&params);
/// assert_eq!(schema["type"], "object");
/// assert_eq!(schema["required"][0], "path");
/// assert_eq!(schema["properties"]["path"]["type"], "string");
/// ```
pub fn parameters_to_schema(params: &[ToolParameter]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();

    for p in params {
        properties.insert(p.name.clone(), p.schema.clone());
        if p.required {
            required.push(Value::String(p.name.clone()));
        }
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    if !properties.is_empty() {
        schema.insert("properties".into(), Value::Object(properties));
    }
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }

    Value::Object(schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_schema_to_parameters_empty() {
        let schema = json!({});
        let params = schema_to_parameters(&schema);
        assert!(params.is_empty());
    }

    #[test]
    fn test_schema_to_parameters_with_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path"},
                "recursive": {"type": "boolean"}
            },
            "required": ["path"]
        });
        let params = schema_to_parameters(&schema);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "path");
        assert!(params[0].required);
        assert_eq!(params[1].name, "recursive");
        assert!(!params[1].required);
    }

    #[test]
    fn test_parameters_to_schema_single_required() {
        let params = vec![ToolParameter {
            name: "path".into(),
            description: "File path".into(),
            required: true,
            schema: json!({"type": "string"}),
        }];
        let schema = parameters_to_schema(&params);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "path");
        assert_eq!(schema["properties"]["path"]["type"], "string");
    }

    #[test]
    fn test_parameters_to_schema_mixed_required_optional() {
        let params = vec![
            ToolParameter {
                name: "path".into(),
                description: "".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
            ToolParameter {
                name: "limit".into(),
                description: "".into(),
                required: false,
                schema: json!({"type": "integer"}),
            },
        ];
        let schema = parameters_to_schema(&params);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["path"]);
        assert_eq!(schema["properties"]["limit"]["type"], "integer");
    }

    #[test]
    fn test_parameters_to_schema_empty() {
        let schema = parameters_to_schema(&[]);
        assert_eq!(schema["type"], "object");
        assert!(schema.get("properties").is_none());
        assert!(schema.get("required").is_none());
    }

    #[test]
    fn test_roundtrip() {
        let params = vec![
            ToolParameter {
                name: "x".into(),
                description: "desc".into(),
                required: true,
                schema: json!({"type": "number"}),
            },
            ToolParameter {
                name: "y".into(),
                description: "opt".into(),
                required: false,
                schema: json!({"type": "boolean"}),
            },
        ];
        let schema = parameters_to_schema(&params);
        let roundtripped = schema_to_parameters(&schema);
        assert_eq!(roundtripped.len(), 2);
        assert_eq!(roundtripped[0].name, "x");
        assert!(roundtripped[0].required);
        assert_eq!(roundtripped[1].name, "y");
        assert!(!roundtripped[1].required);
    }
}
