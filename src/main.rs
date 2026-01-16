mod agent;
mod ai_sdk;
mod tools;
mod ui;

use agent::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable not set");

    let agent = Agent::new(api_key);

    ui::run_tui(agent)
}
