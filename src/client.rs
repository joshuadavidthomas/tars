use crate::protocol::{SendMessageRequest, SessionCreateResponse, StreamEvent};
use futures::StreamExt;
use reqwest::Client as HttpClient;
use std::error::Error;
use std::future::Future;
use std::path::PathBuf;

pub struct ClientConfig {
    pub base_url: String,
    pub token: String,
}

#[derive(Clone)]
pub struct ClientSession {
    base_url: String,
    token: String,
    session_id: String,
    http: HttpClient,
}

type ClientResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn resolve_token(explicit: Option<String>) -> ClientResult<String> {
    if let Some(token) = explicit {
        return Ok(token);
    }

    read_token_file().map_err(|_| {
        "No auth token found; pass --token, set TARS_TOKEN, or start the server to create one."
            .into()
    })
}

impl ClientSession {
    pub async fn connect(config: ClientConfig) -> ClientResult<Self> {
        let base_url = normalize_base_url(&config.base_url);
        let http = HttpClient::new();

        let response = http
            .post(format!("{}/sessions", base_url))
            .bearer_auth(&config.token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Failed to create session: {} - {}", status, body).into());
        }

        let body: SessionCreateResponse = response.json().await?;

        Ok(Self {
            base_url,
            token: config.token,
            session_id: body.session_id,
            http,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn send_message(&self, content: String) -> ClientResult<()> {
        let request = SendMessageRequest { content };
        let response = self
            .http
            .post(format!(
                "{}/sessions/{}/messages",
                self.base_url, self.session_id
            ))
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Failed to send message: {} - {}", status, body).into());
        }

        Ok(())
    }

    pub async fn stream_events<F, Fut>(&self, mut on_event: F) -> ClientResult<()>
    where
        F: FnMut(StreamEvent) -> Fut,
        Fut: Future<Output = ()>,
    {
        let response = self
            .http
            .get(format!(
                "{}/sessions/{}/stream",
                self.base_url, self.session_id
            ))
            .bearer_auth(&self.token)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Failed to open stream: {} - {}", status, body).into());
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let chunk = String::from_utf8_lossy(&chunk);
            if chunk.contains('\r') {
                buffer.push_str(&chunk.replace("\r\n", "\n"));
            } else {
                buffer.push_str(&chunk);
            }

            while let Some(idx) = buffer.find("\n\n") {
                let raw_event = buffer[..idx].to_string();
                buffer = buffer[idx + 2..].to_string();

                if let Some(data) = extract_sse_data(&raw_event) {
                    if let Ok(event) = serde_json::from_str::<StreamEvent>(&data) {
                        on_event(event).await;
                    }
                }
            }
        }

        Ok(())
    }
}

fn normalize_base_url(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn extract_sse_data(raw: &str) -> Option<String> {
    let mut data_lines = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

fn read_token_file() -> ClientResult<String> {
    let path = token_path();
    let token = std::fs::read_to_string(&path)?;
    Ok(token.trim().to_string())
}

fn token_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".tars").join("server.token");
    }

    PathBuf::from("tars.token")
}
