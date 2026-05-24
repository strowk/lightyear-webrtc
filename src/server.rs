use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use bevy_app::{App, Plugin};
use bevy_ecs::prelude::*;
use lightyear_link::{Link, LinkPlugin, LinkStart, Linked, Linking};
use lightyear_link::server::{Server, ServerLinkPlugin};
use tracing::{error, info};

use crate::connection::{
    PendingResult, PendingWebRtcConnection,
    build_rtc_config, create_data_channel_init, wait_for_ice_gathering, wire_data_channel,
};
use crate::signaling::SignalingClient;
use crate::IceConfig;

pub struct WebRtcServerPlugin;

/// WebRTC host (server) IO component.
///
/// Spawn an entity with this component and trigger `LinkStart` to begin
/// the WebRTC connection setup as a host.
#[derive(Component)]
#[require(Server, Link)]
pub struct WebRtcServerIo {
    pub ice_config: IceConfig,
    pub signaling: Option<Box<dyn SignalingClient>>,
}

impl Plugin for WebRtcServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::WebRtcPlugin>() {
            app.add_plugins(crate::WebRtcPlugin);
        }
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        if !app.is_plugin_added::<ServerLinkPlugin>() {
            app.add_plugins(ServerLinkPlugin);
        }
        app.add_observer(Self::link);
    }
}

impl WebRtcServerPlugin {
    fn link(
        trigger: On<LinkStart>,
        mut query: Query<(Entity, &mut WebRtcServerIo), (Without<Linking>, Without<Linked>)>,
        mut commands: Commands,
    ) {
        let Ok((entity, mut server_io)) = query.get_mut(trigger.entity) else {
            return;
        };

        let Some(signaling) = server_io.signaling.take() else {
            error!("WebRtcServerIo signaling already consumed");
            return;
        };

        let ice_config = server_io.ice_config.clone();

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
            match setup_host(ice_config, signaling).await {
                Ok(result) => {
                    *result_clone.lock().unwrap() = Some(result);
                    info!("WebRTC host connection established");
                }
                Err(e) => {
                    *error_clone.lock().unwrap() = Some(e.to_string());
                    error!("WebRTC host setup failed: {e}");
                }
            }
        });
    }
}

async fn setup_host(
    ice_config: IceConfig,
    mut signaling: Box<dyn SignalingClient>,
) -> Result<PendingResult, crate::WebRtcError> {
    use wasm_bindgen_futures::JsFuture;

    let config = build_rtc_config(&ice_config);
    let pc = web_sys::RtcPeerConnection::new_with_configuration(&config)
        .map_err(|e| crate::WebRtcError::Connection(format!("{e:?}")))?;

    // Create the DataChannel (host creates it, guest receives it via ondatachannel)
    let dc_init = create_data_channel_init();
    let dc = pc.create_data_channel_with_data_channel_dict("lightyear", &dc_init);

    // Wire up DataChannel callbacks
    let (data_tx, data_rx, event_rx) = wire_data_channel(&dc);

    // Create offer
    let offer = JsFuture::from(pc.create_offer())
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("create_offer failed: {e:?}")))?;
    let offer_sdp = js_sys::Reflect::get(&offer, &"sdp".into())
        .map_err(|e| crate::WebRtcError::Connection(format!("get sdp failed: {e:?}")))?
        .as_string()
        .ok_or_else(|| crate::WebRtcError::Connection("offer sdp not a string".into()))?;

    // Set local description
    let offer_desc = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Offer);
    offer_desc.set_sdp(&offer_sdp);
    JsFuture::from(pc.set_local_description(&offer_desc))
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("set_local_description failed: {e:?}")))?;

    // Wait for ICE candidates to be gathered so the SDP includes them
    let full_offer_sdp = wait_for_ice_gathering(&pc).await?;
    info!("WebRTC host: ICE gathering complete");

    // Exchange via signaling: publish offer (with candidates), get answer
    let answer_sdp = signaling
        .publish_offer(full_offer_sdp)
        .await
        .map_err(|e| crate::WebRtcError::Signaling(e.to_string()))?;

    // Set remote description (the answer)
    let answer_desc = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Answer);
    answer_desc.set_sdp(&answer_sdp);
    JsFuture::from(pc.set_remote_description(&answer_desc))
        .await
        .map_err(|e| crate::WebRtcError::Connection(format!("set_remote_description failed: {e:?}")))?;

    info!("WebRTC host: remote description set, waiting for DataChannel to open");
    Ok(PendingResult { data_tx, data_rx, event_rx })
}
