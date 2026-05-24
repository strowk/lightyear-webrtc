# Lobby Game Example

A minimal example showing how to use a custom WebSocket signaling server with `lightyear-webrtc`.
Two players connect via a lobby, then move colored squares in 2D space and see each other.

## Architecture

```
┌─────────────┐         ┌──────────────────┐         ┌─────────────┐
│  Host (WASM)│◄──WS───►│ Signaling Server │◄──WS───►│ Guest (WASM)│
│  Bevy app   │         │  (axum, native)  │         │  Bevy app   │
└──────┬──────┘         └──────────────────┘         └──────┬──────┘
       │                                                     │
       └────────────── WebRTC DataChannel ───────────────────┘
                    (peer-to-peer after signaling)
```

## How It Works

1. **Host** opens the game with `?host` in the URL, chooses room settings via query params
2. **Signaling server** creates a room and gives the host a short room code
3. **Guest** opens the game with `?code=ABCDEF` to join a private room (or browses public rooms)
4. The server relays the SDP offer/answer between host and guest
5. Once the WebRTC connection is established, players communicate peer-to-peer

## Running

### Prerequisites

- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-bindgen-cli`: `cargo install wasm-bindgen-cli`
- A static file server (e.g. `npx serve`)

### 1. Start the signaling server

```bash
cd examples/signaling_server
cargo run
```

This starts the server on `http://0.0.0.0:3536`.

### 2. Build and serve the game

In the other terminal:

```bash
cd examples/lobby_game
./build_web.sh
npx serve out -p 8888 # or any static file server of your choice
```

### 3. Open host in browser

Navigate to:
```
http://localhost:8888/?host&name=MyRoom
```

With more options:
```
http://localhost:8888/?host&name=MyRoom&public=false&password=secret123
```

The host will see a room code displayed (e.g. "Room Code: **X4K9NP**").

### 4. Open guest in another browser tab

Join by room code (for private rooms):
```
http://localhost:8888/?code=X4K9NP
```

Join with password:
```
http://localhost:8888/?code=X4K9NP&password=secret123
```

### 5. Play!

- Use **WASD** or **Arrow Keys** to move your square
- Green square = host, Blue square = guest
- Both players see each other's movement in real-time

## Query Parameters

### Host
| Param      | Description                          | Default    |
|-----------|--------------------------------------|------------|
| `host`    | Required. Marks this as the host     | -          |
| `name`    | Room name (visible in public list)   | "My Game"  |
| `public`  | Whether room is publicly listed      | `true`     |
| `password`| Password required to join            | none       |

### Guest
| Param      | Description                          | Default    |
|-----------|--------------------------------------|------------|
| `code`    | Room code for joining private rooms  | -          |
| `room_id` | Room UUID for joining public rooms   | -          |
| `password`| Password if host configured one      | none       |

## REST API (Signaling Server)

- `GET /rooms?search=term` - List public rooms (JSON array)
- `WS /host?name=...&public=...&password=...` - Host WebSocket
- `WS /join?code=...&password=...` or `WS /join?room_id=...` - Guest WebSocket

## Protocol

The signaling protocol matches the `SignalingClient` trait:

```
Host                    Server                   Guest
 │                        │                        │
 │──WS connect──────────►│                        │
 │◄─room_created(code)───│                        │
 │──offer(sdp)──────────►│                        │
 │                        │◄──────WS connect───────│
 │                        │──offer(sdp)──────────►│
 │                        │◄──answer(sdp)──────────│
 │◄─answer(sdp)──────────│                        │
 │                        │──done─────────────────►│
 │        [WebRTC P2P established]                 │
```

## ICE / STUN

This example uses Google's public STUN server (`stun:stun.l.google.com:19302`).
This works for most network configurations but will NOT work behind symmetric NAT.
For production, consider using a TURN server.
