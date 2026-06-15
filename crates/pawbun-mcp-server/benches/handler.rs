use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pawbun_toolkit::mcp::*;
use pawbun_toolkit::{Tool, ToolError, ToolParameter, ToolResult};
use pawbun_mcp_server::handler::RequestHandler;
use serde_json::json;
use std::borrow::Cow;

#[derive(Debug)]
struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "Echoes input back." }
    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![
            ToolParameter {
                name: "message".into(),
                description: "Message to echo".into(),
                required: true,
                schema: json!({"type": "string"}),
            },
        ])
    }
    fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            success: true,
            content: input.into(),
            metadata: None,
            elapsed_ms: None,
        })
    }
}

fn make_handler() -> RequestHandler {
    let mut toolkit = pawbun_toolkit::ToolKit::new();
    toolkit.register(Box::new(EchoTool));

    RequestHandler::new(
        toolkit,
        ServerInfo { name: "bench".into(), version: "0.1.0".into() },
        json!({"tools": {}}),
        "2024-11-05".into(),
        None,
    )
}

fn benchmark_initialize(c: &mut Criterion) {
    c.bench_function("handler_initialize", |b| {
        b.iter(|| {
            let mut handler = make_handler();
            let init = JsonRpcRequest::new(
                1i64,
                "initialize",
                Some(json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "bench", "version": "1.0"}})),
            );
            let resp = handler.handle(black_box(init));
            black_box(resp);
            let notif = JsonRpcRequest::notification("notifications/initialized", None);
            handler.handle(black_box(notif));
        })
    });
}

fn benchmark_tools_list(c: &mut Criterion) {
    let mut handler = make_handler();
    let init = JsonRpcRequest::new(
        1i64,
        "initialize",
        Some(json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "bench", "version": "1.0"}})),
    );
    handler.handle(init);
    let notif = JsonRpcRequest::notification("notifications/initialized", None);
    handler.handle(notif);

    let req = JsonRpcRequest::new(2i64, "tools/list", None);
    c.bench_function("handler_tools_list/1", |b| {
        b.iter(|| {
            let resp = handler.handle(black_box(req.clone()));
            black_box(resp);
        })
    });
}

fn benchmark_tools_call(c: &mut Criterion) {
    let mut handler = make_handler();
    let init = JsonRpcRequest::new(
        1i64,
        "initialize",
        Some(json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "bench", "version": "1.0"}})),
    );
    handler.handle(init);
    let notif = JsonRpcRequest::notification("notifications/initialized", None);
    handler.handle(notif);

    let req = JsonRpcRequest::new(
        2i64,
        "tools/call",
        Some(json!({"name": "echo", "arguments": {"message": "hello"}})),
    );
    c.bench_function("handler_tools_call", |b| {
        b.iter(|| {
            let resp = handler.handle(black_box(req.clone()));
            black_box(resp);
        })
    });
}

criterion_group!(benches, benchmark_initialize, benchmark_tools_list, benchmark_tools_call);
criterion_main!(benches);
