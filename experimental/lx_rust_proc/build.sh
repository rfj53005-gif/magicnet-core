#!/usr/bin/env bash
# 构建 lx_rust_proc 静态库。
#   ./build.sh <rust-target> [rust1|rust2|bench]
# rust1  默认实现，输出 lib/<target>/；
# rust2  生产候选，输出 lib/<target>-rust2/；
# bench  基准专用（同时含两种实现 + 分配计数），输出 lib/<target>-bench/。
set -euo pipefail
TASK_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${1:-aarch64-linux-android}"
VARIANT="${2:-rust1}"

case "$TARGET" in
aarch64-linux-android) OUTPUT=android-arm64 ;;
x86_64-unknown-linux-gnu) OUTPUT=linux-amd64 ;;
*)
	echo "unsupported experimental Rust scanner target: $TARGET" >&2
	exit 2
	;;
esac

case "$VARIANT" in
rust1)
	FEATURES=()
	OUT_DIR="$TASK_ROOT/lib/$OUTPUT"
	NEEDS_RUST2=0
	;;
rust2)
	FEATURES=(--features rust2)
	OUT_DIR="$TASK_ROOT/lib/$OUTPUT-rust2"
	NEEDS_RUST2=1
	;;
bench)
	FEATURES=(--features bench-dual,stats)
	OUT_DIR="$TASK_ROOT/lib/$OUTPUT-bench"
	NEEDS_RUST2=1
	;;
*)
	echo "unsupported variant: $VARIANT" >&2
	exit 2
	;;
esac

cargo build --manifest-path "$TASK_ROOT/Cargo.toml" --locked --offline --release \
	--target "$TARGET" "${FEATURES[@]}"
ARCHIVE="$TASK_ROOT/target/$TARGET/release/libmagicnet_proc_scan.a"
mkdir -p "$OUT_DIR"
cp "$ARCHIVE" "$OUT_DIR/"

# 用 NDK 的 llvm-nm 校验导出符号，防止 Go 标签与静态库错配。
if [[ -n "${ANDROID_NDK_HOME:-}" && -x "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm" ]]; then
	NM="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm"
else
	NM=nm
fi
"$NM" -g --defined-only "$OUT_DIR/libmagicnet_proc_scan.a" >"$OUT_DIR/symbols.txt"
grep -q 'magicnet_proc_scanner_variant' "$OUT_DIR/symbols.txt" ||
	{
		echo "missing magicnet_proc_scanner_variant in $VARIANT archive" >&2
		exit 1
	}
if [[ "$NEEDS_RUST2" = 1 ]]; then
	grep -q 'magicnet_proc_scanner_rust2_marker' "$OUT_DIR/symbols.txt" ||
		{
			echo "variant=$VARIANT but archive has no Rust2 marker" >&2
			exit 1
		}
else
	if grep -q 'magicnet_proc_scanner_rust2_marker' "$OUT_DIR/symbols.txt"; then
		echo "variant=rust1 but archive contains Rust2 marker" >&2
		exit 1
	fi
fi
if [[ "$VARIANT" = bench ]]; then
	grep -q 'magicnet_proc_scan_r1' "$OUT_DIR/symbols.txt" ||
		{
			echo "bench archive has no Rust1 alias symbol" >&2
			exit 1
		}
fi
echo "built $VARIANT for $TARGET -> $OUT_DIR/libmagicnet_proc_scan.a"