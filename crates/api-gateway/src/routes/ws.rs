use axum::{
    extract::{
        Extension, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::server::AppState;
use crate::{error::GatewayError, middleware::TenantId};

pub async fn session_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(tenant_id): Extension<TenantId>,
    Path(session_id): Path<Uuid>,
) -> Result<Response, GatewayError> {
    // Validate session exists before upgrading
    state
        .tenant_manager
        .get_session(&tenant_id.0, &session_id)
        .await?;

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, tenant_id.0, session_id, state)))
}

async fn handle_socket(
    mut socket: axum::extract::ws::WebSocket,
    tenant_id: String,
    session_id: Uuid,
    state: Arc<AppState>,
) {
    let mut rx = match state
        .tenant_manager
        .subscribe_events(&tenant_id, &session_id)
        .await
    {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "failed to subscribe to session events");
            let _ = socket
                .send(Message::Text(
                    serde_json::json!({"type":"error","code":"subscribe_failed","message":"failed to subscribe to events"}).to_string().into(),
                ))
                .await;
            return;
        }
    };

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                let server_event = map_agent_event(event);
                let text = match serde_json::to_string(&server_event) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to serialize event");
                        continue;
                    }
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = interval.tick() => {
                let ping = serde_json::json!({"type":"ping"}).to_string();
                if socket.send(Message::Text(ping.into())).await.is_err() {
                    break;
                }
            }
            Some(Ok(msg)) = socket.recv() => {
                match msg {
                    Message::Text(text) => {
                        if let Err(e) = handle_client_message(&text, &tenant_id, &session_id, &state).await {
                            tracing::warn!(error = %e, "websocket client message error");
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            else => break,
        }
    }
}

async fn handle_client_message(
    text: &str,
    tenant_id: &str,
    session_id: &Uuid,
    state: &Arc<AppState>,
) -> Result<(), GatewayError> {
    #[derive(serde::Deserialize)]
    struct ClientWsMessage {
        action: String,
        #[serde(default)]
        content: Vec<crate::types::MessageContentPart>,
    }

    let msg: ClientWsMessage = serde_json::from_str(text).map_err(|_| {
        GatewayError::Tenant(tenant::TenantError::BadRequest("invalid_json".to_string()))
    })?;

    match msg.action.as_str() {
        "send_message" => {
            let content = msg
                .content
                .into_iter()
                .map(|p| match p {
                    crate::types::MessageContentPart::Text { text } => ai_provider::Content::Text {
                        text,
                        text_signature: None,
                    },
                    crate::types::MessageContentPart::Image { data, mime_type } => {
                        ai_provider::Content::Image { data, mime_type }
                    }
                    crate::types::MessageContentPart::Video { data, mime_type } => {
                        ai_provider::Content::Video { data, mime_type }
                    }
                    crate::types::MessageContentPart::Audio { data, mime_type } => {
                        ai_provider::Content::Audio { data, mime_type }
                    }
                })
                .collect();
            state
                .tenant_manager
                .send_message(tenant_id, session_id, content)
                .await?;
        }
        "interrupt" => {
            state
                .tenant_manager
                .interrupt(tenant_id, session_id)
                .await?;
        }
        "pong" => {}
        _ => {}
    }

    Ok(())
}

fn map_agent_event(event: agent_core::AgentEvent) -> crate::types::ServerEvent {
    use agent_core::AgentEvent;

    match event {
        AgentEvent::TurnStart { turn_index } => crate::types::ServerEvent::TurnStart { turn_index },
        AgentEvent::MessageStart { message_index } => {
            crate::types::ServerEvent::MessageStart { message_index }
        }
        AgentEvent::MessageUpdate { content_delta, .. } => crate::types::ServerEvent::TextDelta {
            delta: content_delta,
        },
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
        } => crate::types::ServerEvent::ToolCallStarted {
            call_id: tool_call_id,
            name: tool_name,
        },
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            result,
        } => {
            let result_text = extract_tool_result_text(&result.content);
            crate::types::ServerEvent::ToolCallDone {
                call_id: tool_call_id,
                result: result_text,
                is_error: result.is_error,
            }
        }
        AgentEvent::TurnEnd { messages, .. } => {
            let (stop_reason, usage) = extract_turn_end_info(&messages);
            crate::types::ServerEvent::TurnEnd { stop_reason, usage }
        }
        AgentEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
        } => crate::types::ServerEvent::AutoRetryStart {
            attempt,
            max_attempts,
            delay_ms,
        },
        AgentEvent::AutoRetryEnd { success, error } => {
            crate::types::ServerEvent::AutoRetryEnd { success, error }
        }
        AgentEvent::Error { error } => crate::types::ServerEvent::Error {
            code: error.code().to_string(),
            message: error.to_sanitized_string(),
        },
        AgentEvent::StateChanged { state } => crate::types::ServerEvent::StateChanged {
            state: format!("{:?}", state).to_lowercase(),
        },
        _ => crate::types::ServerEvent::TextDelta {
            delta: String::new(),
        },
    }
}

fn extract_tool_result_text(contents: &[agent_core::Content]) -> Option<String> {
    let parts: Vec<String> = contents
        .iter()
        .map(|c| match c {
            agent_core::Content::Text { text, .. } => text.clone(),
            agent_core::Content::Image { mime_type, .. } => format!("[image: {}]", mime_type),
            agent_core::Content::Video { mime_type, .. } => format!("[video: {}]", mime_type),
            agent_core::Content::Audio { mime_type, .. } => format!("[audio: {}]", mime_type),
            _ => String::new(),
        })
        .collect();
    let joined = parts.join("\n");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn extract_turn_end_info(
    messages: &[agent_core::AgentMessage],
) -> (String, Option<crate::types::UsageInfo>) {
    let last_assistant = messages.iter().rev().find_map(|m| match m {
        agent_core::AgentMessage::Assistant(a) => Some(a),
        _ => None,
    });

    match last_assistant {
        Some(a) => {
            let stop_reason = format!("{:?}", a.stop_reason).to_lowercase();
            let usage = Some(crate::types::UsageInfo {
                input_tokens: a.usage.input_tokens,
                output_tokens: a.usage.output_tokens,
            });
            (stop_reason, usage)
        }
        None => ("unknown".into(), None),
    }
}
