use pawbun_toolkit::{
    pawbun_tool, Tool, ToolError, ToolExecutor, ToolKit, ToolParameter, ToolRegistry, ToolResult,
};
use std::borrow::Cow;

// 测试宏自动生成 name / description / parameters
#[derive(Debug)]
struct EchoTool;

#[pawbun_tool(name = "echo", description = "Echoes the input back")]
impl Tool for EchoTool {
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            content: input.into(),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

// 测试宏保留用户自定义的 name / description
#[derive(Debug)]
struct CustomNameTool;

#[pawbun_tool(name = "custom", description = "A custom tool")]
impl Tool for CustomNameTool {
    fn name(&self) -> &str {
        "overridden_name"
    }

    fn description(&self) -> &str {
        "Overridden description"
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            content: format!("custom: {}", input),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

// 测试宏保留用户自定义的 parameters
#[derive(Debug)]
struct ParamTool;

#[pawbun_tool(name = "param_tool", description = "A tool with params")]
impl Tool for ParamTool {
    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![ToolParameter {
            name: "input".into(),
            description: "Any string".into(),
            required: true,
            schema: serde_json::json!({"type": "string"}),
        }])
    }

    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            content: format!("got: {}", input),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

#[test]
fn test_macro_generates_name_and_description() {
    let tool = EchoTool;
    assert_eq!(tool.name(), "echo");
    assert_eq!(tool.description(), "Echoes the input back");
}

#[test]
fn test_macro_generates_empty_parameters() {
    let tool = EchoTool;
    let params = tool.parameters();
    assert!(params.is_empty());
}

#[test]
fn test_macro_respects_user_parameters() {
    let tool = ParamTool;
    let params = tool.parameters();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "input");
}

#[test]
fn test_macro_tool_integration_with_toolkit() {
    let mut kit = ToolKit::new();
    kit.register(Box::new(EchoTool));

    let result = kit.execute("echo", "hello").unwrap();
    assert!(result.success);
    assert_eq!(result.content, "hello");

    let desc = kit.descriptions();
    assert!(desc.contains("echo"));
    assert!(desc.contains("Echoes the input back"));
}

#[test]
fn test_macro_respects_user_name_and_description() {
    let tool = CustomNameTool;
    // User-defined name/desc should override macro-generated ones.
    assert_eq!(tool.name(), "overridden_name");
    assert_eq!(tool.description(), "Overridden description");
}

#[test]
fn test_macro_custom_name_tool_execution() {
    let mut kit = ToolKit::new();
    kit.register(Box::new(CustomNameTool));

    let result = kit.execute("overridden_name", "world").unwrap();
    assert!(result.success);
    assert_eq!(result.content, "custom: world");
}
