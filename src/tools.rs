use std::future::Future;
use std::pin::Pin;

mod edit_file;
mod list_files;
mod read_file;

type ToolHandler = fn(
    serde_json::Value,
) -> Pin<
    Box<dyn Future<Output = Result<String, Box<dyn std::error::Error + Send + Sync>>> + Send>,
>;

pub(crate) struct ToolDefinition {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) input_schema: serde_json::Value,
    pub(crate) handler: ToolHandler,
}

pub(crate) fn get_all_tools() -> Vec<ToolDefinition> {
    vec![
        read_file::definition(),
        list_files::definition(),
        edit_file::definition(),
    ]
}
