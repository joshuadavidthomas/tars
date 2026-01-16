use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use super::ToolDefinition;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ListFilesInput {
    #[schemars(description = "Optional relative path to list files from. Defaults to current directory if not provided.")]
    #[serde(default)]
    path: String,
}

async fn list_files_impl(
    input: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_files",
        description: "List files and directories at a given path. If no path is provided, lists files in the current directory.",
        input_schema: serde_json::to_value(schema_for!(ListFilesInput)).unwrap(),
        handler: |input| Box::pin(list_files_impl(input)),
    }
}
