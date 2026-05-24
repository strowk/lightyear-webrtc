# lightyear_webrtc

Browser-only WebRTC transport for [lightyear](https://github.com/cBournhonesque/lightyear), built on `web-sys`.

https://github.com/user-attachments/assets/77cc416a-bc46-44e9-a073-7ffedd9ad3cf

Provides peer-to-peer data channels between two browser tabs using the standard WebRTC offer/answer flow. One peer acts as the **host** (creates the data channel) and the other as the **client** (receives it). 
Both sides are full WASM applications — there is no native server involved, except for STUN (and optional TURN) for NAT traversal.

## Features

- **Bevy plugin integration** — drop in `WebRtcServerPlugin` / `WebRtcClientPlugin` and the connection lifecycle is managed through Bevy's ECS
- **lightyear_link compatible** — connected peers get a `Link` component for sending and receiving `Bytes` payloads, fitting into lightyear's transport abstraction
- **Pluggable signaling** — implement the `SignalingClient` trait to exchange SDP offers/answers however you want (WebSocket relay, Firebase, manual copy-paste, etc.)
- **Unreliable, unordered data channel** — configured for low-latency game traffic (no retransmits, no ordering guarantees)

## Cargo features

| Feature  | Description |
|----------|-------------|
| `client` | Enables `WebRtcClientPlugin` and `WebRtcClientIo` (guest/client role) |
| `server` | Enables `WebRtcServerPlugin` and `WebRtcServerIo` (host/server role) |

Neither feature is enabled by default. Enable what you need:

```toml
[dependencies]
lightyear_webrtc = { version = "0.26", features = ["client", "server"] }
```

## Quick start

```rust
use lightyear_webrtc::prelude::*;
use lightyear_webrtc::prelude::server::*;
use lightyear_link::LinkStart;

// 1. Configure ICE servers
let ice_config = IceConfig {
    ice_servers: vec![IceServer {
        urls: vec!["stun:stun.l.google.com:19302".into()],
        username: None,
        credential: None,
    }],
};

// 2. Provide a signaling implementation
let signaling = MySignalingImpl::new(/* ... */);

// 3. Spawn the IO entity and trigger the connection
let entity = commands
    .spawn(WebRtcServerIo {
        ice_config,
        signaling: Some(Box::new(signaling)),
    })
    .id();
commands.trigger(LinkStart { entity });
```

The client side is identical but uses `WebRtcClientIo` and `WebRtcClientPlugin`.

Once the WebRTC connection is established, the entity receives:
- `Linked` — marks the connection as live
- `WebRtcChannels` — exposes `data_tx` / `data_rx` for raw `Vec<u8>` messaging
- `Link` — lightyear's transport abstraction with `send` / `recv` buffers

## Signaling

Signaling is the mechanism used to exchange SDP offers and answers between peers before the WebRTC connection is established. This crate does **not** provide a signaling server — you bring your own by implementing `SignalingClient`:

```rust
pub trait SignalingClient: Send + Sync + 'static {
    /// Host publishes its offer and waits for the client's answer.
    fn publish_offer(&mut self, offer: String) -> BoxFuture<'_, Result<String, SignalingError>>;

    /// Client retrieves the host's pending offer.
    fn retrieve_offer(&mut self) -> BoxFuture<'_, Result<String, SignalingError>>;

    /// Client sends its answer back to the host.
    fn submit_answer(&mut self, answer: String) -> BoxFuture<'_, Result<(), SignalingError>>;
}
```

## Connection lifecycle

```
LinkStart triggered
    -> Linking (async setup in progress)
        -> Linked + WebRtcChannels (success)
        -> Unlinked { reason } (failure)
```

## Example: WebRTC Pong

The `examples/webrtc_pong` directory contains a minimal networked Pong game demonstrating the full connection flow with manual copy-paste signaling.

### Building and running

```bash
cd examples/webrtc_pong
bash build_web.sh     # or: bash build_web.sh release
npx serve out -p 8888 # or any static file server of your choice
```

Then open two browser tabs:
1. Navigate to `http://localhost:8888/?host`
2. Navigate to `http://localhost:8888/?client`
3. Copy the OFFER from the host tab, paste it into the client tab
4. Copy the ANSWER from the client tab, paste it back into the host tab
5. Play Pong with arrow keys

The host is authoritative over ball physics and sends ball position to the client. Both peers send their paddle position to each other.

## Requirements

- **Target:** `wasm32-unknown-unknown` (browser only)
- **Rust:** 1.88+
- **Bevy:** 0.18

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

## Disclaimer

This is mostly fun weekend project for me to learn about WebRTC, this is not really indended to be production ready transport solution. 
That said, if you find this useful or want to contribute, please open an issue or PR, I will try to respond when I have time.

