use crate::agent::Agent;
use crate::ai_sdk::{
    assistant_content_from_response, AssistantMessage, ContentBlock, MessageParam,
    ResponseContentBlock, UserMessage,
};
use crate::protocol::{SendMessageRequest, SessionCreateResponse, StreamEvent};
use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use futures::StreamExt;
use std::collections::HashMap;
use std::convert::Infallible;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

pub struct ServerConfig {
    pub listen: String,
    pub auth_token: String,
}

struct ServerState {
    agent: Arc<Agent>,
    sessions: Mutex<HashMap<String, Arc<SessionState>>>,
    auth_token: String,
}

struct SessionState {
    conversation: Mutex<Vec<MessageParam>>,
    events: broadcast::Sender<StreamEvent>,
    running: Mutex<bool>,
}

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub async fn run(config: ServerConfig) -> ServerResult<()> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable not set")?;

    let state = Arc::new(ServerState {
        agent: Arc::new(Agent::new(api_key)),
        sessions: Mutex::new(HashMap::new()),
        auth_token: config.auth_token,
    });

    let app = axum::Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/:id/messages", post(send_message))
        .route("/sessions/:id/stream", get(stream_session))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    println!("tars server listening on http://{}", config.listen);
    println!("auth token stored at {}", token_path().display());
    axum::serve(listener, app).await?;

    Ok(())
}

pub fn resolve_token(explicit: Option<String>) -> ServerResult<String> {
    if let Some(token) = explicit {
        write_token_file(&token)?;
        return Ok(token);
    }

    if let Ok(token) = read_token_file() {
        return Ok(token);
    }

    let token = Uuid::new_v4().to_string();
    write_token_file(&token)?;
    Ok(token)
}

async fn create_session(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<SessionCreateResponse>, StatusCode> {
    authorize(&headers, &state.auth_token)?;

    let session_id = Uuid::new_v4().to_string();
    let (events, _) = broadcast::channel(200);
    let session = Arc::new(SessionState {
        conversation: Mutex::new(Vec::new()),
        events,
        running: Mutex::new(false),
    });

    state
        .sessions
        .lock()
        .await
        .insert(session_id.clone(), session);

    Ok(Json(SessionCreateResponse { session_id }))
}

async fn send_message(
    State(state): State<Arc<ServerState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<SendMessageRequest>,
) -> Result<StatusCode, StatusCode> {
    authorize(&headers, &state.auth_token)?;

    let session = {
        let sessions = state.sessions.lock().await;
        sessions.get(&session_id).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;

    {
        let mut running = session.running.lock().await;
        if *running {
            return Err(StatusCode::CONFLICT);
        }
        *running = true;
    }

    let agent = Arc::clone(&state.agent);
    let session_clone = Arc::clone(&session);
    let message = payload.content;
    tokio::spawn(async move {
        let result = run_agent_loop(agent, session_clone, message).await;
        if let Err(err) = result {
            let _ = session.events.send(StreamEvent::Error {
                message: err.to_string(),
            });
        }
        let _ = session.events.send(StreamEvent::Done);
        let mut running = session.running.lock().await;
        *running = false;
    });

    Ok(StatusCode::ACCEPTED)
}

async fn stream_session(
    State(state): State<Arc<ServerState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    authorize(&headers, &state.auth_token)?;

    let session = {
        let sessions = state.sessions.lock().await;
        sessions.get(&session_id).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;

    let stream = BroadcastStream::new(session.events.subscribe()).filter_map(|item| async move {
        match item {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok::<Event, Infallible>(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

async fn run_agent_loop(
    agent: Arc<Agent>,
    session: Arc<SessionState>,
    message: String,
) -> ServerResult<()> {
    {
        let mut conversation = session.conversation.lock().await;
        conversation.push(MessageParam::User(UserMessage::from_text(message)));
    }

    loop {
        let conversation = { session.conversation.lock().await.clone() };
        let response = agent.run_inference(conversation.as_slice()).await?;
        let mut tool_results: Vec<ContentBlock> = Vec::new();

        for content in &response.content {
            match content {
                ResponseContentBlock::Text { text } => {
                    let _ = session.events.send(StreamEvent::Assistant { text: text.clone() });
                }
                ResponseContentBlock::ToolUse { id, name, input } => {
                    let _ = session.events.send(StreamEvent::ToolCall {
                        name: name.clone(),
                        input: input.clone(),
                    });

                    let result = agent
                        .execute_tool(id.clone(), name.clone(), input.clone())
                        .await;

                    let (content, is_error) = match &result {
                        ContentBlock::ToolResult {
                            content,
                            is_error,
                            ..
                        } => (content.clone(), is_error.unwrap_or(false)),
                        _ => (String::new(), false),
                    };

                    let _ = session.events.send(StreamEvent::ToolResult { content, is_error });
                    tool_results.push(result);
                }
            }
        }

        let assistant_content = assistant_content_from_response(&response);
        {
            let mut conversation = session.conversation.lock().await;
            conversation.push(MessageParam::Assistant(AssistantMessage::new(
                assistant_content,
            )));
            if !tool_results.is_empty() {
                conversation.push(MessageParam::User(UserMessage::new(tool_results.clone())));
            }
        }

        if tool_results.is_empty() {
            break;
        }
    }

    Ok(())
}

fn authorize(headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    match header {
        Some(value) if value == format!("Bearer {}", token) => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn read_token_file() -> ServerResult<String> {
    let token = std::fs::read_to_string(token_path())?;
    Ok(token.trim().to_string())
}

fn write_token_file(token: &str) -> ServerResult<()> {
    let path = token_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(&path)?;
    use std::io::Write;
    file.write_all(token.as_bytes())?;
    Ok(())
}

fn token_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".tars").join("server.token");
    }

    PathBuf::from("tars.token")
}
