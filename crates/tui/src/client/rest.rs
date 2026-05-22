use crate::client::error::TuiError;
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
        Self {
            client,
            base_url: config.url.trim_end_matches('/').to_string(),
        }
    }

    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, TuiError> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(TuiError::HttpStatus { status, body })
        }
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
        token: &str,
    ) -> Result<(), TuiError> {
        let url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&SendMessageRequest {
                content: vec![MessageContentPart::Text {
                    text: content.to_string(),
                }],
            })
            .send()
            .await?;
        Self::check_status(resp).await?;
        Ok(())
    }

    pub async fn interrupt(&self, session_id: &str, token: &str) -> Result<(), TuiError> {
        let url = format!(
            "{}/api/v1/sessions/{}/messages/current",
            self.base_url, session_id
        );
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        Self::check_status(resp).await?;
        Ok(())
    }

    pub async fn list_sessions(&self, token: &str) -> Result<Vec<SessionInfo>, TuiError> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<Vec<SessionInfo>>().await?)
    }

    pub async fn get_session(
        &self,
        session_id: &str,
        token: &str,
    ) -> Result<SessionInfo, TuiError> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<SessionInfo>().await?)
    }

    pub async fn create_session(
        &self,
        title: Option<&str>,
        token: &str,
    ) -> Result<SessionInfo, TuiError> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&CreateSessionRequest {
                title: title.map(|t| t.to_string()),
            })
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<SessionInfo>().await?)
    }

    pub async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
        token: &str,
    ) -> Result<SessionInfo, TuiError> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "title": title }))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<SessionInfo>().await?)
    }

    pub async fn update_model(
        &self,
        session_id: &str,
        model: &str,
        token: &str,
    ) -> Result<SessionInfo, TuiError> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<SessionInfo>().await?)
    }

    pub async fn compact_session(&self, session_id: &str, token: &str) -> Result<(), TuiError> {
        let url = format!("{}/api/v1/sessions/{}/compact", self.base_url, session_id);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        Self::check_status(resp).await?;
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str, token: &str) -> Result<(), TuiError> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        Self::check_status(resp).await?;
        Ok(())
    }

    pub async fn get_session_messages(
        &self,
        session_id: &str,
        token: &str,
    ) -> Result<Vec<crate::client::model::HistoricalMessage>, TuiError> {
        let url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp
            .json::<Vec<crate::client::model::HistoricalMessage>>()
            .await?)
    }

    pub async fn update_system_prompt(
        &self,
        session_id: &str,
        prompt: &str,
        token: &str,
    ) -> Result<SessionInfo, TuiError> {
        let url = format!("{}/api/v1/sessions/{}", self.base_url, session_id);
        let resp = self
            .client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "system_prompt": prompt }))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json::<SessionInfo>().await?)
    }
}
