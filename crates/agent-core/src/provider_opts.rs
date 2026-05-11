use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct ProviderStreamOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning: Option<llm_client::ReasoningLevel>,
    pub max_retries: Option<u32>,
    pub timeout: Option<Duration>,
}

impl ProviderStreamOptions {
    pub fn from_options(options: &llm_client::StreamOptions) -> Self {
        Self {
            max_tokens: options.max_tokens,
            temperature: options.temperature,
            top_p: options.top_p,
            reasoning: options.reasoning,
            max_retries: Some(options.max_retries),
            timeout: Some(options.timeout),
        }
    }
}
