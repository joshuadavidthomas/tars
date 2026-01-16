use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use super::ToolDefinition;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ReadFileInput {
    #[schemars(description = "The relative path of a file in the working directory.")]
    path: String,
}

async fn read_file_impl(
    input: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let input: ReadFileInput = serde_json::from_value(input)?;
    tokio::fs::read_to_string(&input.path)
        .await
        .map_err(|e| format!("Error reading file: {}", e).into())
}

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file",
        description: "Read the contents of a given relative file path. Use this when you want to see what's inside a file. Do not use this with directory names.",
        input_schema: serde_json::to_value(schema_for!(ReadFileInput)).unwrap(),
        handler: |input| Box::pin(read_file_impl(input)),
    }
}
