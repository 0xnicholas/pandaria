use llm_client::{ToolCall, ToolDef, ValidationError, validate_tool_arguments, validate_tool_call};
use serde_json::Value;

fn make_tool(name: &str, schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: "test tool".to_string(),
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
    assert_eq!(result.unwrap()["count"], 42);
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
fn test_error_message_format() {
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
    let err = validate_tool_arguments(&tool, &tc).unwrap_err();
    match err {
        ValidationError::SchemaViolation {
            tool,
            errors,
            received,
            errors_formatted,
        } => {
            assert_eq!(tool, "test");
            assert!(!errors.is_empty());
            assert!(errors.iter().any(|e| !e.path.is_empty()));
            assert!(errors.iter().any(|e| !e.message.is_empty()));
            assert_eq!(received, serde_json::json!({}));
            assert!(!errors_formatted.is_empty());
            assert!(errors_formatted.contains("root"));
        }
        _ => panic!("expected SchemaViolation"),
    }
}

#[test]
fn test_schema_caching() {
    let tool = make_tool(
        "cached_tool",
        serde_json::json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]}),
    );
    let tc = ToolCall {
        id: "1".into(),
        name: "cached_tool".into(),
        arguments: serde_json::json!({"count": "42"}),
        thought_signature: None,
    };

    // First call should compile and cache the schema
    let result1 = validate_tool_arguments(&tool, &tc);
    assert!(result1.is_ok());

    // Second call should use cached schema and still succeed
    let result2 = validate_tool_arguments(&tool, &tc);
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap()["count"], 42);
}

#[test]
fn test_wrong_type_uncoercible() {
    let tool = make_tool(
        "test",
        serde_json::json!({"type": "object", "properties": {"count": {"type": "integer"}}, "required": ["count"]}),
    );
    let tc = ToolCall {
        id: "1".into(),
        name: "test".into(),
        arguments: serde_json::json!({"count": "abc"}),
        thought_signature: None,
    };
    let result = validate_tool_arguments(&tool, &tc);
    assert!(matches!(
        result,
        Err(ValidationError::SchemaViolation { .. })
    ));
}
