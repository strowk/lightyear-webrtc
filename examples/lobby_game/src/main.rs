mod signaling;

use bevy::prelude::*;
use lightyear_link::{Link, LinkStart, Linked};
use lightyear_webrtc::{IceConfig, IceServer};

use crate::signaling::{GuestWsSignaling, HostWsSignaling};

const SIGNALING_URL: &str = "ws://127.0.0.1:3536";
const PLAYER_SIZE: f32 = 40.0;
const PLAYER_SPEED: f32 = 200.0;

#[derive(Resource)]
struct NetworkRole {
    is_host: bool,
}

#[derive(Resource)]
struct NetworkEntity(Entity);

#[derive(Component)]
struct LocalPlayer;

#[derive(Component)]
struct RemotePlayer;

fn main() {
    let is_host = get_is_host();
    let room_code = get_room_code();

    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: if is_host {
                "Lobby - HOST".into()
            } else {
                "Lobby - GUEST".into()
            },
            canvas: Some("#bevy-canvas".into()),
            ..default()
        }),
        ..default()
    }));

    app.insert_resource(NetworkRole { is_host });
    if let Some(code) = room_code {
        app.insert_resource(JoinCode(code));
    }

    if is_host {
        app.add_plugins(lightyear_webrtc::server::WebRtcServerPlugin);
    } else {
        app.add_plugins(lightyear_webrtc::client::WebRtcClientPlugin);
    }

    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.1)));
    app.add_systems(Startup, setup);
    app.add_systems(Update, move_local_player);
    app.add_systems(Update, send_position);
    app.add_systems(Update, receive_position);

    app.run();
}

#[derive(Resource)]
struct JoinCode(String);

fn get_is_host() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("host"))
        .unwrap_or(false)
}

fn get_room_code() -> Option<String> {
    get_query_param("code")
}

fn get_room_id() -> Option<String> {
    get_query_param("room_id")
}

fn get_query_param(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let prefix = format!("{name}=");
    for part in search.trim_start_matches('?').split('&') {
        if let Some(value) = part.strip_prefix(&prefix) {
            if !value.is_empty() {
                return Some(
                    js_sys::decode_uri_component(value)
                        .map(|s| s.into())
                        .unwrap_or_else(|_| value.to_string()),
                );
            }
        }
    }
    None
}

fn get_room_name() -> String {
    get_query_param("name").unwrap_or_else(|| "My Game".to_string())
}

fn get_password() -> Option<String> {
    get_query_param("password")
}

fn is_public() -> bool {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    // Default to public unless explicitly set to false
    !search.contains("public=false")
}

fn setup(mut commands: Commands, role: Res<NetworkRole>, join_code: Option<Res<JoinCode>>) {
    commands.spawn(Camera2d);

    let ice_config = IceConfig {
        ice_servers: vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: None,
            credential: None,
        }],
    };

    // Spawn local player
    let local_color = if role.is_host {
        Color::srgb(0.2, 0.8, 0.3) // green for host
    } else {
        Color::srgb(0.3, 0.5, 0.9) // blue for guest
    };
    commands.spawn((
        LocalPlayer,
        Sprite::from_color(local_color, Vec2::splat(PLAYER_SIZE)),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // Spawn remote player (will be updated via network)
    let remote_color = if role.is_host {
        Color::srgb(0.3, 0.5, 0.9)
    } else {
        Color::srgb(0.2, 0.8, 0.3)
    };
    commands.spawn((
        RemotePlayer,
        Sprite::from_color(remote_color, Vec2::splat(PLAYER_SIZE)),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // Setup network
    let entity = if role.is_host {
        let room_name = get_room_name();
        let public = is_public();
        let password = get_password();

        let signaling = HostWsSignaling::new(
            SIGNALING_URL.to_string(),
            room_name,
            public,
            password,
        );

        commands
            .spawn(lightyear_webrtc::server::WebRtcServerIo {
                ice_config,
                signaling: Some(Box::new(signaling)),
            })
            .id()
    } else {
        let code = join_code.map(|c| c.0.clone());
        let room_id = get_room_id();
        let password = get_password();

        let signaling = GuestWsSignaling::new(
            SIGNALING_URL.to_string(),
            room_id,
            code,
            password,
        );

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

fn move_local_player(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut query: Query<&mut Transform, With<LocalPlayer>>,
) {
    let mut dir = Vec2::ZERO;
    if keyboard.pressed(KeyCode::ArrowUp) || keyboard.pressed(KeyCode::KeyW) {
        dir.y += 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowDown) || keyboard.pressed(KeyCode::KeyS) {
        dir.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowLeft) || keyboard.pressed(KeyCode::KeyA) {
        dir.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowRight) || keyboard.pressed(KeyCode::KeyD) {
        dir.x += 1.0;
    }

    if dir != Vec2::ZERO {
        dir = dir.normalize();
    }

    for mut transform in query.iter_mut() {
        transform.translation.x += dir.x * PLAYER_SPEED * time.delta_secs();
        transform.translation.y += dir.y * PLAYER_SPEED * time.delta_secs();
    }
}

fn send_position(
    net_entity: Option<Res<NetworkEntity>>,
    player_query: Query<&Transform, With<LocalPlayer>>,
    mut link_query: Query<&mut Link, With<Linked>>,
) {
    let Some(net_entity) = net_entity else { return };
    let Ok(mut link) = link_query.get_mut(net_entity.0) else {
        return;
    };

    if let Ok(transform) = player_query.single() {
        let x = transform.translation.x;
        let y = transform.translation.y;
        let mut msg = Vec::with_capacity(8);
        msg.extend_from_slice(&x.to_le_bytes());
        msg.extend_from_slice(&y.to_le_bytes());
        link.send.push(bytes::Bytes::from(msg));
    }
}

fn receive_position(
    net_entity: Option<Res<NetworkEntity>>,
    mut link_query: Query<&mut Link, With<Linked>>,
    mut remote_query: Query<&mut Transform, With<RemotePlayer>>,
) {
    let Some(net_entity) = net_entity else { return };
    let Ok(mut link) = link_query.get_mut(net_entity.0) else {
        return;
    };

    let mut latest_pos = None;
    for msg in link.recv.drain() {
        if msg.len() == 8 {
            let x = f32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
            let y = f32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
            latest_pos = Some((x, y));
        }
    }

    if let Some((x, y)) = latest_pos {
        if let Ok(mut t) = remote_query.single_mut() {
            t.translation.x = x;
            t.translation.y = y;
        }
    }
}
