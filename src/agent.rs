use crate::ai_sdk::{
    ContentBlock, MessageParam, MessageRequest, MessageResponse, ToolDefinitionApi,
};
use crate::tools::{get_all_tools, ToolDefinition};
use reqwest::Client;

pub struct Agent {
    client: Client,
    api_key: String,
    tools: Vec<ToolDefinition>,
}

impl Agent {
    pub fn new(api_key: String) -> Self {
        let client = Client::new();
        let tools = get_all_tools();
        Self {
            client,
            api_key,
            tools,
        }
    }

    pub(crate) async fn run_inference(
        &self,
        conversation: &[MessageParam],
    ) -> Result<MessageResponse, Box<dyn std::error::Error + Send + Sync>> {
        let tools_api: Vec<ToolDefinitionApi> = self
            .tools
            .iter()
            .map(|t| ToolDefinitionApi {
                name: t.name.to_string(),
                description: t.description.to_string(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        let request = MessageRequest {
            model: "claude-haiku-4-5-20251001".to_string(),
            max_tokens: 4096,
            messages: conversation.to_vec(),
            tools: tools_api,
        };

        let response = self
            .client
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

    pub(crate) async fn execute_tool(
        &self,
        id: String,
        name: String,
        input: serde_json::Value,
    ) -> ContentBlock {
        let tool_def = self.tools.iter().find(|t| t.name == name);

        match tool_def {
            Some(tool) => match (tool.handler)(input).await {
                Ok(result) => ContentBlock::tool_result(id, result, false),
                Err(e) => ContentBlock::tool_result(id, e.to_string(), true),
            },
            None => ContentBlock::tool_result(id, "tool not found".to_string(), true),
        }
    }
}
