use llm_client::{
    calculate_cost, get_model, models_are_equal, models_for_provider, providers, supports_xhigh,
};

#[test]
fn test_get_model_found() {
    let m = get_model("anthropic", "claude-sonnet-4-20250514");
    assert!(m.is_some());
    assert_eq!(m.unwrap().id, "claude-sonnet-4-20250514");
}

#[test]
fn test_get_model_not_found() {
    assert!(get_model("openai", "nonexistent-model-xyz").is_none());
}

#[test]
fn test_models_for_provider() {
    let models = models_for_provider("openai");
    assert!(!models.is_empty());
    // Should contain known OpenAI models
    assert!(models.iter().any(|m| m.id.contains("gpt")));
}

#[test]
fn test_providers_list() {
    let p = providers();
    assert!(p.iter().any(|s| s == "anthropic"));
    assert!(p.iter().any(|s| s == "openai"));
    assert!(p.iter().any(|s| s == "google"));
}

#[test]
fn test_calculate_cost() {
    let model = get_model("anthropic", "claude-sonnet-4-20250514").unwrap();
    let usage = llm_client::Usage {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cache_read_input_tokens: Some(100_000),
        cache_creation_input_tokens: Some(50_000),
        total_tokens: 1_650_000,
    };
    let cost = calculate_cost(&model, &usage);
    assert!((cost.input - 3.0).abs() < 0.01);
    assert!((cost.output - 7.5).abs() < 0.01);
}

#[test]
fn test_supports_xhigh() {
    assert!(supports_xhigh("gpt-5.2"));
    assert!(!supports_xhigh("gpt-4.1"));
}

#[test]
fn test_models_are_equal() {
    let a = get_model("openai", "gpt-5.2");
    let b = get_model("openai", "gpt-5.2");
    assert!(models_are_equal(a.as_ref(), b.as_ref()));
    let c = get_model("openai", "gpt-5.3");
    assert!(!models_are_equal(a.as_ref(), c.as_ref()));
    assert!(!models_are_equal(None, a.as_ref()));
    assert!(!models_are_equal(a.as_ref(), None));
}
