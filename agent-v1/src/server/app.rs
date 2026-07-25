use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::{Any, CorsLayer};

use crate::api::{
    AgentConfig, AgentMode, AgentSession, EditPolicy, SessionSource,
};

use super::static_files::INDEX_HTML;

#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub workspace: PathBuf,
    pub mode: AgentMode,
    pub edit_policy: EditPolicy,
    pub port: u16,
}

struct LiveAgent {
    session: AgentSession,
    mode: AgentMode,
    edit_policy: EditPolicy,
    model: String,
    busy: bool,
}

struct AppState {
    options: ServeOptions,
    agent: Mutex<LiveAgent>,
    /// JSON-serialized [`crate::api::AgentEvent`] lines for WebSocket clients.
    events: broadcast::Sender<String>,
}

pub async fn build_router(options: ServeOptions) -> Result<Router> {
    let (events, _) = broadcast::channel::<String>(256);
    let (session, model) = open_session(&options, options.mode, options.edit_policy)?;
    spawn_forwarder(session.subscribe()?, events.clone());

    let state = Arc::new(AppState {
        options: options.clone(),
        agent: Mutex::new(LiveAgent {
            session,
            mode: options.mode,
            edit_policy: options.edit_policy,
            model,
            busy: false,
        }),
        events,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Ok(Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/message", post(send_message))
        .route("/api/approval", post(approval))
        .route("/api/mode", post(set_mode))
        .route("/api/stop", post(stop))
        .route("/ws", get(ws_upgrade))
        .layer(cors)
        .with_state(state))
}

fn open_session(
    options: &ServeOptions,
    mode: AgentMode,
    edit_policy: EditPolicy,
) -> Result<(AgentSession, String)> {
    let config = AgentConfig::builder()
        .workspace(&options.workspace)
        .mode(mode)
        .edit_policy(edit_policy)
        .session(SessionSource::Persistent)
        .build()
        .context("invalid agent config")?;
    let model = config.model().to_string();
    let session = AgentSession::open(config).context("open session")?;
    Ok((session, model))
}

fn spawn_forwarder(mut stream: crate::api::EventStream, events: broadcast::Sender<String>) {
    tokio::spawn(async move {
        while let Some(event) = stream.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = events.send(json);
            }
        }
    });
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Serialize)]
struct StatusResponse {
    workspace: String,
    mode: String,
    edit_policy: String,
    model: String,
    session_id: String,
    busy: bool,
    stop_requested: bool,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let agent = state.agent.lock().await;
    Json(StatusResponse {
        workspace: state.options.workspace.display().to_string(),
        mode: agent.mode.as_str().to_string(),
        edit_policy: agent.edit_policy.as_str().to_string(),
        model: agent.model.clone(),
        session_id: agent.session.session_id().to_string(),
        busy: agent.busy,
        stop_requested: agent.session.stop_requested(),
    })
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Json(body): Json<MessageBody>,
) -> Response {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return (StatusCode::BAD_REQUEST, "message is empty").into_response();
    }

    let mut agent = state.agent.lock().await;
    if agent.busy {
        return (StatusCode::CONFLICT, "a turn is already running").into_response();
    }
    agent.busy = true;
    if let Err(e) = agent.session.send(text).await {
        agent.busy = false;
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    let events = state.events.subscribe();
    let state_busy = Arc::clone(&state);
    tokio::spawn(async move {
        let mut events = events;
        loop {
            match events.recv().await {
                Ok(json) => {
                    if json.contains("\"type\":\"finished\"") {
                        let mut agent = state_busy.agent.lock().await;
                        agent.busy = false;
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
struct ApprovalBody {
    id: String,
    /// `once` | `task` | `reject`
    decision: String,
}

async fn approval(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ApprovalBody>,
) -> Response {
    let agent = state.agent.lock().await;
    let result = match body.decision.trim().to_ascii_lowercase().as_str() {
        "once" | "approve" | "approve_once" => agent.session.approve(body.id).await,
        "task" | "approve_for_task" | "approve-task" => {
            agent.session.approve_for_task(body.id).await
        }
        "reject" | "deny" => agent.session.reject(body.id).await,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unknown decision '{other}' (use once|task|reject)"),
            )
                .into_response();
        }
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
    edit_policy: Option<String>,
}

async fn set_mode(State(state): State<Arc<AppState>>, Json(body): Json<ModeBody>) -> Response {
    let mode = match AgentMode::parse(&body.mode) {
        Some(m) => m,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "mode must be ask or agent".to_string(),
            )
                .into_response();
        }
    };
    let edit_policy = match mode {
        AgentMode::Ask => EditPolicy::ReadOnly,
        AgentMode::Agent => body
            .edit_policy
            .as_deref()
            .and_then(EditPolicy::parse)
            .unwrap_or(state.options.edit_policy),
    };

    let mut agent = state.agent.lock().await;
    if agent.busy {
        return (
            StatusCode::CONFLICT,
            "cannot change mode while a turn is running",
        )
            .into_response();
    }
    if agent.mode == mode && agent.edit_policy == edit_policy {
        return StatusCode::NO_CONTENT.into_response();
    }

    if let Err(e) = agent.session.shutdown() {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    match open_session(&state.options, mode, edit_policy) {
        Ok((session, model)) => {
            match session.subscribe() {
                Ok(stream) => spawn_forwarder(stream, state.events.clone()),
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
            *agent = LiveAgent {
                session,
                mode,
                edit_policy,
                model,
                busy: false,
            };
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn stop(State(state): State<Arc<AppState>>) -> Response {
    let agent = state.agent.lock().await;
    agent.session.request_stop();
    StatusCode::NO_CONTENT.into_response()
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| ws_client(socket, state))
}

async fn ws_client(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.events.subscribe();
    let (workspace, model) = {
        let agent = state.agent.lock().await;
        (
            state.options.workspace.display().to_string(),
            agent.model.clone(),
        )
    };
    let hello = serde_json::json!({
        "type": "server_hello",
        "workspace": workspace,
        "model": model,
    });
    if socket
        .send(Message::Text(hello.to_string()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        let _ = socket.send(Message::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(json) => {
                        if socket.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
