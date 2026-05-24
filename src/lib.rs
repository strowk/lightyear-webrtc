#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;
pub mod signaling;
pub mod connection;

use alloc::string::String;
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use lightyear_core::time::Instant;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSystems, LinkSystems, Linked, Linking, Unlinked,
};

use crate::connection::{PendingWebRtcConnection, WebRtcChannels, WebRtcEvent};

#[derive(thiserror::Error, Debug)]
pub enum WebRtcError {
    #[error("signaling failed: {0}")]
    Signaling(String),
    #[error("WebRTC connection failed: {0}")]
    Connection(String),
    #[error("DataChannel error: {0}")]
    DataChannel(String),
}

/// ICE server configuration. No defaults — must be explicitly provided.
#[derive(Clone, Debug)]
pub struct IceConfig {
    pub ice_servers: Vec<IceServer>,
}

#[derive(Clone, Debug)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

pub mod prelude {
    pub use crate::{IceConfig, IceServer, WebRtcError, WebRtcPlugin};
    pub use crate::signaling::{SignalingClient, SignalingError};
    pub use crate::connection::WebRtcChannels;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{WebRtcClientIo, WebRtcClientPlugin};
    }

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::{WebRtcServerIo, WebRtcServerPlugin};
    }
}

/// Core plugin that provides WebRTC send/recv systems and connection polling.
/// Added automatically by `WebRtcServerPlugin` and `WebRtcClientPlugin`.
pub struct WebRtcPlugin;

impl Plugin for WebRtcPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_systems(PreUpdate, poll_pending_connections);
        app.add_systems(
            PreUpdate,
            receive.in_set(LinkReceiveSystems::BufferToLink),
        );
        app.add_systems(PostUpdate, send.in_set(LinkSystems::Send));
        app.add_systems(PreUpdate, poll_events);

        #[cfg(feature = "server")]
        app.add_observer(on_server_linked);
    }
}

fn poll_pending_connections(
    query: Query<(Entity, &PendingWebRtcConnection), With<Linking>>,
    mut commands: Commands,
) {
    for (entity, pending) in query.iter() {
        // Check for error first
        if let Some(error) = pending.error.lock().unwrap().take() {
            commands
                .entity(entity)
                .remove::<PendingWebRtcConnection>()
                .insert(Unlinked { reason: error });
            continue;
        }
        // Check for successful result
        if let Some(result) = pending.result.lock().unwrap().take() {
            commands
                .entity(entity)
                .remove::<PendingWebRtcConnection>()
                .insert((
                    WebRtcChannels {
                        data_rx: result.data_rx,
                        data_tx: result.data_tx,
                        event_rx: result.event_rx,
                    },
                    Linked,
                ));
        }
    }
}

fn receive(mut query: Query<(&mut Link, &mut WebRtcChannels), With<Linked>>) {
    for (mut link, mut channels) in query.iter_mut() {
        while let Ok(data) = channels.data_rx.try_recv() {
            link.recv.push(bytes::Bytes::from(data), Instant::now());
        }
    }
}

fn send(mut query: Query<(&mut Link, &WebRtcChannels), With<Linked>>) {
    for (mut link, channels) in query.iter_mut() {
        for payload in link.send.drain() {
            let _ = channels.data_tx.unbounded_send(payload.to_vec());
        }
    }
}

fn poll_events(
    mut query: Query<(Entity, &mut WebRtcChannels), With<Linked>>,
    mut commands: Commands,
) {
    for (entity, mut channels) in query.iter_mut() {
        while let Ok(event) = channels.event_rx.try_recv() {
            match event {
                WebRtcEvent::Disconnected(reason) => {
                    commands.entity(entity).insert(Unlinked { reason });
                    break;
                }
                WebRtcEvent::Error(reason) => {
                    commands.entity(entity).insert(Unlinked { reason });
                    break;
                }
                WebRtcEvent::Connected => {}
            }
        }
    }
}

#[cfg(feature = "server")]
fn on_server_linked(
    trigger: On<Add, Linked>,
    query: Query<Entity, With<crate::server::WebRtcServerIo>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    if query.contains(entity) {
        commands.spawn((
            lightyear_link::server::LinkOf { server: entity },
            Link::new(None),
        ));
    }
}
