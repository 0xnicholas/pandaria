use agent_core::types::AgentMessage;

#[test]
fn test_message_clone_equality() {
    let msg = AgentMessage::User(ai_provider::UserMessage {
        content: vec![ai_provider::Content::Text {
            text: "test".to_string(),
            text_signature: None,
        }],
        timestamp: std::time::SystemTime::UNIX_EPOCH,
    });

    let cloned = msg.clone();
    assert_eq!(msg, cloned);
}
