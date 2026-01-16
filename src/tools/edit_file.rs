use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use std::io;

use super::ToolDefinition;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct EditFileInput {
    #[schemars(description = "The path to the file")]
    path: String,
    #[schemars(description = "Text to search for - must match exactly and must only have one match exactly")]
    old_str: String,
    #[schemars(description = "Text to replace old_str with")]
    new_str: String,
}

async fn edit_file_impl(
    input: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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

pub(crate) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "edit_file",
        description: "Make edits to a text file.\n\nReplaces 'old_str' with 'new_str' in the given file. 'old_str' and 'new_str' MUST be different from each other.\n\nIf the file specified with path doesn't exist, it will be created.",
        input_schema: serde_json::to_value(schema_for!(EditFileInput)).unwrap(),
        handler: |input| Box::pin(edit_file_impl(input)),
    }
}
