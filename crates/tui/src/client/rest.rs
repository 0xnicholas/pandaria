use crate::client::model::*;
use crate::config::ServerConfig;
use reqwest::Client;
use std::time::Duration;

#[derive(Clone)]
pub struct RestClient {
    client: Client,
    base_url: String,
}

impl RestClient {
    pub fn new(config: &ServerConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .expect("failed to build reqwest client");
        Self { client, base_url: config.url.trim_end_matches('/').to_string() }
    }

    pub async fn send_message(&self, session_id: &str, content: &str, token: &str) -> Result<(), String> {
        let url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        let resp = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&SendMessageRequest { content: content.to_string() })
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { Ok(()) }
        else { Err(format!("HTTP {}: {}", resp.status().as_u16(), resp.text().await.unwrap_or_default())) }
    }

    pub async fn interrupt(&self, session_id: &str, token: &str) -> Result<(), String> {
        let url = format!("{}/api/v1/sessions/{}/messages/current", self.base_url, session_id);
        let resp = self.client.delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { Ok(()) }
        else { Err(format!("HTTP {}: {}", resp.status().as_u16(), resp.text().await.unwrap_or_default())) }
    }

    pub async fn list_sessions(&self, token: &str) -> Result<Vec<SessionInfo>, String> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let resp = self.client.get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { resp.json::<Vec<SessionInfo>>().await.map_err(|e| e.to_string()) }
        else { Err(format!("HTTP {}", resp.status().as_u16())) }
    }

    pub async fn get_session(&self, session_id: &str, token: &str) -> Result<SessionInfo, String> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self.client.get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { resp.json::<SessionInfo>().await.map_err(|e| e.to_string()) }
        else { Err(format!("HTTP {}", resp.status().as_u16())) }
    }

    pub async fn create_session(&self, title: Option<&str>, token: &str) -> Result<SessionInfo, String> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let resp = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&CreateSessionRequest { title: title.map(|t| t.to_string()) })
            .send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() { resp.json::<SessionInfo>().await.map_err(|e| e.to_string()) }
        else { Err(format!("HTTP {}", resp.status().as_u16())) }
    }
}
