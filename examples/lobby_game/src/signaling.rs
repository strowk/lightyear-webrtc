use futures_channel::oneshot;
use lightyear_webrtc::signaling::{BoxFuture, SignalingClient, SignalingError};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, WebSocket};

extern crate alloc;
use alloc::format;
use alloc::string::String;
use std::sync::{Arc, Mutex};

// --- Protocol messages matching the signaling server ---

#[derive(Deserialize)]
#[serde(tag = "type")]
enum HostServerMsg {
    #[serde(rename = "room_created")]
    RoomCreated { code: String, room_id: String },
    #[serde(rename = "answer")]
    Answer { answer: String },
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum HostClientMsg {
    #[serde(rename = "offer")]
    Offer { offer: String },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum GuestServerMsg {
    #[serde(rename = "offer")]
    Offer { offer: String },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "error")]
    Error { message: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum GuestClientMsg {
    #[serde(rename = "answer")]
    Answer { answer: String },
}

/// WebSocket-based signaling client for the host.
///
/// Flow: connect -> receive room_created -> send offer -> wait for answer
pub struct HostWsSignaling {
    server_url: String,
    room_name: String,
    public: bool,
    password: Option<String>,
}

unsafe impl Send for HostWsSignaling {}
unsafe impl Sync for HostWsSignaling {}

impl HostWsSignaling {
    pub fn new(server_url: String, room_name: String, public: bool, password: Option<String>) -> Self {
        Self {
            server_url,
            room_name,
            public,
            password,
        }
    }

    fn build_url(&self) -> String {
        let mut url = format!(
            "{}/host?name={}&public={}",
            self.server_url,
            js_sys::encode_uri_component(&self.room_name),
            self.public
        );
        if let Some(ref pw) = self.password {
            url.push_str(&format!("&password={}", js_sys::encode_uri_component(pw)));
        }
        url
    }
}

impl SignalingClient for HostWsSignaling {
    fn publish_offer(&mut self, offer: String) -> BoxFuture<'_, Result<String, SignalingError>> {
        Box::pin(async move {
            let url = self.build_url();
            let ws = connect_ws(&url)?;

            // Wait for open
            wait_open(&ws).await?;

            // Wait for room_created
            let text = wait_message(&ws).await?;
            let msg: HostServerMsg = serde_json::from_str(&text)
                .map_err(|e| SignalingError::Failed(format!("Parse error: {e}")))?;

            match msg {
                HostServerMsg::RoomCreated { code, room_id } => {
                    show_room_code(&code);
                }
                HostServerMsg::Error { message } => {
                    return Err(SignalingError::Failed(message));
                }
                _ => return Err(SignalingError::Failed("Unexpected message".into())),
            }

            // Send our offer
            let cmd = serde_json::to_string(&HostClientMsg::Offer { offer }).unwrap();
            ws.send_with_str(&cmd)
                .map_err(|e| SignalingError::Failed(format!("Send failed: {e:?}")))?;

            // Wait for guest's answer
            let text = wait_message(&ws).await?;
            let msg: HostServerMsg = serde_json::from_str(&text)
                .map_err(|e| SignalingError::Failed(format!("Parse error: {e}")))?;

            match msg {
                HostServerMsg::Answer { answer } => {
                    ws.close().ok();
                    Ok(answer)
                }
                HostServerMsg::Error { message } => Err(SignalingError::Failed(message)),
                _ => Err(SignalingError::Failed("Unexpected message".into())),
            }
        })
    }

    fn retrieve_offer(&mut self) -> BoxFuture<'_, Result<String, SignalingError>> {
        // Host never calls retrieve_offer
        Box::pin(async { Err(SignalingError::Failed("Host does not retrieve offers".into())) })
    }

    fn submit_answer(&mut self, _answer: String) -> BoxFuture<'_, Result<(), SignalingError>> {
        // Host never calls submit_answer
        Box::pin(async { Err(SignalingError::Failed("Host does not submit answers".into())) })
    }
}

/// WebSocket-based signaling client for the guest.
///
/// Flow: connect -> receive offer -> (caller processes it) -> send answer
/// The WebSocket must stay open between retrieve_offer and submit_answer.
pub struct GuestWsSignaling {
    server_url: String,
    room_id: Option<String>,
    code: Option<String>,
    password: Option<String>,
    /// Stored WebSocket from retrieve_offer, used in submit_answer.
    ws: Option<WebSocket>,
}

unsafe impl Send for GuestWsSignaling {}
unsafe impl Sync for GuestWsSignaling {}

impl GuestWsSignaling {
    pub fn new(
        server_url: String,
        room_id: Option<String>,
        code: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self {
            server_url,
            room_id,
            code,
            password,
            ws: None,
        }
    }

    fn build_url(&self) -> String {
        let mut url = format!("{}/join?", self.server_url);
        if let Some(ref room_id) = self.room_id {
            url.push_str(&format!("room_id={}", room_id));
        } else if let Some(ref code) = self.code {
            url.push_str(&format!("code={}", js_sys::encode_uri_component(code)));
        }
        if let Some(ref pw) = self.password {
            url.push_str(&format!("&password={}", js_sys::encode_uri_component(pw)));
        }
        url
    }
}

impl SignalingClient for GuestWsSignaling {
    fn publish_offer(&mut self, _offer: String) -> BoxFuture<'_, Result<String, SignalingError>> {
        // Guest never calls publish_offer
        Box::pin(async { Err(SignalingError::Failed("Guest does not publish offers".into())) })
    }

    fn retrieve_offer(&mut self) -> BoxFuture<'_, Result<String, SignalingError>> {
        Box::pin(async move {
            let url = self.build_url();
            let ws = connect_ws(&url)?;

            wait_open(&ws).await?;

            // Wait for the offer from server
            let text = wait_message(&ws).await?;
            let msg: GuestServerMsg = serde_json::from_str(&text)
                .map_err(|e| SignalingError::Failed(format!("Parse error: {e}")))?;

            match msg {
                GuestServerMsg::Offer { offer } => {
                    // Keep WS alive for submit_answer
                    self.ws = Some(ws);
                    Ok(offer)
                }
                GuestServerMsg::Error { message } => Err(SignalingError::Failed(message)),
                _ => Err(SignalingError::Failed("Unexpected message".into())),
            }
        })
    }

    fn submit_answer(&mut self, answer: String) -> BoxFuture<'_, Result<(), SignalingError>> {
        Box::pin(async move {
            let ws = self
                .ws
                .take()
                .ok_or_else(|| SignalingError::Failed("No active connection".into()))?;

            let cmd = serde_json::to_string(&GuestClientMsg::Answer { answer }).unwrap();
            ws.send_with_str(&cmd)
                .map_err(|e| SignalingError::Failed(format!("Send failed: {e:?}")))?;

            // Wait for "done" confirmation
            let text = wait_message(&ws).await?;
            let msg: GuestServerMsg = serde_json::from_str(&text)
                .map_err(|e| SignalingError::Failed(format!("Parse error: {e}")))?;

            ws.close().ok();

            match msg {
                GuestServerMsg::Done => Ok(()),
                GuestServerMsg::Error { message } => Err(SignalingError::Failed(message)),
                _ => Err(SignalingError::Failed("Unexpected message".into())),
            }
        })
    }
}

// --- Helpers ---

fn connect_ws(url: &str) -> Result<WebSocket, SignalingError> {
    WebSocket::new(url).map_err(|e| SignalingError::Failed(format!("WebSocket connect failed: {e:?}")))
}

async fn wait_open(ws: &WebSocket) -> Result<(), SignalingError> {
    // If already open, return immediately
    if ws.ready_state() == WebSocket::OPEN {
        return Ok(());
    }

    let (tx, rx) = oneshot::channel::<Result<(), String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let tx_open = tx.clone();
    let on_open = Closure::<dyn FnMut()>::new(move || {
        if let Some(tx) = tx_open.lock().unwrap().take() {
            let _ = tx.send(Ok(()));
        }
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    let tx_err = tx.clone();
    let on_error = Closure::<dyn FnMut()>::new(move || {
        if let Some(tx) = tx_err.lock().unwrap().take() {
            let _ = tx.send(Err("WebSocket error".into()));
        }
    });
    ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    rx.await
        .map_err(|_| SignalingError::Failed("Connection cancelled".into()))?
        .map_err(SignalingError::Failed)
}

async fn wait_message(ws: &WebSocket) -> Result<String, SignalingError> {
    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let tx_msg = tx.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        if let Some(text) = event.data().as_string() {
            if let Some(tx) = tx_msg.lock().unwrap().take() {
                let _ = tx.send(Ok(text));
            }
        }
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let tx_close = tx.clone();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        if let Some(tx) = tx_close.lock().unwrap().take() {
            let _ = tx.send(Err("WebSocket closed".into()));
        }
    });
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

    rx.await
        .map_err(|_| SignalingError::Failed("Channel dropped".into()))?
        .map_err(SignalingError::Failed)
}

fn show_room_code(code: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("room-code") {
            el.set_inner_html(&format!("Room Code: <b>{code}</b>"));
        }
    }
}
