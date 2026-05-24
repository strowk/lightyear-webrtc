use bevy::prelude::*;
use lightyear_link::Linked;
use lightyear_webrtc::connection::WebRtcChannels;

use crate::{NetworkEntity, NetworkRole};

const PADDLE_SPEED: f32 = 300.0;
const PADDLE_WIDTH: f32 = 20.0;
const PADDLE_HEIGHT: f32 = 100.0;
const ARENA_HEIGHT: f32 = 600.0;

const BALL_SIZE: f32 = 15.0;
const BALL_SPEED: f32 = 300.0;
const RESET_X: f32 = 350.0;

#[derive(Component)]
pub(crate) struct LocalPaddle;

#[derive(Component)]
pub(crate) struct RemotePaddle;

#[derive(Component)]
pub(crate) struct Ball;

#[derive(Component)]
pub(crate) struct BallVelocity(Vec2);

#[derive(Resource, Default)]
pub(crate) struct GameStarted(bool);

pub(crate) fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

pub(crate) fn setup_paddles(mut commands: Commands, role: Res<NetworkRole>) {
    let local_x = if role.is_host { -300.0 } else { 300.0 };
    let remote_x = if role.is_host { 300.0 } else { -300.0 };

    let paddle_size = Vec2::new(PADDLE_WIDTH, PADDLE_HEIGHT);

    // Local paddle (green)
    commands.spawn((
        LocalPaddle,
        Sprite::from_color(Color::srgb(0.2, 0.8, 0.2), paddle_size),
        Transform::from_xyz(local_x, 0.0, 0.0),
    ));

    // Remote paddle (blue)
    commands.spawn((
        RemotePaddle,
        Sprite::from_color(Color::srgb(0.2, 0.2, 0.8), paddle_size),
        Transform::from_xyz(remote_x, 0.0, 0.0),
    ));
}

pub(crate) fn setup_ball(mut commands: Commands) {
    commands.spawn((
        Ball,
        BallVelocity(Vec2::ZERO),
        Sprite::from_color(Color::srgb(0.9, 0.9, 0.9), Vec2::splat(BALL_SIZE)),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
}

pub(crate) fn init_game_started(app: &mut App) {
    app.init_resource::<GameStarted>();
}

fn random_ball_velocity() -> Vec2 {
    let angle = (js_sys::Math::random() as f32 - 0.5) * core::f32::consts::FRAC_PI_4;
    let dir_x = if js_sys::Math::random() > 0.5 {
        1.0
    } else {
        -1.0
    };
    Vec2::new(dir_x * angle.cos(), angle.sin()) * BALL_SPEED
}

pub(crate) fn start_ball_on_connect(
    net_entity: Option<Res<NetworkEntity>>,
    link_query: Query<(), With<Linked>>,
    mut game_started: ResMut<GameStarted>,
    mut ball_query: Query<&mut BallVelocity, With<Ball>>,
    role: Res<NetworkRole>,
) {
    if game_started.0 {
        return;
    }
    let Some(net_entity) = net_entity else { return };
    if link_query.get(net_entity.0).is_err() {
        return;
    }

    game_started.0 = true;

    if role.is_host {
        if let Ok(mut vel) = ball_query.single_mut() {
            vel.0 = random_ball_velocity();
        }
    }
}

pub(crate) fn move_local_paddle(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut query: Query<&mut Transform, With<LocalPaddle>>,
) {
    let mut direction = 0.0f32;
    if keyboard.pressed(KeyCode::ArrowUp) {
        direction += 1.0;
    }
    if keyboard.pressed(KeyCode::ArrowDown) {
        direction -= 1.0;
    }

    for mut transform in query.iter_mut() {
        transform.translation.y += direction * PADDLE_SPEED * time.delta_secs();
        transform.translation.y = transform.translation.y.clamp(
            -ARENA_HEIGHT / 2.0 + PADDLE_HEIGHT / 2.0,
            ARENA_HEIGHT / 2.0 - PADDLE_HEIGHT / 2.0,
        );
    }
}

pub(crate) fn ball_physics(
    time: Res<Time>,
    game_started: Res<GameStarted>,
    mut ball_query: Query<(&mut Transform, &mut BallVelocity), With<Ball>>,
    paddle_query: Query<
        &Transform,
        (
            Or<(With<LocalPaddle>, With<RemotePaddle>)>,
            Without<Ball>,
        ),
    >,
) {
    if !game_started.0 {
        return;
    }
    let Ok((mut ball_t, mut ball_v)) = ball_query.single_mut() else {
        return;
    };

    // Movement
    ball_t.translation.x += ball_v.0.x * time.delta_secs();
    ball_t.translation.y += ball_v.0.y * time.delta_secs();

    // Top/bottom wall bounce
    let half_arena = ARENA_HEIGHT / 2.0 - BALL_SIZE / 2.0;
    if ball_t.translation.y.abs() > half_arena {
        ball_t.translation.y = ball_t.translation.y.signum() * half_arena;
        ball_v.0.y = -ball_v.0.y;
    }

    // Paddle collision
    for paddle_t in paddle_query.iter() {
        let dx = (ball_t.translation.x - paddle_t.translation.x).abs();
        let dy = (ball_t.translation.y - paddle_t.translation.y).abs();

        let overlap_x = (PADDLE_WIDTH + BALL_SIZE) / 2.0;
        let overlap_y = (PADDLE_HEIGHT + BALL_SIZE) / 2.0;

        if dx < overlap_x && dy < overlap_y {
            ball_v.0.x = -ball_v.0.x;
            let sign = (ball_t.translation.x - paddle_t.translation.x).signum();
            ball_t.translation.x = paddle_t.translation.x + sign * overlap_x;

            // Vary Y angle based on where on the paddle the ball hit
            let hit_ratio = (ball_t.translation.y - paddle_t.translation.y) / (PADDLE_HEIGHT / 2.0);
            ball_v.0.y = hit_ratio * BALL_SPEED * 0.8;
        }
    }

    // Reset if past edges
    if ball_t.translation.x.abs() > RESET_X {
        ball_t.translation.x = 0.0;
        ball_t.translation.y = 0.0;
        ball_v.0 = random_ball_velocity();
    }
}

pub(crate) fn send_state(
    role: Res<NetworkRole>,
    net_entity: Option<Res<NetworkEntity>>,
    paddle_query: Query<&Transform, With<LocalPaddle>>,
    ball_query: Query<&Transform, With<Ball>>,
    link_query: Query<&WebRtcChannels, With<Linked>>,
) {
    let Some(net_entity) = net_entity else { return };
    let Ok(channels) = link_query.get(net_entity.0) else {
        return;
    };

    // Both sides send paddle position (tag 0)
    if let Ok(transform) = paddle_query.single() {
        let y = transform.translation.y;
        let mut msg = vec![0u8];
        msg.extend_from_slice(&y.to_le_bytes());
        let _ = channels.data_tx.unbounded_send(msg);
    }

    // Host also sends ball position (tag 1)
    if role.is_host {
        if let Ok(transform) = ball_query.single() {
            let mut msg = vec![1u8];
            msg.extend_from_slice(&transform.translation.x.to_le_bytes());
            msg.extend_from_slice(&transform.translation.y.to_le_bytes());
            let _ = channels.data_tx.unbounded_send(msg);
        }
    }
}

pub(crate) fn receive_state(
    role: Res<NetworkRole>,
    net_entity: Option<Res<NetworkEntity>>,
    mut channels_query: Query<
        (&mut WebRtcChannels, Option<&mut lightyear_link::Link>),
        With<Linked>,
    >,
    mut paddle_query: Query<&mut Transform, (With<RemotePaddle>, Without<Ball>)>,
    mut ball_query: Query<&mut Transform, (With<Ball>, Without<RemotePaddle>)>,
) {
    let Some(net_entity) = net_entity else { return };
    let Ok((mut channels, link)) = channels_query.get_mut(net_entity.0) else {
        return;
    };

    let mut messages: Vec<Vec<u8>> = Vec::new();

    if let Some(mut link) = link {
        for payload in link.recv.drain() {
            messages.push(payload.to_vec());
        }
    }
    while let Ok(data) = channels.data_rx.try_recv() {
        messages.push(data.to_vec());
    }

    let mut latest_paddle_y = None;
    let mut latest_ball_pos = None;

    for msg in messages {
        if msg.is_empty() {
            continue;
        }
        match msg[0] {
            0 if msg.len() == 5 => {
                latest_paddle_y =
                    Some(f32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]));
            }
            1 if msg.len() == 9 => {
                let x = f32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
                let y = f32::from_le_bytes([msg[5], msg[6], msg[7], msg[8]]);
                latest_ball_pos = Some((x, y));
            }
            _ => {}
        }
    }

    if let Some(y) = latest_paddle_y {
        if let Ok(mut t) = paddle_query.single_mut() {
            t.translation.y = y;
        }
    }

    // Client receives ball position from host
    if !role.is_host {
        if let Some((x, y)) = latest_ball_pos {
            if let Ok(mut t) = ball_query.single_mut() {
                t.translation.x = x;
                t.translation.y = y;
            }
        }
    }
}
