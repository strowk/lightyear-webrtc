use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use bevy_ecs::prelude::*;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use futures_channel::mpsc;
use futures_core::Stream;

use crate::IceConfig;

/// Internal state communicated from async WebRTC task to Bevy systems.
pub(crate) enum WebRtcEvent {
    Connected,
    Disconnected(String),
    Error(String),
}

/// Component added to the entity after async connection setup begins.
/// Bridges between the async WebRTC DataChannel and Bevy's synchronous ECS.
#[derive(Component)]
pub struct WebRtcChannels {
    /// Incoming data from the remote peer's DataChannel.
    pub data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Outgoing data to send through the DataChannel.
    pub data_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Connection state events from the async task.
    pub(crate) event_rx: mpsc::UnboundedReceiver<WebRtcEvent>,
}

/// Holds a shared slot where the async WebRTC task will deposit
/// the channel handles once the connection is established.
#[derive(Component)]
pub struct PendingWebRtcConnection {
    pub(crate) result: Arc<std::sync::Mutex<Option<PendingResult>>>,
    pub(crate) error: Arc<std::sync::Mutex<Option<String>>>,
}

pub(crate) struct PendingResult {
    pub data_tx: mpsc::UnboundedSender<Vec<u8>>,
    pub data_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub event_rx: mpsc::UnboundedReceiver<WebRtcEvent>,
}

/// Converts `IceConfig` to a `web_sys::RtcConfiguration`.
pub(crate) fn build_rtc_config(ice_config: &IceConfig) -> web_sys::RtcConfiguration {
    let ice_servers = js_sys::Array::new();

    for server in &ice_config.ice_servers {
        let rtc_ice_server = web_sys::RtcIceServer::new();
        let urls = js_sys::Array::new();
        for url in &server.urls {
            urls.push(&wasm_bindgen::JsValue::from_str(url));
        }
        rtc_ice_server.set_urls(&urls);

        if let Some(username) = &server.username {
            rtc_ice_server.set_username(username);
        }
        if let Some(credential) = &server.credential {
            rtc_ice_server.set_credential(credential);
        }
        ice_servers.push(&rtc_ice_server);
    }

    let config = web_sys::RtcConfiguration::new();
    config.set_ice_servers(&ice_servers);
    config
}

/// Creates an unreliable, unordered DataChannel config (raw pipe for lightyear).
pub(crate) fn create_data_channel_init() -> web_sys::RtcDataChannelInit {
    let init = web_sys::RtcDataChannelInit::new();
    init.set_ordered(false);
    init.set_max_retransmits(0);
    init
}

/// Helper future that yields the next item from an `UnboundedReceiver`.
/// Equivalent to `StreamExt::next()` without requiring the `futures` crate.
struct Next<'a, T> {
    receiver: &'a mut mpsc::UnboundedReceiver<T>,
}

impl<T> Future for Next<'_, T> {
    type Output = Option<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll_next(cx) {
            Poll::Ready(item) => Poll::Ready(item),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn next<T>(receiver: &mut mpsc::UnboundedReceiver<T>) -> Next<'_, T> {
    Next { receiver }
}

/// Waits for ICE gathering to complete, then returns the full local SDP
/// (with all candidates baked in).
pub(crate) async fn wait_for_ice_gathering(pc: &web_sys::RtcPeerConnection) -> Result<String, crate::WebRtcError> {
    use wasm_bindgen::prelude::*;

    // If already complete, return immediately
    if pc.ice_gathering_state() == web_sys::RtcIceGatheringState::Complete {
        return pc.local_description()
            .ok_or_else(|| crate::WebRtcError::Connection("no local description".into()))
            .map(|desc| desc.sdp());
    }

    // Otherwise wait for the gathering state to become complete
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    let tx = std::sync::Mutex::new(Some(tx));
    let cb = Closure::<dyn FnMut()>::new({
        let pc = pc.clone();
        move || {
            if pc.ice_gathering_state() == web_sys::RtcIceGatheringState::Complete
                && let Some(tx) = tx.lock().unwrap().take()
            {
                let _ = tx.send(());
            }
        }
    });
    pc.set_onicegatheringstatechange(Some(cb.as_ref().unchecked_ref()));
    cb.forget();

    // Check again in case it completed between our first check and setting the callback
    if pc.ice_gathering_state() == web_sys::RtcIceGatheringState::Complete {
        return pc.local_description()
            .ok_or_else(|| crate::WebRtcError::Connection("no local description".into()))
            .map(|desc| desc.sdp());
    }

    rx.await.map_err(|_| crate::WebRtcError::Connection("ICE gathering cancelled".into()))?;

    pc.local_description()
        .ok_or_else(|| crate::WebRtcError::Connection("no local description after ICE".into()))
        .map(|desc| desc.sdp())
}

/// Sets up DataChannel callbacks that forward data through mpsc channels.
///
/// Returns `(data_tx_for_send_system, data_rx_for_recv_system, event_rx)`.
pub(crate) fn wire_data_channel(
    dc: &web_sys::RtcDataChannel,
) -> (
    mpsc::UnboundedSender<Vec<u8>>,
    mpsc::UnboundedReceiver<Vec<u8>>,
    mpsc::UnboundedReceiver<WebRtcEvent>,
) {
    use wasm_bindgen::prelude::*;

    dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);

    let (incoming_tx, incoming_rx) = mpsc::unbounded::<Vec<u8>>();
    let (outgoing_tx, outgoing_rx) = mpsc::unbounded::<Vec<u8>>();
    let (event_tx, event_rx) = mpsc::unbounded::<WebRtcEvent>();

    // onmessage: forward incoming data to the channel
    let tx = incoming_tx.clone();
    let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
        if let Ok(buf) = ev.data().dyn_into::<js_sys::ArrayBuffer>() {
            let array = js_sys::Uint8Array::new(&buf);
            let data = array.to_vec();
            let _ = tx.unbounded_send(data);
        }
    });
    dc.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // onopen: signal connected
    let tx = event_tx.clone();
    let onopen = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.unbounded_send(WebRtcEvent::Connected);
    });
    dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    // onclose: signal disconnected
    let tx = event_tx.clone();
    let onclose = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.unbounded_send(WebRtcEvent::Disconnected("DataChannel closed".to_string()));
    });
    dc.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();

    // onerror: signal error
    let tx = event_tx.clone();
    let onerror = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.unbounded_send(WebRtcEvent::Error("DataChannel error".to_string()));
    });
    dc.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    // Spawn a task to forward outgoing data to the DataChannel
    let dc_clone = dc.clone();
    let mut outgoing_rx = outgoing_rx;
    wasm_bindgen_futures::spawn_local(async move {
        while let Some(data) = next(&mut outgoing_rx).await {
            let array = js_sys::Uint8Array::from(data.as_slice());
            let _ = dc_clone.send_with_array_buffer_view(&array);
        }
    });

    (outgoing_tx, incoming_rx, event_rx)
}
