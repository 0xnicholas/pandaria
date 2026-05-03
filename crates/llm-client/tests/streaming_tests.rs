use llm_client::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, StopReason,
    Usage,
};
use std::time::SystemTime;

fn make_partial(provider: &str, model: &str) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: provider.to_string(),
        model: model.to_string(),
        api: Api {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        usage: Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            total_tokens: 0,
        },
        stop_reason: StopReason::Stop,
        response_id: None,
        error_message: None,
        timestamp: SystemTime::UNIX_EPOCH,
    }
}

#[tokio::test]
async fn test_event_stream_push_next() {
    let (mut stream, tx) = AssistantMessageEventStream::new(32);
    let partial = make_partial("test", "v1");

    tx.send(AssistantMessageEvent::Start {
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: partial.clone(),
    })
    .await
    .unwrap();
    drop(tx);

    let event1 = stream.next().await;
    assert!(matches!(event1, Some(AssistantMessageEvent::Start { .. })));

    let event2 = stream.next().await;
    assert!(matches!(event2, Some(AssistantMessageEvent::Done { .. })));

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_event_stream_to_message_done() {
    let (stream, tx) = AssistantMessageEventStream::new(32);
    let partial = make_partial("test", "v1");

    tx.send(AssistantMessageEvent::Start {
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: partial.clone(),
    })
    .await
    .unwrap();
    drop(tx);

    let result = stream.to_message().await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().stop_reason, StopReason::Stop);
}

#[tokio::test]
async fn test_event_stream_to_message_error() {
    let (stream, tx) = AssistantMessageEventStream::new(32);
    let partial = make_partial("test", "v1");

    tx.send(AssistantMessageEvent::Start {
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::Error {
        error: AssistantMessage {
            error_message: Some("something went wrong".to_string()),
            ..partial.clone()
        },
    })
    .await
    .unwrap();
    drop(tx);

    let result = stream.to_message().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_event_stream_to_message_no_terminal() {
    let (stream, tx) = AssistantMessageEventStream::new(32);
    let partial = make_partial("test", "v1");

    tx.send(AssistantMessageEvent::Start {
        partial: partial.clone(),
    })
    .await
    .unwrap();
    drop(tx);

    let result = stream.to_message().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_event_stream_content_index_tracking() {
    let (mut stream, tx) = AssistantMessageEventStream::new(32);
    let partial = make_partial("test", "v1");

    tx.send(AssistantMessageEvent::TextStart {
        content_index: 0,
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: "Hello ".to_string(),
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::TextEnd {
        content_index: 0,
        text: "Hello ".to_string(),
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::TextStart {
        content_index: 1,
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::TextDelta {
        content_index: 1,
        delta: "world".to_string(),
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::TextEnd {
        content_index: 1,
        text: "world".to_string(),
        partial: partial.clone(),
    })
    .await
    .unwrap();
    tx.send(AssistantMessageEvent::Done {
        reason: StopReason::Stop,
        message: partial.clone(),
    })
    .await
    .unwrap();
    drop(tx);

    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextStart { content_index: 0, .. })
    );
    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextDelta { content_index: 0, .. })
    );
    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextEnd { content_index: 0, .. })
    );
    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextStart { content_index: 1, .. })
    );
    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextDelta { content_index: 1, .. })
    );
    let event = stream.next().await.unwrap();
    assert!(
        matches!(event, AssistantMessageEvent::TextEnd { content_index: 1, .. })
    );
}
