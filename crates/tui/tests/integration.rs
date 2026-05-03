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
    let config = tui::config::ServerConfig { url: server.uri(), timeout_secs: 5 };
    let client = tui::client::rest::RestClient::new(&config);
    let sessions = client.list_sessions("test-token").await.expect("list sessions");
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
    let config = tui::config::ServerConfig { url: server.uri(), timeout_secs: 5 };
    let client = tui::client::rest::RestClient::new(&config);
    let session = client.create_session(Some("my session"), "test-token").await.expect("create session");
    assert_eq!(session.id, "new-session");
    assert_eq!(session.title, Some("my session".to_string()));
}

#[test]
fn test_command_parser_roundtrip() {
    let cmds = vec![
        "/quit", "/q", "/new", "/new test", "/switch abc",
        "/list", "/model", "/model gpt-4o", "/clear", "/help",
        "/connect http://localhost", "/auth sk-token", "/tokens",
    ];
    for c in cmds {
        assert!(tui::command::Command::parse(c).is_some(), "failed to parse: {}", c);
    }
}

#[test]
fn test_config_defaults() {
    let cli = tui::config::CliArgs { url: None, token: None, theme: None, config: None };
    let config = tui::config::Config::load(cli).expect("load config");
    assert_eq!(config.server.url, "http://localhost:8080");
    assert_eq!(config.ui.max_history, 500);
}
