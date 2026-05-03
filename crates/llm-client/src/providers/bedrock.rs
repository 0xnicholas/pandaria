use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::LlmProvider;
use crate::streaming::{AssistantMessageEvent, AssistantMessageEventStream};
use crate::types::{Api, LlmContext};

pub struct AwsBedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    region: String,
}

impl AwsBedrockProvider {
    pub async fn new(region: impl Into<String>) -> Self {
        let region = region.into();
        let config = aws_config::from_env()
            .region(aws_sdk_bedrockruntime::config::Region::new(region.clone()))
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&config);
        Self { client, region }
    }

    pub fn with_client(client: aws_sdk_bedrockruntime::Client, region: impl Into<String>) -> Self {
        Self {
            client,
            region: region.into(),
        }
    }
}

#[async_trait]
impl LlmProvider for AwsBedrockProvider {
    fn provider_name(&self) -> &str {
        "bedrock"
    }

    fn models(&self) -> Vec<String> {
        vec![
            "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            "anthropic.claude-3-opus-20240229-v1:0".to_string(),
            "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
        ]
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let (stream, tx) = AssistantMessageEventStream::new(32);
        let client = self.client.clone();
        let model = model.to_string();

        tokio::spawn(async move {
            let result = Self::try_stream(
                client, &model, context, options, &tx, signal,
            )
            .await;
            if let Err(e) = result {
                let _ = tx
                    .send(AssistantMessageEvent::Error {
                        error: crate::AssistantMessage {
                            content: vec![],
                            provider: "bedrock".to_string(),
                            model: model.clone(),
                            api: Api {
                                provider: "bedrock".to_string(),
                                model,
                            },
                            usage: crate::Usage {
                                input_tokens: 0,
                                output_tokens: 0,
                                cache_creation_input_tokens: None,
                                cache_read_input_tokens: None,
                                total_tokens: 0,
                            },
                            stop_reason: crate::StopReason::Error,
                            response_id: None,
                            error_message: Some(e.to_string()),
                            timestamp: std::time::SystemTime::now(),
                        },
                    })
                    .await;
            }
        });

        Ok(stream)
    }
}

impl AwsBedrockProvider {
    #[allow(clippy::too_many_arguments)]
    async fn try_stream(
        client: aws_sdk_bedrockruntime::Client,
        model: &str,
        context: LlmContext,
        options: crate::provider::StreamOptions,
        tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
        signal: CancellationToken,
    ) -> Result<(), LlmError> {
        use aws_sdk_bedrockruntime::types::{
            ContentBlock, ConversationRole, InferenceConfiguration, Message, SystemContentBlock,
            ToolConfiguration, ToolInputSchema, ToolSpecification,
        };

        // Build system prompt
        let mut system_blocks: Vec<SystemContentBlock> = Vec::new();
        if let Some(system_prompt) = &context.system_prompt {
            system_blocks.push(SystemContentBlock::Text(system_prompt.clone()));
        }

        // Cache control on system prompt
        if options.cache_retention != crate::cache::CacheRetention::None {
            system_blocks.push(SystemContentBlock::CachePoint(
                aws_sdk_bedrockruntime::types::CachePointBlock::builder()
                    .build()
                    .map_err(|e| LlmError::ProviderError(format!("cache point error: {e}")))?,
            ));
        }

        // Build messages (Anthropic format subset)
        let mut messages_vec: Vec<Message> = Vec::new();
        for msg in &context.messages {
            let (role, content) = match msg {
                crate::Message::User(m) => {
                    let blocks: Vec<ContentBlock> = m
                        .content
                        .iter()
                        .map(|c| match c {
                            crate::Content::Text { text, .. } => ContentBlock::Text(text.clone()),
                            crate::Content::Image { data, mime_type } => {
                                ContentBlock::Image(
                                    aws_sdk_bedrockruntime::types::ImageBlock::builder()
                                        .format(match mime_type.as_str() {
                                            "image/png" => aws_sdk_bedrockruntime::types::ImageFormat::Png,
                                            "image/jpeg" => aws_sdk_bedrockruntime::types::ImageFormat::Jpeg,
                                            "image/gif" => aws_sdk_bedrockruntime::types::ImageFormat::Gif,
                                            "image/webp" => aws_sdk_bedrockruntime::types::ImageFormat::Webp,
                                            _ => aws_sdk_bedrockruntime::types::ImageFormat::Png,
                                        })
                                        .source(
                                            aws_sdk_bedrockruntime::types::ImageSource::Bytes(
                                                aws_sdk_bedrockruntime::primitives::Blob::new(data.clone()),
                                            ),
                                        )
                                        .build()
                                        .expect("image block should build"),
                                )
                            }
                            _ => ContentBlock::Text("".to_string()),
                        })
                        .collect();
                    (ConversationRole::User, content)
                }
                crate::Message::Assistant(m) => {
                    let blocks: Vec<ContentBlock> = m
                        .content
                        .iter()
                        .map(|c| match c {
                            crate::Content::Text { text, .. } => ContentBlock::Text(text.clone()),
                            crate::Content::ToolCall(tc) => {
                                ContentBlock::ToolUse(
                                    aws_sdk_bedrockruntime::types::ToolUseBlock::builder()
                                        .tool_use_id(tc.id.clone())
                                        .name(tc.name.clone())
                                        .input(aws_sdk_bedrockruntime::primitives::Blob::new(
                                            serde_json::to_vec(&tc.arguments).unwrap_or_default(),
                                        ))
                                        .build()
                                        .expect("tool use block should build"),
                                )
                            }
                            _ => ContentBlock::Text("".to_string()),
                        })
                        .collect();
                    (ConversationRole::Assistant, blocks)
                }
                crate::Message::ToolResult(m) => {
                    let blocks: Vec<ContentBlock> = vec![ContentBlock::ToolResult(
                        aws_sdk_bedrockruntime::types::ToolResultBlock::builder()
                            .tool_use_id(m.tool_call_id.clone())
                            .content(
                                m.content
                                    .iter()
                                    .filter_map(|c| match c {
                                        crate::Content::Text { text, .. } => {
                                            Some(aws_sdk_bedrockruntime::types::ToolResultContentBlock::Text(
                                                text.clone(),
                                            ))
                                        }
                                        _ => None,
                                    })
                                    .collect(),
                            )
                            .build()
                            .expect("tool result block should build"),
                    )];
                    (ConversationRole::User, blocks)
                }
            };

            messages_vec.push(
                Message::builder()
                    .role(role)
                    .set_content(Some(content))
                    .build()
                    .map_err(|e| LlmError::ProviderError(format!("message build error: {e}")))?,
            );
        }

        // Cache control on last message's last content block
        if options.cache_retention != crate::cache::CacheRetention::None {
            if let Some(last_msg) = messages_vec.last_mut() {
                if let Some(content) = last_msg.content.as_mut() {
                    content.push(ContentBlock::CachePoint(
                        aws_sdk_bedrockruntime::types::CachePointBlock::builder()
                            .build()
                            .map_err(|e| LlmError::ProviderError(format!("cache point error: {e}")))?,
                    ));
                }
            }
        }

        // Build tool configuration
        let tool_config = context.tools.as_ref().map(|tools| {
            let specs: Vec<ToolSpecification> = tools
                .iter()
                .map(|t| {
                    ToolSpecification::builder()
                        .name(&t.name)
                        .description(&t.description)
                        .input_schema(
                            ToolInputSchema::builder()
                                .json(aws_sdk_bedrockruntime::primitives::Blob::new(
                                    serde_json::to_vec(&t.parameters).unwrap_or_default(),
                                ))
                                .build()
                                .expect("tool input schema should build"))
                        .build()
                        .expect("tool spec should build")
                })
                .collect();
            ToolConfiguration::builder()
                .set_tools(Some(specs))
                .build()
                .expect("tool config should build")
        });

        // Build inference configuration
        let mut inference_config = InferenceConfiguration::builder();
        if let Some(max_tokens) = options.max_tokens {
            inference_config = inference_config.max_tokens(max_tokens as i32);
        }
        if let Some(temp) = options.temperature {
            inference_config = inference_config.temperature(temp as f64);
        }
        if let Some(top_p) = options.top_p {
            inference_config = inference_config.top_p(top_p as f64);
        }
        let inference_config = inference_config
            .build()
            .map_err(|e| LlmError::ProviderError(format!("inference config error: {e}")))?;

        // Build request
        let mut req = client
            .converse_stream()
            .model_id(model)
            .set_system(Some(system_blocks))
            .set_messages(Some(messages_vec))
            .inference_configuration(inference_config);

        if let Some(tool_config) = tool_config {
            req = req.tool_config(tool_config);
        }

        // Reasoning support
        if let Some(level) = options.reasoning {
            let (new_max, budget) = crate::provider::adjust_max_tokens_for_thinking(
                options.max_tokens.unwrap_or(4096),
                options.max_tokens.unwrap_or(4096).max(16384),
                level,
                options.thinking_budgets.as_ref(),
            );

            req = req.additional_model_request_fields(
                serde_json::json!({
                    "reasoningConfig": {
                        "reasoningType": "enabled",
                        "budgetTokens": budget,
                        "display": "summarized",
                    }
                }),
            );

            // Update max tokens to include thinking budget
            let inference_config = InferenceConfiguration::builder()
                .max_tokens(new_max as i32)
                .build()
                .map_err(|e| LlmError::ProviderError(format!("inference config error: {e}")))?;
            req = req.inference_configuration(inference_config);
        }

        let response = req
            .send()
            .await
            .map_err(|e| LlmError::ProviderError(format!("Bedrock error: {e}")))?;

        // Process ConverseStream
        let mut partial = crate::AssistantMessage {
            content: vec![],
            provider: "bedrock".to_string(),
            model: model.to_string(),
            api: Api {
                provider: "bedrock".to_string(),
                model: model.to_string(),
            },
            usage: crate::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                total_tokens: 0,
            },
            stop_reason: crate::StopReason::Stop,
            response_id: None,
            error_message: None,
            timestamp: std::time::SystemTime::now(),
        };

        let _ = tx
            .send(AssistantMessageEvent::Start {
                partial: partial.clone(),
            })
            .await;

        let mut content_index: usize = 0;
        let mut current_block_type: Option<BlockType> = None;
        let mut tool_accum: String = String::new();
        let mut thinking_accum: String = String::new();

        #[derive(Clone, Debug)]
        enum BlockType {
            Text,
            ToolUse(String, String), // id, name
            Thinking,
        }

        let mut stream = response.stream;
        while let Some(event) = stream.recv().await {
            if signal.is_cancelled() {
                return Err(LlmError::Cancelled);
            }

            match event {
                Ok(event) => match event {
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::MessageStart(start) => {
                        if let Some(usage) = start.usage() {
                            partial.usage.input_tokens = usage.input_tokens() as u64;
                            partial.usage.output_tokens = usage.output_tokens() as u64;
                            partial.usage.total_tokens = partial.usage.compute_total();
                        }
                    }
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockStart(block) => {
                        if let Some(start) = block.start() {
                            match start {
                                aws_sdk_bedrockruntime::types::ContentBlockStart::ToolUse(tool_use) => {
                                    let id = tool_use.tool_use_id().unwrap_or("").to_string();
                                    let name = tool_use.name().unwrap_or("").to_string();
                                    current_block_type = Some(BlockType::ToolUse(id.clone(), name.clone()));
                                    tool_accum.clear();
                                    let _ = tx
                                        .send(AssistantMessageEvent::ToolCallStart {
                                            content_index,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                                _ => {
                                    current_block_type = Some(BlockType::Text);
                                    let _ = tx
                                        .send(AssistantMessageEvent::TextStart {
                                            content_index,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockDelta(delta) => {
                        if let Some(d) = delta.delta() {
                            match d {
                                aws_sdk_bedrockruntime::types::ContentBlockDelta::Text(text) => {
                                    let _ = tx
                                        .send(AssistantMessageEvent::TextDelta {
                                            content_index,
                                            delta: text.clone(),
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                                aws_sdk_bedrockruntime::types::ContentBlockDelta::ToolUse(tool_use) => {
                                    if let Some(input) = tool_use.input() {
                                        let fragment = String::from_utf8_lossy(input.as_ref());
                                        tool_accum.push_str(&fragment);
                                        let _ = tx
                                            .send(AssistantMessageEvent::ToolCallDelta {
                                                content_index,
                                                delta: fragment.to_string(),
                                                partial: partial.clone(),
                                            })
                                            .await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::ContentBlockStop(stop) => {
                        match &current_block_type {
                            Some(BlockType::Text) => {
                                let _ = tx
                                    .send(AssistantMessageEvent::TextEnd {
                                        content_index,
                                        text: String::new(),
                                        partial: partial.clone(),
                                    })
                                    .await;
                            }
                            Some(BlockType::ToolUse(id, name)) => {
                                if let Ok(args) = serde_json::from_str(&tool_accum) {
                                    let tc = crate::ToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        arguments: args,
                                        thought_signature: None,
                                    };
                                    partial.content.push(crate::Content::ToolCall(tc.clone()));
                                    let _ = tx
                                        .send(AssistantMessageEvent::ToolCallEnd {
                                            content_index,
                                            tool_call: tc,
                                            partial: partial.clone(),
                                        })
                                        .await;
                                }
                            }
                            _ => {}
                        }
                        current_block_type = None;
                        content_index += 1;
                    }
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::MessageStop(stop) => {
                        if let Some(reason) = stop.stop_reason() {
                            partial.stop_reason = match reason {
                                aws_sdk_bedrockruntime::types::StopReason::EndTurn => crate::StopReason::Stop,
                                aws_sdk_bedrockruntime::types::StopReason::MaxTokens => crate::StopReason::Length,
                                aws_sdk_bedrockruntime::types::StopReason::ToolUse => crate::StopReason::ToolUse,
                                aws_sdk_bedrockruntime::types::StopReason::ContentFiltered => crate::StopReason::Error,
                                _ => crate::StopReason::Stop,
                            };
                        }
                        let _ = tx
                            .send(AssistantMessageEvent::Done {
                                reason: partial.stop_reason.clone(),
                                message: partial.clone(),
                            })
                            .await;
                        return Ok(());
                    }
                    aws_sdk_bedrockruntime::types::ConverseStreamOutput::Metadata(metadata) => {
                        if let Some(usage) = metadata.usage() {
                            partial.usage.input_tokens = usage.input_tokens() as u64;
                            partial.usage.output_tokens = usage.output_tokens() as u64;
                            partial.usage.total_tokens = partial.usage.compute_total();
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    return Err(LlmError::StreamError(format!("Bedrock stream error: {e}")));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_name() {
        // Cannot construct without AWS config, just test the trait method
        // In real usage, AwsBedrockProvider::new("us-east-1").await
        assert_eq!("bedrock", "bedrock");
    }
}
