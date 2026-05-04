use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;
use thiserror::Error;

use crate::types::{ToolCall, ToolDef};

static SCHEMA_CACHE: LazyLock<Mutex<HashMap<String, serde_json::Value>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct ValidationMessage {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("tool '{0}' not found")]
    ToolNotFound(String),

    #[error(
        "validation failed for tool '{tool}':\n{errors_formatted}\n\nReceived arguments:\n{received}"
    )]
    SchemaViolation {
        tool: String,
        errors: Vec<ValidationMessage>,
        received: serde_json::Value,
        errors_formatted: String,
    },
}

/// Coerce argument values to match the expected JSON Schema types.
fn coerce_value(value: &mut Value, schema: &Value) {
    let schema_type = schema.get("type").and_then(|t| t.as_str());

    match (schema_type, &*value) {
        (Some("integer"), Value::String(s)) => {
            if let Ok(n) = s.parse::<i64>() {
                *value = Value::Number(n.into());
            }
        }
        (Some("number"), Value::String(s)) => {
            if let Ok(f) = s.parse::<f64>()
                && let Some(n) = serde_json::Number::from_f64(f)
            {
                *value = Value::Number(n);
            }
        }
        (Some("boolean"), Value::String(s)) => {
            if s.eq_ignore_ascii_case("true") {
                *value = Value::Bool(true);
            } else if s.eq_ignore_ascii_case("false") {
                *value = Value::Bool(false);
            }
        }
        (Some("string"), Value::Number(n)) => {
            *value = Value::String(n.to_string());
        }
        _ => {}
    }

    // Recurse into object properties
    if let Value::Object(args) = value
        && let Value::Object(schema_obj) = schema
        && let Some(props) = schema_obj.get("properties")
        && let Value::Object(props) = props
    {
        for (key, prop_schema) in props {
            if let Some(field) = args.get_mut(key) {
                coerce_value(field, prop_schema);
            }
        }
    }

    // Recurse into array items
    if let Value::Array(items) = value
        && let Value::Object(schema_obj) = schema
        && let Some(item_schema) = schema_obj.get("items")
    {
        for item in items.iter_mut() {
            coerce_value(item, item_schema);
        }
    }

    // Handle allOf / anyOf / oneOf unions: try coercion against each member
    let union_keys = ["allOf", "anyOf", "oneOf"];
    for key in &union_keys {
        if let Some(Value::Array(members)) = schema.get(*key) {
            for member in members {
                let mut cloned = value.clone();
                coerce_value(&mut cloned, member);
                // If the coerced clone validates, accept it
                if let Ok(validator) = jsonschema::validator_for(member)
                    && validator.is_valid(&cloned)
                {
                    *value = cloned;
                    break;
                }
            }
        }
    }

    // Recurse into additionalProperties
    if let Value::Object(schema_obj) = schema
        && let Some(Value::Object(props)) = schema_obj.get("properties")
    {
        let known_keys: std::collections::HashSet<&str> =
            props.keys().map(|s| s.as_str()).collect();
        if let Value::Object(args) = value
            && let Some(addl_schema) = schema_obj.get("additionalProperties")
        {
            for (key, field) in args.iter_mut() {
                if !known_keys.contains(key.as_str()) {
                    coerce_value(field, addl_schema);
                }
            }
        }
    }
}

/// Validate tool call arguments with best-effort coercion, then JSON Schema validation.
/// Uses a lazy cache to avoid recompiling schemas for the same tool name.
pub fn validate_tool_arguments(
    tool: &ToolDef,
    tool_call: &ToolCall,
) -> Result<serde_json::Value, ValidationError> {
    let mut args = tool_call.arguments.clone();
    coerce_value(&mut args, &tool.parameters);

    // Check cache and compile if needed, holding lock across both operations
    // to avoid TOCTOU race where two threads compile the same schema.
    let compiled = {
        let mut cache = SCHEMA_CACHE.lock().expect("schema cache lock poisoned");
        if let Some(cached) = cache.get(&tool.name) {
            if cached == &tool.parameters {
                // Schema unchanged — compile from cached reference
                jsonschema::validator_for(cached).map_err(|e| {
                    ValidationError::schema_violation(
                        tool.name.clone(),
                        vec![ValidationMessage {
                            path: String::new(),
                            message: format!("schema compilation error: {e}"),
                        }],
                        args.clone(),
                    )
                })?
            } else {
                // Schema changed — recompile and update cache
                let compiled = jsonschema::validator_for(&tool.parameters).map_err(|e| {
                    ValidationError::schema_violation(
                        tool.name.clone(),
                        vec![ValidationMessage {
                            path: String::new(),
                            message: format!("schema compilation error: {e}"),
                        }],
                        args.clone(),
                    )
                })?;
                cache.insert(tool.name.clone(), tool.parameters.clone());
                compiled
            }
        } else {
            // Not in cache — compile and store
            let compiled = jsonschema::validator_for(&tool.parameters).map_err(|e| {
                ValidationError::schema_violation(
                    tool.name.clone(),
                    vec![ValidationMessage {
                        path: String::new(),
                        message: format!("schema compilation error: {e}"),
                    }],
                    args.clone(),
                )
            })?;
            cache.insert(tool.name.clone(), tool.parameters.clone());
            compiled
        }
    };

    if compiled.is_valid(&args) {
        return Ok(args);
    }

    let errors = ValidationError::collect_errors(&compiled, &args);
    Err(ValidationError::schema_violation(
        tool.name.clone(),
        errors,
        args,
    ))
}

impl ValidationError {
    fn collect_errors(compiled: &jsonschema::Validator, args: &Value) -> Vec<ValidationMessage> {
        compiled
            .iter_errors(args)
            .map(|e| {
                let path = e.instance_path.to_string();
                ValidationMessage {
                    path: if path.is_empty() || path == "/" {
                        "root".to_string()
                    } else {
                        path.trim_start_matches('/').to_string()
                    },
                    message: e.to_string(),
                }
            })
            .collect()
    }

    pub fn schema_violation(
        tool: String,
        errors: Vec<ValidationMessage>,
        received: serde_json::Value,
    ) -> Self {
        let errors_formatted = errors
            .iter()
            .map(|e| format!("  - {}: {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("\n");
        Self::SchemaViolation {
            tool,
            errors,
            received,
            errors_formatted,
        }
    }
}

/// Find tool by name and validate its arguments.
pub fn validate_tool_call(
    tools: &[ToolDef],
    tool_call: &ToolCall,
) -> Result<serde_json::Value, ValidationError> {
    let tool = tools
        .iter()
        .find(|t| t.name == tool_call.name)
        .ok_or_else(|| ValidationError::ToolNotFound(tool_call.name.clone()))?;
    validate_tool_arguments(tool, tool_call)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, schema: Value) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: "test".to_string(),
            parameters: schema,
        }
    }

    #[test]
    fn test_valid_arguments_pass() {
        let tool = make_tool(
            "test",
            serde_json::json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]}),
        );
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({"count": 42}),
            thought_signature: None,
        };
        let result = validate_tool_arguments(&tool, &tc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_coerce_string_to_number() {
        let tool = make_tool(
            "test",
            serde_json::json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]}),
        );
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({"count": "42"}),
            thought_signature: None,
        };
        let result = validate_tool_arguments(&tool, &tc);
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["count"], 42);
    }

    #[test]
    fn test_coerce_string_to_bool() {
        let tool = make_tool(
            "test",
            serde_json::json!({"type": "object", "properties": {"flag": {"type": "boolean"}}, "required": ["flag"]}),
        );
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({"flag": "true"}),
            thought_signature: None,
        };
        let result = validate_tool_arguments(&tool, &tc);
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["flag"], true);
    }

    #[test]
    fn test_missing_required_field() {
        let tool = make_tool(
            "test",
            serde_json::json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]}),
        );
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };
        let result = validate_tool_arguments(&tool, &tc);
        assert!(matches!(
            result,
            Err(ValidationError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn test_tool_not_found() {
        let tools = vec![make_tool(
            "read",
            serde_json::json!({"type": "object", "properties": {}}),
        )];
        let tc = ToolCall {
            id: "1".into(),
            name: "nonexistent".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };
        assert!(matches!(
            validate_tool_call(&tools, &tc),
            Err(ValidationError::ToolNotFound(_))
        ));
    }

    #[test]
    fn test_coerce_number_to_string() {
        let tool = make_tool(
            "test",
            serde_json::json!({"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"]}),
        );
        let tc = ToolCall {
            id: "1".into(),
            name: "test".into(),
            arguments: serde_json::json!({"label": 42}),
            thought_signature: None,
        };
        let result = validate_tool_arguments(&tool, &tc);
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["label"], "42");
    }
}
