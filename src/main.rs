mod agent;
mod ai_sdk;
mod client;
mod protocol;
mod server;
mod tools;
mod ui;

use clap::{Args, Parser, Subcommand};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "tars",
    version,
    about = "Terminal-based agent",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    client: ClientArgs,
}

#[derive(Subcommand)]
enum Command {
    Server(ServerArgs),
}

#[derive(Args, Clone)]
struct ClientArgs {
    #[arg(long)]
    server: Option<String>,
    #[arg(long)]
    token: Option<String>,
}

#[derive(Args)]
struct ServerArgs {
    #[arg(long, env = "TARS_LISTEN", default_value = "127.0.0.1:7331")]
    listen: String,
    #[arg(long, env = "TARS_TOKEN")]
    token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Server(args)) => {
            let auth_token = server::resolve_token(args.token)?;
            server::run(server::ServerConfig {
                listen: args.listen,
                auth_token,
            })
            .await
        }
        None => {
            let base_url = cli
                .client
                .server
                .or_else(|| std::env::var("TARS_SERVER").ok())
                .unwrap_or_else(|| "http://127.0.0.1:7331".to_string());

            let token = cli.client.token.or_else(|| std::env::var("TARS_TOKEN").ok());
            let mut auth_token = token.clone();

            if let Some(host_port) = host_port_from_base_url(&base_url) {
                if is_local_http(&base_url) && !is_server_reachable(&host_port).await {
                    let api_key_set = std::env::var("ANTHROPIC_API_KEY").is_ok();
                    if !api_key_set {
                        return Err(
                            "ANTHROPIC_API_KEY environment variable not set; cannot start server"
                                .into(),
                        );
                    }
                    let server_token = server::resolve_token(token)?;
                    spawn_server(host_port.clone(), server_token.clone());
                    wait_for_server(&host_port).await?;
                    auth_token = Some(server_token);
                }
            }

            let auth_token = match auth_token {
                Some(token) => token,
                None => client::resolve_token(None)?,
            };
            let session = client::ClientSession::connect(client::ClientConfig {
                base_url,
                token: auth_token,
            })
            .await?;
            ui::run_tui(session)
        }
    }
}

fn host_port_from_base_url(base_url: &str) -> Option<String> {
    let base = base_url.trim();
    let without_scheme = base
        .strip_prefix("http://")
        .or_else(|| base.strip_prefix("https://"))?;
    let host_port = without_scheme.split('/').next()?.trim();
    if host_port.is_empty() {
        None
    } else {
        Some(ensure_port(host_port))
    }
}

fn ensure_port(host_port: &str) -> String {
    if host_port.starts_with('[') {
        if host_port.contains("]:") {
            host_port.to_string()
        } else {
            format!("{}:7331", host_port)
        }
    } else if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{}:7331", host_port)
    }
}

fn is_local_http(base_url: &str) -> bool {
    let base = base_url.trim();
    let without_scheme = match base.strip_prefix("http://") {
        Some(rest) => rest,
        None => return false,
    };
    let host_port = without_scheme.split('/').next().unwrap_or("");
    host_port.starts_with("127.0.0.1")
        || host_port.starts_with("localhost")
        || host_port.starts_with("[::1]")
}

async fn is_server_reachable(host_port: &str) -> bool {
    tokio::net::TcpStream::connect(host_port).await.is_ok()
}

fn spawn_server(listen: String, token: String) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build();
        match runtime {
            Ok(rt) => {
                let result = rt.block_on(server::run(server::ServerConfig {
                    listen,
                    auth_token: token,
                }));
                if let Err(err) = result {
                    eprintln!("tars server stopped: {}", err);
                }
            }
            Err(err) => {
                eprintln!("failed to start server runtime: {}", err);
            }
        }
    });
}

async fn wait_for_server(host_port: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for _ in 0..20 {
        if is_server_reachable(host_port).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err("Server did not start listening in time".into())
}
