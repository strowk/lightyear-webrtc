mod game;
mod signaling;

use bevy::prelude::*;
use lightyear_link::LinkStart;
use lightyear_webrtc::{IceConfig, IceServer};

use crate::game::*;
use crate::signaling::ManualSignaling;

#[derive(Resource)]
pub(crate) struct NetworkRole {
    pub(crate) is_host: bool,
}

#[derive(Resource)]
pub(crate) struct NetworkEntity(pub(crate) Entity);

fn main() {
    let is_host = get_is_host();

    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: if is_host {
                "Pong - HOST".into()
            } else {
                "Pong - CLIENT".into()
            },
            canvas: Some("#bevy-canvas".into()),
            ..default()
        }),
        ..default()
    }));

    app.insert_resource(NetworkRole { is_host });
    init_game_started(&mut app);

    if is_host {
        app.add_plugins(lightyear_webrtc::server::WebRtcServerPlugin);
    } else {
        app.add_plugins(lightyear_webrtc::client::WebRtcClientPlugin);
    }

    app.insert_resource(ClearColor(Color::srgb(0.1, 0.0, 0.2)));
    app.add_systems(Startup, setup_camera);
    app.add_systems(Startup, setup_paddles);
    app.add_systems(Startup, setup_ball);
    app.add_systems(Startup, setup_network);
    app.add_systems(Update, move_local_paddle);
    app.add_systems(Update, start_ball_on_connect);
    app.add_systems(
        Update,
        ball_physics.run_if(|role: Res<NetworkRole>| role.is_host),
    );
    app.add_systems(Update, send_state);
    app.add_systems(Update, receive_state);

    app.run();
}

fn get_is_host() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("host"))
        .unwrap_or(false)
}

fn setup_network(mut commands: Commands, role: Res<NetworkRole>) {
    let ice_config = IceConfig {
        ice_servers: vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: None,
            credential: None,
        }],
    };

    let signaling = ManualSignaling;

    let entity = if role.is_host {
        commands
            .spawn(lightyear_webrtc::server::WebRtcServerIo {
                ice_config,
                signaling: Some(Box::new(signaling)),
            })
            .id()
    } else {
        commands
            .spawn(lightyear_webrtc::client::WebRtcClientIo {
                ice_config,
                signaling: Some(Box::new(signaling)),
            })
            .id()
    };

    commands.trigger(LinkStart { entity });
    commands.insert_resource(NetworkEntity(entity));
}
