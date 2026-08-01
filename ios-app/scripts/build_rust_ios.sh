#!/bin/sh
set -euo pipefail

# Builds the Rust core for iOS. Produces TWO static libs, because a simulator slice cannot
# run on a phone and vice versa:
#
#   build/libseal_ffi_sim.a  → x86_64 + arm64 simulator (Intel and Apple-silicon Macs)
#   build/libseal_ffi_ios.a  → arm64 device            (what you install on a real iPhone)
#
# project.yml links whichever matches the active SDK. Building only the simulator lib — as
# this script used to — makes a device build fail to link, so installing on a phone was
# impossible.

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$ROOT_DIR/ios-app/build"
mkdir -p "$BUILD_DIR"

export PATH="$HOME/.cargo/bin:$PATH"
export RUSTUP_TOOLCHAIN=stable
export RUSTC="$HOME/.cargo/bin/rustc"
export CARGO="$HOME/.cargo/bin/cargo"

# Install the targets once; a no-op when they are already present.
"$HOME/.cargo/bin/rustup" target add x86_64-apple-ios aarch64-apple-ios-sim aarch64-apple-ios 2>/dev/null || true

# ── simulator ──
"$CARGO" build --manifest-path "$ROOT_DIR/Cargo.toml" --release -p seal-ffi --target x86_64-apple-ios
"$CARGO" build --manifest-path "$ROOT_DIR/Cargo.toml" --release -p seal-ffi --target aarch64-apple-ios-sim

lipo -create \
  "$ROOT_DIR/target/x86_64-apple-ios/release/libseal_ffi.a" \
  "$ROOT_DIR/target/aarch64-apple-ios-sim/release/libseal_ffi.a" \
  -output "$BUILD_DIR/libseal_ffi_sim.a"

# ── real device ──
"$CARGO" build --manifest-path "$ROOT_DIR/Cargo.toml" --release -p seal-ffi --target aarch64-apple-ios
cp "$ROOT_DIR/target/aarch64-apple-ios/release/libseal_ffi.a" "$BUILD_DIR/libseal_ffi_ios.a"

echo "built:"
echo "  $BUILD_DIR/libseal_ffi_sim.a  ($(lipo -archs "$BUILD_DIR/libseal_ffi_sim.a"))"
echo "  $BUILD_DIR/libseal_ffi_ios.a  ($(lipo -archs "$BUILD_DIR/libseal_ffi_ios.a"))"
