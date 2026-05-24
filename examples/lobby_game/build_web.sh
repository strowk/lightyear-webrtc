#!/bin/bash
set -e

cd "$(dirname "$0")"

PROFILE="${1:-debug}"
BUILD_FLAGS=""
if [ "$PROFILE" == "release" ]; then
    BUILD_FLAGS="--release"
fi

echo "Building lobby_game for WASM ($PROFILE)..."
RUSTFLAGS="--cfg web_sys_unstable_apis --cfg getrandom_backend=\"wasm_js\"" \
cargo build --target wasm32-unknown-unknown $BUILD_FLAGS

echo "Running wasm-bindgen..."
wasm-bindgen --no-typescript \
    --target web \
    --out-dir ./out/ \
    --out-name "lobby_game" \
    ./target/wasm32-unknown-unknown/$PROFILE/lobby_game.wasm

echo "Copying web files..."
cp web/* out/

echo "Build complete! Files in examples/lobby_game/out/"
echo "Run: npx serve out -p 8888"
