mod ui;

use reqwest::Client;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;

// ============================================================================
// Data Structures for Anthropic API
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
struct MessageRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<MessageParam>,
    tools: Vec<ToolDefinitionApi>,
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
    fn new(content: Vec<ContentBlock>) -> Self {
        Self { role: "user".to_string(), content }
    }

    fn from_text(text: String) -> Self {
        Self::new(vec![ContentBlock::Text { text }])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    role: String,
    content: Vec<ContentBlock>,
}

impl AssistantMessage {
    fn new(content: Vec<ContentBlock>) -> Self {
        Self { role: "assistant".to_string(), content }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, #[serde(skip_serializing_if = "Option::is_none")] is_error: Option<bool> },
}

impl ContentBlock {
    fn text(text: String) -> Self {
        Self::Text { text }
    }

    fn tool_result(tool_use_id: String, content: String, is_error: bool) -> Self {
        Self::ToolResult { tool_use_id, content, is_error: if is_error { Some(true) } else { None } }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MessageResponse {
    id: String,
    content: Vec<ResponseContentBlock>,
    stop_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolDefinitionApi {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// ============================================================================
// Tool Definitions
// ============================================================================

type ToolHandler = fn(serde_json::Value) -> Pin<Box<dyn Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    handler: ToolHandler,
}

// read_file tool
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ReadFileInput {
    #[schemars(description = "The relative path of a file in the working directory.")]
    path: String,
}

async fn read_file_impl(input: serde_json::Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let input: ReadFileInput = serde_json::from_value(input)?;
    tokio::fs::read_to_string(&input.path)
        .await
        .map_err(|e| format!("Error reading file: {}", e).into())
}

// list_files tool
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ListFilesInput {
    #[schemars(description = "Optional relative path to list files from. Defaults to current directory if not provided.")]
    #[serde(default)]
    path: String,
}

async fn list_files_impl(input: serde_json::Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let input: ListFilesInput = serde_json::from_value(input)?;
    let dir = if input.path.is_empty() { "." } else { &input.path };

    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.file_name();
        let path_str = path.to_string_lossy().to_string();

        if entry.file_type().await?.is_dir() {
            files.push(format!("{}/", path_str));
        } else {
            files.push(path_str);
        }
    }

    files.sort();
    serde_json::to_string(&files).map_err(|e| e.into())
}

// edit_file tool
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct EditFileInput {
    #[schemars(description = "The path to the file")]
    path: String,
    #[schemars(description = "Text to search for - must match exactly and must only have one match exactly")]
    old_str: String,
    #[schemars(description = "Text to replace old_str with")]
    new_str: String,
}

async fn edit_file_impl(input: serde_json::Value) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let input: EditFileInput = serde_json::from_value(input)?;

    if input.path.is_empty() || input.old_str == input.new_str {
        return Err("Invalid input parameters".into());
    }

    match tokio::fs::read_to_string(&input.path).await {
        Ok(content) => {
            let new_content = content.replace(&input.old_str, &input.new_str);

            if !content.contains(&input.old_str) && !input.old_str.is_empty() {
                return Err("old_str not found in file".into());
            }

            tokio::fs::write(&input.path, new_content).await?;
            Ok("OK".to_string())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if input.old_str.is_empty() {
                if let Some(parent) = std::path::Path::new(&input.path).parent() {
                    if !parent.as_os_str().is_empty() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                }
                tokio::fs::write(&input.path, &input.new_str).await?;
                Ok(format!("Successfully created file {}", input.path))
            } else {
                Err(e.into())
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn get_all_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "read_file",
            description: "Read the contents of a given relative file path. Use this when you want to see what's inside a file. Do not use this with directory names.",
            input_schema: serde_json::to_value(schema_for!(ReadFileInput)).unwrap(),
            handler: |input| Box::pin(read_file_impl(input)),
        },
        ToolDefinition {
            name: "list_files",
            description: "List files and directories at a given path. If no path is provided, lists files in the current directory.",
            input_schema: serde_json::to_value(schema_for!(ListFilesInput)).unwrap(),
            handler: |input| Box::pin(list_files_impl(input)),
        },
        ToolDefinition {
            name: "edit_file",
            description: "Make edits to a text file.\n\nReplaces 'old_str' with 'new_str' in the given file. 'old_str' and 'new_str' MUST be different from each other.\n\nIf the file specified with path doesn't exist, it will be created.",
            input_schema: serde_json::to_value(schema_for!(EditFileInput)).unwrap(),
            handler: |input| Box::pin(edit_file_impl(input)),
        },
    ]
}

// ============================================================================
// Agent
// ============================================================================

pub struct Agent {
    client: Client,
    api_key: String,
    tools: Vec<ToolDefinition>,
}

impl Agent {
    pub fn new(api_key: String) -> Self {
        let client = Client::new();
        let tools = get_all_tools();
        Self { client, api_key, tools }
    }

    pub async fn run_inference(&self, conversation: &Vec<MessageParam>) -> Result<MessageResponse, Box<dyn std::error::Error + Send + Sync>> {
        let tools_api: Vec<ToolDefinitionApi> = self.tools.iter().map(|t| ToolDefinitionApi {
            name: t.name.to_string(),
            description: t.description.to_string(),
            input_schema: t.input_schema.clone(),
        }).collect();

        let request = MessageRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 4096,
            messages: conversation.clone(),
            tools: tools_api,
        };

        let response = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await?;
            return Err(format!("API error: {} - {}", status, error_text).into());
        }

        response.json().await.map_err(|e| e.into())
    }

    pub async fn execute_tool(&self, id: String, name: String, input: serde_json::Value) -> ContentBlock {
        let tool_def = self.tools.iter().find(|t| t.name == name);

        match tool_def {
            Some(tool) => {
                match (tool.handler)(input).await {
                    Ok(result) => ContentBlock::tool_result(id, result, false),
                    Err(e) => ContentBlock::tool_result(id, e.to_string(), true),
                }
            }
            None => ContentBlock::tool_result(id, "tool not found".to_string(), true),
        }
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable not set");

    let agent = Agent::new(api_key);

    ui::run_tui(agent)
}
