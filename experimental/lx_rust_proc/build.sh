#!/usr/bin/env bash
set -euo pipefail
TASK_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUST_PROC_TARGET="${1:-aarch64-linux-android}"
case "$RUST_PROC_TARGET" in
 aarch64-linux-android) RUST_PROC_OUTPUT=android-arm64 ;;
 x86_64-unknown-linux-gnu) RUST_PROC_OUTPUT=linux-amd64 ;;
 *) echo "unsupported experimental Rust scanner target" >&2; exit 2 ;;
esac
cargo build --manifest-path "$TASK_ROOT/Cargo.toml" --locked --offline --release --target "$RUST_PROC_TARGET"
mkdir -p "$TASK_ROOT/lib/$RUST_PROC_OUTPUT"
cp "$TASK_ROOT/target/$RUST_PROC_TARGET/release/libmagicnet_proc_scan.a" "$TASK_ROOT/lib/$RUST_PROC_OUTPUT/"
