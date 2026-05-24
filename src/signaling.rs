use alloc::string::String;
use core::future::Future;
use core::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(thiserror::Error, Debug)]
pub enum SignalingError {
    #[error("signaling failed: {0}")]
    Failed(String),
}

/// Pluggable signaling interface for WebRTC offer/answer exchange.
///
/// Implementations handle how SDP offers and answers are delivered between peers.
/// Examples: WebSocket relay, QR codes, manual copy-paste, Firebase, etc.
///
/// Must be `Send + Sync` to be stored in a Bevy Component.
/// Implementations that use `!Send` types (like `web_sys`) internally should
/// communicate with `spawn_local` tasks via channels.
pub trait SignalingClient: Send + Sync + 'static {
    /// Host calls this to publish its SDP offer.
    /// Should block (async) until the remote answer is received, then return it.
    fn publish_offer(&mut self, offer: String) -> BoxFuture<'_, Result<String, SignalingError>>;

    /// Guest calls this to retrieve the pending SDP offer from the host.
    fn retrieve_offer(&mut self) -> BoxFuture<'_, Result<String, SignalingError>>;

    /// Guest calls this to send its SDP answer back to the host.
    fn submit_answer(&mut self, answer: String) -> BoxFuture<'_, Result<(), SignalingError>>;
}
