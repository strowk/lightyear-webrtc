mod storage;

use axum::{
    Router,
    extract::{Query, State, WebSocketUpgrade, ws},
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use storage::RoomStorage;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    storage: RoomStorage,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "signaling_server=debug,tower_http=info".parse().unwrap()),
        )
        .init();

    let state = AppState {
        storage: RoomStorage::new(),
    };

    let app = Router::new()
        .route("/rooms", get(list_rooms))
        .route("/host", get(host_ws))
        .route("/join", get(join_ws))
        .layer(CorsLayer::very_permissive())
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3536").await.unwrap();
    tracing::info!("Signaling server listening on http://0.0.0.0:3536");
    axum::serve(listener, app).await.unwrap();
}

// --- REST endpoint: list public rooms ---

#[derive(Deserialize)]
struct ListQuery {
    search: Option<String>,
}

async fn list_rooms(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let rooms = state.storage.list_public(query.search.as_deref()).await;
    axum::Json(rooms)
}

// --- WebSocket: Host creates a room and waits for a guest ---
//
// Protocol (matching SignalingClient trait):
//   1. Host connects via WS with room params in query string
//   2. Server creates room, sends host: {"type":"room_created","code":"...","room_id":"..."}
//   3. Host sends its SDP offer: {"type":"offer","offer":"..."}
//   4. Server stores the offer in the room
//   5. When a guest connects and retrieves the offer, then submits an answer...
//   6. Server relays the answer to host: {"type":"answer","answer":"..."}
//   7. Done - host's publish_offer() resolves with the answer

#[derive(Deserialize)]
struct HostQuery {
    name: String,
    public: Option<bool>,
    password: Option<String>,
}

/// Messages from server to host.
#[derive(Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum HostServerMsg {
    #[serde(rename = "room_created")]
    RoomCreated { code: String, room_id: String },
    #[serde(rename = "answer")]
    Answer { answer: String },
    #[serde(rename = "error")]
    Error { message: String },
}

/// Messages from host to server.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum HostClientMsg {
    #[serde(rename = "offer")]
    Offer { offer: String },
}

async fn host_ws(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HostQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_host(socket, state, query))
}

async fn handle_host(socket: ws::WebSocket, state: Arc<AppState>, query: HostQuery) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let is_public = query.public.unwrap_or(true);
    let (room, mut answer_rx) = state
        .storage
        .create_room(query.name.clone(), is_public, query.password.clone())
        .await;

    tracing::info!(
        room_id = %room.id,
        name = %room.name,
        code = %room.code,
        public = is_public,
        "Room created"
    );

    // Tell host their room code
    let msg = serde_json::to_string(&HostServerMsg::RoomCreated {
        code: room.code.clone(),
        room_id: room.id.to_string(),
    })
    .unwrap();
    if ws_tx.send(ws::Message::text(msg)).await.is_err() {
        state.storage.remove_room(room.id).await;
        return;
    }

    let room_id = room.id;

    // Wait for host's offer
    let offer = loop {
        match ws_rx.next().await {
            Some(Ok(ws::Message::Text(text))) => {
                if let Ok(HostClientMsg::Offer { offer }) = serde_json::from_str(&text) {
                    break offer;
                }
            }
            Some(Ok(_)) => continue,
            _ => {
                state.storage.remove_room(room_id).await;
                return;
            }
        }
    };

    // Store the offer so guests can retrieve it
    state.storage.set_offer(room_id, offer).await;

    // Wait for a guest's answer (relayed via storage channel)
    let answer = tokio::select! {
        Some(answer) = answer_rx.recv() => answer,
        _ = wait_for_disconnect(&mut ws_rx) => {
            state.storage.remove_room(room_id).await;
            return;
        }
    };

    // Send answer to host
    let msg = serde_json::to_string(&HostServerMsg::Answer { answer }).unwrap();
    let _ = ws_tx.send(ws::Message::text(msg)).await;

    tracing::info!(room_id = %room_id, "SDP exchange complete");

    // Keep connection alive briefly for the message to flush
    let _ = ws_rx.next().await;
    state.storage.remove_room(room_id).await;
}

async fn wait_for_disconnect(ws_rx: &mut futures::stream::SplitStream<ws::WebSocket>) {
    while let Some(Ok(_)) = ws_rx.next().await {}
}

// --- WebSocket: Guest joins a room ---
//
// Protocol (matching SignalingClient trait):
//   1. Guest connects via WS with room_id or code (+ optional password) in query
//   2. Server validates room + password
//   3. Server sends guest the host's offer: {"type":"offer","offer":"..."}
//      (If host hasn't sent offer yet, server waits)
//   4. Guest sends its SDP answer: {"type":"answer","answer":"..."}
//   5. Server relays the answer to the host
//   6. Done - guest's submit_answer() resolves

#[derive(Deserialize)]
struct JoinQuery {
    room_id: Option<String>,
    code: Option<String>,
    password: Option<String>,
}

/// Messages from server to guest.
#[derive(Serialize)]
#[serde(tag = "type")]
enum GuestServerMsg {
    #[serde(rename = "offer")]
    Offer { offer: String },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "error")]
    Error { message: String },
}

/// Messages from guest to server.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum GuestClientMsg {
    #[serde(rename = "answer")]
    Answer { answer: String },
}

async fn join_ws(
    State(state): State<Arc<AppState>>,
    Query(query): Query<JoinQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_join(socket, state, query))
}

async fn handle_join(socket: ws::WebSocket, state: Arc<AppState>, query: JoinQuery) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Find the room
    let room = if let Some(ref room_id_str) = query.room_id {
        let Ok(room_id) = Uuid::parse_str(room_id_str) else {
            send_error(&mut ws_tx, "Invalid room ID").await;
            return;
        };
        state.storage.find_by_id(room_id).await
    } else if let Some(ref code) = query.code {
        state.storage.find_by_code(code).await
    } else {
        send_error(&mut ws_tx, "Must provide room_id or code").await;
        return;
    };

    let Some(room) = room else {
        send_error(&mut ws_tx, "Room not found").await;
        return;
    };

    // Check password
    if let Some(ref expected_pw) = room.password {
        match &query.password {
            Some(pw) if pw == expected_pw => {}
            _ => {
                send_error(&mut ws_tx, "Invalid password").await;
                return;
            }
        }
    }

    // Get the host's offer (wait if not ready yet)
    let Some(offer) = state.storage.wait_for_offer(room.id).await else {
        send_error(&mut ws_tx, "Host disconnected before sending offer").await;
        return;
    };

    // Send offer to guest
    let msg = serde_json::to_string(&GuestServerMsg::Offer { offer }).unwrap();
    if ws_tx.send(ws::Message::text(msg)).await.is_err() {
        return;
    }

    // Wait for guest's answer
    let answer = loop {
        match ws_rx.next().await {
            Some(Ok(ws::Message::Text(text))) => {
                if let Ok(GuestClientMsg::Answer { answer }) = serde_json::from_str(&text) {
                    break answer;
                }
            }
            Some(Ok(_)) => continue,
            _ => return,
        }
    };

    // Relay answer to host
    state.storage.submit_answer(room.id, answer).await;

    // Confirm to guest
    let msg = serde_json::to_string(&GuestServerMsg::Done).unwrap();
    let _ = ws_tx.send(ws::Message::text(msg)).await;

    tracing::info!(room_id = %room.id, "Guest joined successfully");
}

async fn send_error(
    ws_tx: &mut futures::stream::SplitSink<ws::WebSocket, ws::Message>,
    message: &str,
) {
    let msg = serde_json::to_string(&GuestServerMsg::Error {
        message: message.to_string(),
    })
    .unwrap();
    let _ = ws_tx.send(ws::Message::text(msg)).await;
}
