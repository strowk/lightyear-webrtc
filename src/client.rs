use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_link::{Link, LinkPlugin, LinkStart, Linked, Linking};
use tracing::{error, info};

use crate::connection::{
    PendingResult, PendingWebRtcConnection,
    build_rtc_config, wait_for_ice_gathering, wire_data_channel,
};
use crate::signaling::SignalingClient;
use crate::IceConfig;

pub struct WebRtcClientPlugin;

/// WebRTC guest (client) IO component.
///
/// Spawn an entity with this component and trigger `LinkStart` to begin
/// the WebRTC connection setup as a guest.
#[derive(Component)]
#[require(Link)]
pub struct WebRtcClientIo {
    pub ice_config: IceConfig,
    pub signaling: Option<Box<dyn SignalingClient>>,
}

impl Plugin for WebRtcClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::WebRtcPlugin>() {
            app.add_plugins(crate::WebRtcPlugin);
        }
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Self::link);
    }
}

impl WebRtcClientPlugin {
    fn link(
        trigger: On<LinkStart>,
        mut query: Query<(Entity, &mut WebRtcClientIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) {
        let Ok((entity, mut client_io)) = query.get_mut(trigger.entity) else {
            return;
        };

        let Some(signaling) = client_io.signaling.take() else {
            error!("WebRtcClientIo signaling already consumed");
            return;
        };

        let ice_config = client_io.ice_config.clone();

        let pending_result = Arc::new(std::sync::Mutex::new(None));
        let pending_error = Arc::new(std::sync::Mutex::new(None));
        let result_clone = pending_result.clone();
        let error_clone = pending_error.clone();

        commands.entity(entity).insert((
            Linking,
            PendingWebRtcConnection {
                result: pending_result,
                error: pending_error,
            },
        ));

        wasm_bindgen_futures::spawn_local(async move {
            match setup_guest(ice_config, signaling).await {
                Ok(result) => {
                    *result_clone.lock().unwrap() = Some(result);
                    info!("WebRTC guest connection established");
                }
                Err(e) => {
                    *error_clone.lock().unwrap() = Some(e.to_string());
                    error!("WebRTC guest setup failed: {e}");
                }
            }
        });
    }
}

async fn setup_guest(
    ice_config: IceConfig,
    mut signaling: Box<dyn SignalingClient>,
) -> Result<PendingResult, crate::WebRtcError> {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    // Retrieve the host's offer via signaling
    let offer_sdp = signaling
        .retrieve_offer()
        .await
        .map_err(|e| crate::WebRtcError::Signaling(e.to_string()))?;

    let config = build_rtc_config(&ice_config);
    let pc = web_sys::RtcPeerConnection::new_with_configuration(&config)
        .map_err(|e| crate::WebRtcError::Connection(format!("{e:?}")))?;

    // Set up a oneshot to receive the DataChannel from the host
    let (dc_tx, dc_rx) = futures_channel::oneshot::channel::<web_sys::RtcDataChannel>();
    let dc_tx = std::sync::Mutex::new(Some(dc_tx));

    let ondatachannel = Closure::<dyn FnMut(web_sys::RtcDataChannelEvent)>::new(
        move |ev: web_sys::RtcDataChannelEvent| {
            let dc = ev.channel();
            if let Some(tx) = dc_tx.lock().unwrap().take() {
                let _ = tx.send(dc);
            }
        },
    );
    pc.set_ondatachannel(Some(ondatachannel.as_ref().unchecked_ref()));
    ondatachannel.forget();

    // Set remote description (the host's offer)
    let offer_desc = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Offer);
    offer_desc.set_sdp(&offer_sdp);
    JsFuture::from(pc.set_remote_description(&offer_desc))
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("set_remote_description failed: {e:?}")))?;

    // Create answer
    let answer = JsFuture::from(pc.create_answer())
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("create_answer failed: {e:?}")))?;
    let answer_sdp = js_sys::Reflect::get(&answer, &"sdp".into())
        .map_err(|e| crate::WebRtcError::Connection(format!("get sdp failed: {e:?}")))?
        .as_string()
        .ok_or_else(|| crate::WebRtcError::Connection("answer sdp not a string".into()))?;

    // Set local description
    let answer_desc = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Answer);
    answer_desc.set_sdp(&answer_sdp);
    JsFuture::from(pc.set_local_description(&answer_desc))
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("set_local_description failed: {e:?}")))?;

    // Wait for ICE candidates to be gathered so the SDP includes them
    let full_answer_sdp = wait_for_ice_gathering(&pc).await?;
    info!("WebRTC guest: ICE gathering complete");

    // Send answer (with candidates) to host via signaling
    signaling
        .submit_answer(full_answer_sdp)
        .await
        .map_err(|e| crate::WebRtcError::Signaling(e.to_string()))?;

    // Wait for the DataChannel from the host
    let dc = dc_rx
        .await
        .map_err(|_| crate::WebRtcError::DataChannel("ondatachannel never fired".into()))?;

    // Wire up the DataChannel
    let (data_tx, data_rx, event_rx) = wire_data_channel(&dc);

    info!("WebRTC guest: DataChannel received and wired");
    Ok(PendingResult { data_tx, data_rx, event_rx })
}
