use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_rest_client_list_sessions() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"id": "s1", "title": null, "model": "gpt-4o", "context_window": 200000, "created_at": null}
        ])))
        .mount(&server)
        .await;
    let config = tui::config::ServerConfig {
        url: server.uri(),
        timeout_secs: 5,
    };
    let client = tui::client::rest::RestClient::new(&config);
    let sessions = client
        .list_sessions("test-token")
        .await
        .expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "s1");
}

#[tokio::test]
async fn test_rest_client_create_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sessions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "id": "new-session", "title": "my session", "model": "gpt-4o",
            "context_window": 200000, "created_at": null
        })))
        .mount(&server)
        .await;
    let config = tui::config::ServerConfig {
        url: server.uri(),
        timeout_secs: 5,
    };
    let client = tui::client::rest::RestClient::new(&config);
    let session = client
        .create_session(Some("my session"), "test-token")
        .await
        .expect("create session");
    assert_eq!(session.id, "new-session");
    assert_eq!(session.title, Some("my session".to_string()));
}

#[tokio::test]
async fn test_sse_connect_receives_events() {
    let server = MockServer::start().await;
    let sse_body = "data: {\"type\": \"text_delta\", \"content_index\": 0, \"delta\": \"hello\"}\r\n\r\n\
                     data: {\"type\": \"turn_end\", \"stop_reason\": \"stop\", \"usage\": {\"input_tokens\": 10, \"output_tokens\": 5}}\r\n\r\n";
    Mock::given(method("GET"))
        .and(path("/api/v1/sessions/s1/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let base = server.uri();
    let handle =
        tokio::spawn(
            async move { tui::client::sse::connect(&client, &base, "s1", "token", tx).await },
        );

    let mut events: Vec<tui::client::model::ServerEvent> = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_terminal = matches!(event, tui::client::model::ServerEvent::TurnEnd { .. });
        events.push(event);
        if is_terminal {
            break;
        }
    }

    assert_eq!(events.len(), 2, "expected 2 events, got: {events:?}");
    assert!(matches!(
        events[0],
        tui::client::model::ServerEvent::TextDelta { .. }
    ));
    assert!(matches!(
        events[1],
        tui::client::model::ServerEvent::TurnEnd { .. }
    ));

    let _ = handle.await;
}

#[test]
fn test_command_parser_roundtrip() {
    let cmds = vec![
        "/quit",
        "/q",
        "/new",
        "/new test",
        "/switch abc",
        "/list",
        "/model",
        "/model gpt-4o",
        "/clear",
        "/help",
        "/connect http://localhost",
        "/auth sk-token",
        "/tokens",
    ];
    for c in cmds {
        assert!(
            tui::command::Command::parse(c).is_some(),
            "failed to parse: {}",
            c
        );
    }
}

#[test]
fn test_config_defaults() {
    let cli = tui::config::CliArgs {
        url: None,
        token: None,
        theme: None,
        config: None,
    };
    let config = tui::config::Config::load(cli).expect("load config");
    assert_eq!(config.server.url, "http://localhost:8080");
    assert_eq!(config.ui.max_history, 500);
}

#[tokio::test]
async fn test_rest_client_send_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sessions/s1/messages"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let config = tui::config::ServerConfig {
        url: server.uri(),
        timeout_secs: 5,
    };
    let client = tui::client::rest::RestClient::new(&config);
    client
        .send_message("s1", "hello", "test-token")
        .await
        .expect("send message");
}

#[tokio::test]
async fn test_rest_client_interrupt() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/sessions/s1/messages/current"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let config = tui::config::ServerConfig {
        url: server.uri(),
        timeout_secs: 5,
    };
    let client = tui::client::rest::RestClient::new(&config);
    client
        .interrupt("s1", "test-token")
        .await
        .expect("interrupt");
}

#[test]
fn test_keybinding_parse_all_valid_keys() {
    // Verify all user-configurable keybinding keys parse successfully
    let keys = vec![
        "app.quit",
        "app.interrupt",
        "app.toggle_tool_calls",
        "app.toggle_thinking",
        "app.select_model",
        "app.list_sessions",
        "app.new_session",
        "app.open_command_palette",
        "editor.cursor_up",
        "editor.cursor_down",
        "editor.cursor_left",
        "editor.cursor_right",
        "editor.cursor_word_left",
        "editor.cursor_word_right",
        "editor.cursor_line_start",
        "editor.cursor_line_end",
        "editor.page_up",
        "editor.page_down",
        "editor.delete_char_backward",
        "editor.delete_char_forward",
        "editor.delete_word_backward",
        "editor.delete_word_forward",
        "editor.delete_to_line_start",
        "editor.delete_to_line_end",
        "editor.new_line",
        "editor.submit",
        "editor.undo",
        "editor.yank",
        "editor.yank_pop",
        "autocomplete.trigger",
    ];
    for key in keys {
        assert!(
            tui::keybindings::KeybindingsManager::parse_keybinding_key(key).is_some(),
            "failed to parse keybinding key: {key}"
        );
    }
}
