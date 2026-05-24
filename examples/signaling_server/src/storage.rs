use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, mpsc};
use uuid::Uuid;

/// Represents a hosted game room.
#[derive(Debug, Clone)]
pub struct GameRoom {
    pub id: Uuid,
    /// Human-readable name chosen by the host.
    pub name: String,
    /// Whether the room appears in public listings.
    pub public: bool,
    /// Short alphanumeric code for private room joining.
    pub code: String,
    /// Optional password required to join.
    pub password: Option<String>,
}

/// Internal room state (not cloned to clients).
struct RoomState {
    room: GameRoom,
    /// The host's SDP offer, set once the host sends it.
    offer: Option<String>,
    /// Notified when the offer becomes available.
    offer_ready: Arc<Notify>,
    /// Channel to send guest's answer to the host handler.
    answer_tx: mpsc::Sender<String>,
}

/// In-memory storage for game rooms. Replace this module's internals
/// to back with a database or Redis if needed.
#[derive(Clone, Default)]
pub struct RoomStorage {
    rooms: Arc<Mutex<HashMap<Uuid, Arc<Mutex<RoomState>>>>>,
}

impl RoomStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_room(
        &self,
        name: String,
        public: bool,
        password: Option<String>,
    ) -> (GameRoom, mpsc::Receiver<String>) {
        let code = generate_code();
        let (answer_tx, answer_rx) = mpsc::channel(1);
        let room = GameRoom {
            id: Uuid::new_v4(),
            name,
            public,
            code,
            password,
        };
        let state = RoomState {
            room: room.clone(),
            offer: None,
            offer_ready: Arc::new(Notify::new()),
            answer_tx,
        };
        self.rooms
            .lock()
            .await
            .insert(room.id, Arc::new(Mutex::new(state)));
        (room, answer_rx)
    }

    pub async fn remove_room(&self, id: Uuid) {
        self.rooms.lock().await.remove(&id);
    }

    /// Store the host's SDP offer and notify waiting guests.
    pub async fn set_offer(&self, room_id: Uuid, offer: String) {
        let rooms = self.rooms.lock().await;
        if let Some(state_arc) = rooms.get(&room_id) {
            let mut state = state_arc.lock().await;
            state.offer = Some(offer);
            state.offer_ready.notify_waiters();
        }
    }

    /// Wait for the host's offer to be available, then return it.
    pub async fn wait_for_offer(&self, room_id: Uuid) -> Option<String> {
        let (notify, state_arc) = {
            let rooms = self.rooms.lock().await;
            let state_arc = rooms.get(&room_id)?.clone();
            let state = state_arc.lock().await;
            if let Some(ref offer) = state.offer {
                return Some(offer.clone());
            }
            (state.offer_ready.clone(), state_arc.clone())
        };

        notify.notified().await;

        let state = state_arc.lock().await;
        state.offer.clone()
    }

    /// Submit guest's answer - relays to host via channel.
    pub async fn submit_answer(&self, room_id: Uuid, answer: String) {
        let rooms = self.rooms.lock().await;
        if let Some(state_arc) = rooms.get(&room_id) {
            let state = state_arc.lock().await;
            let _ = state.answer_tx.send(answer).await;
        }
    }

    /// List public rooms. Optionally filter by name substring.
    pub async fn list_public(&self, search: Option<&str>) -> Vec<RoomInfo> {
        let rooms = self.rooms.lock().await;
        let mut result = Vec::new();
        for state_arc in rooms.values() {
            let state = state_arc.lock().await;
            let r = &state.room;
            if !r.public {
                continue;
            }
            if let Some(s) = search {
                if !r.name.to_lowercase().contains(&s.to_lowercase()) {
                    continue;
                }
            }
            result.push(RoomInfo {
                id: r.id,
                name: r.name.clone(),
                has_password: r.password.is_some(),
            });
        }
        result
    }

    /// Find a room by its short code (for private room joining).
    pub async fn find_by_code(&self, code: &str) -> Option<GameRoom> {
        let rooms = self.rooms.lock().await;
        for state_arc in rooms.values() {
            let state = state_arc.lock().await;
            if state.room.code == code {
                return Some(state.room.clone());
            }
        }
        None
    }

    /// Find a room by its ID.
    pub async fn find_by_id(&self, id: Uuid) -> Option<GameRoom> {
        let rooms = self.rooms.lock().await;
        if let Some(state_arc) = rooms.get(&id) {
            let state = state_arc.lock().await;
            return Some(state.room.clone());
        }
        None
    }
}

/// Public room info sent to clients (no secrets).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoomInfo {
    pub id: Uuid,
    pub name: String,
    pub has_password: bool,
}

fn generate_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let chars: Vec<char> = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".chars().collect();
    (0..6).map(|_| chars[rng.random_range(0..chars.len())]).collect()
}
