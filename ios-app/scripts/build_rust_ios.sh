#!/bin/sh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$ROOT_DIR/ios-app/build"
mkdir -p "$BUILD_DIR"

export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=stable
export RUSTC="$HOME/.cargo/bin/rustc"
export CARGO="$HOME/.cargo/bin/cargo"

"$CARGO" build --manifest-path "$ROOT_DIR/Cargo.toml" --release -p seal-ffi --target x86_64-apple-ios
"$CARGO" build --manifest-path "$ROOT_DIR/Cargo.toml" --release -p seal-ffi --target aarch64-apple-ios-sim

lipo -create \
  "$ROOT_DIR/target/x86_64-apple-ios/release/libseal_ffi.a" \
  "$ROOT_DIR/target/aarch64-apple-ios-sim/release/libseal_ffi.a" \
  -output "$BUILD_DIR/libseal_ffi_sim.a"
