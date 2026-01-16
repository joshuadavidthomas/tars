use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MessageRequest {
    pub(crate) model: String,
    pub(crate) max_tokens: u32,
    pub(crate) messages: Vec<MessageParam>,
    pub(crate) tools: Vec<ToolDefinitionApi>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageParam {
    User(UserMessage),
    Assistant(AssistantMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    role: String,
    content: Vec<ContentBlock>,
}

impl UserMessage {
    pub(crate) fn new(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "user".to_string(),
            content,
        }
    }

    pub(crate) fn from_text(text: String) -> Self {
        Self::new(vec![ContentBlock::Text { text }])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    role: String,
    content: Vec<ContentBlock>,
}

impl AssistantMessage {
    pub(crate) fn new(content: Vec<ContentBlock>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

impl ContentBlock {
    pub(crate) fn tool_result(tool_use_id: String, content: String, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id,
            content,
            is_error: if is_error { Some(true) } else { None },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MessageResponse {
    pub(crate) id: String,
    pub(crate) content: Vec<ResponseContentBlock>,
    pub(crate) stop_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolDefinitionApi {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: serde_json::Value,
}

pub(crate) fn assistant_content_from_response(response: &MessageResponse) -> Vec<ContentBlock> {
    response
        .content
        .iter()
        .map(|content| match content {
            ResponseContentBlock::Text { text } => ContentBlock::Text { text: text.clone() },
            ResponseContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assistant_content_from_response_maps_blocks() {
        let response = MessageResponse {
            id: "msg_1".to_string(),
            stop_reason: "end".to_string(),
            content: vec![
                ResponseContentBlock::Text {
                    text: "hello".to_string(),
                },
                ResponseContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "README.md"}),
                },
            ],
        };

        let content = assistant_content_from_response(&response);
        assert_eq!(content.len(), 2);
        match &content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
        match &content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "tool_1");
                assert_eq!(name, "read_file");
                assert_eq!(input, &json!({"path": "README.md"}));
            }
            _ => panic!("expected tool use block"),
        }
    }
}
