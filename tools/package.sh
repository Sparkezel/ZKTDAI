#!/usr/bin/env bash
# Build the AI and lay it out the way Zero-K distributes skirmish AIs:
#   <out>/AI/Skirmish/ZKTDAI/<version>/
#       libSkirmishAI.so   Linux x86_64
#       libSkirmishAI.dylib macOS
#       SkirmishAI.dll     Windows x86_64
#       AIInfo.lua, AIOptions.lua
#
# usage: tools/package.sh [-v <version>] [-o <out-dir>]
#
# Builds natively for the current platform with plain cargo. For a portable
# Linux .so (old glibc) or a Windows DLL built from Linux, cross-compile with
# cargo-zigbuild the way reference/zkgbai-rs/tools/setup-toolchain.sh sets up,
# then copy the artifact here by hand.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
version=""
out="$root/dist"
while [ $# -gt 0 ]; do
    case $1 in
        -v) version=$2; shift 2 ;;
        -o) out=$2; shift 2 ;;
        *) sed -n '2,12p' "$0"; exit 2 ;;
    esac
done
version=${version:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/zktdai/Cargo.toml" | head -1)}
ai="$out/AI/Skirmish/ZKTDAI/$version"
mkdir -p "$ai"
cd "$root"

cargo build --release -p zktdai
target="${CARGO_TARGET_DIR:-$root/target}"
case "$(uname -s)" in
    Linux)  cp "$target/release/libzktdai.so" "$ai/libSkirmishAI.so" ;;
    Darwin) cp "$target/release/libzktdai.dylib" "$ai/libSkirmishAI.dylib" ;;
    MINGW*|MSYS*|CYGWIN*) cp "$target/release/zktdai.dll" "$ai/SkirmishAI.dll" ;;
    *) echo "unsupported platform: $(uname -s)" >&2; exit 2 ;;
esac

sed "s/@VERSION@/$version/" crates/zktdai/data/AIInfo.lua > "$ai/AIInfo.lua"
cp crates/zktdai/data/AIOptions.lua "$ai/AIOptions.lua"
echo "$ai"
