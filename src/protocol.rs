use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionCreateResponse {
    pub session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Assistant { text: String },
    ToolCall {
        name: String,
        input: serde_json::Value,
    },
    ToolResult { content: String, is_error: bool },
    Info { message: String },
    Error { message: String },
    Done,
}
