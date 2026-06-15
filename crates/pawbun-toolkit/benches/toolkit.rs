use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pawbun_toolkit::mcp::core::schema_convert::parameters_to_schema;
use pawbun_toolkit::{
    Tool, ToolError, ToolExecutor, ToolKit, ToolParameter, ToolRegistry, ToolResult,
};
use serde_json::json;
use std::borrow::Cow;

#[derive(Debug)]
struct NoOpTool;

impl Tool for NoOpTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "A no-op tool for benchmarking."
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![])
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

fn benchmark_registry_lookup(c: &mut Criterion) {
    let mut kit = ToolKit::new();
    for i in 0..100 {
        kit.register(Box::new(NamedNoOpTool(format!("tool_{}", i))));
    }

    c.bench_function("registry_get", |b| {
        b.iter(|| {
            let _ = kit.get(black_box("tool_50"));
        })
    });
}

fn benchmark_tool_execution(c: &mut Criterion) {
    let mut kit = ToolKit::new();
    kit.register(Box::new(NoOpTool));

    c.bench_function("tool_execute_overhead", |b| {
        b.iter(|| {
            let result = kit.execute(black_box("noop"), black_box("hello")).unwrap();
            black_box(result);
        })
    });
}

fn benchmark_register(c: &mut Criterion) {
    c.bench_function("tool_register", |b| {
        let mut kit = ToolKit::new();
        let mut counter = 0usize;
        b.iter(|| {
            kit.register(Box::new(NamedNoOpTool(format!("tool_{}", counter))));
            counter += 1;
            black_box(&kit);
        })
    });
}

#[derive(Debug)]
struct NamedNoOpTool(String);

impl Tool for NamedNoOpTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "Named no-op tool"
    }

    fn parameters(&self) -> Cow<'static, [ToolParameter]> {
        Cow::Owned(vec![])
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

fn benchmark_registry_lookup_1000(c: &mut Criterion) {
    let mut kit = ToolKit::new();
    for i in 0..1000 {
        kit.register(Box::new(NamedNoOpTool(format!("tool_{}", i))));
    }

    c.bench_function("registry_lookup/1000", |b| {
        b.iter(|| {
            let _ = kit.get(black_box("tool_500"));
        })
    });
}

fn benchmark_tool_descriptions(c: &mut Criterion) {
    let mut kit = ToolKit::new();
    for i in 0..100 {
        kit.register(Box::new(NamedNoOpTool(format!("tool_{}", i))));
    }

    c.bench_function("tool_descriptions/100", |b| {
        b.iter(|| {
            let _ = black_box(kit.descriptions());
        })
    });
}

fn benchmark_schema_build(c: &mut Criterion) {
    let params = vec![
        ToolParameter {
            name: "url".into(),
            description: "URL to fetch".into(),
            required: true,
            schema: json!({"type": "string", "format": "uri"}),
        },
        ToolParameter {
            name: "max_length".into(),
            description: "Max length".into(),
            required: false,
            schema: json!({"type": "integer"}),
        },
    ];
    c.bench_function("schema_build/10_params", |b| {
        b.iter(|| {
            let schema = parameters_to_schema(black_box(&params));
            black_box(schema);
        })
    });
}

criterion_group!(
    benches,
    benchmark_registry_lookup,
    benchmark_registry_lookup_1000,
    benchmark_tool_execution,
    benchmark_register,
    benchmark_tool_descriptions,
    benchmark_schema_build
);
criterion_main!(benches);
