#!/usr/bin/env bash
# Build the AI and lay it out the way Zero-K distributes skirmish AIs:
#   <out>/AI/Skirmish/ZKTDAI/<version>/
#       libSkirmishAI.so   Linux x86_64 (glibc 2.17 when --cross)
#       SkirmishAI.dll     Windows x86_64 (--cross only)
#       AIInfo.lua, AIOptions.lua
#
# usage: tools/package.sh [-v <version>] [-o <out-dir>] [--cross]
#   default     native build for this machine with plain cargo
#   --cross     portable builds via cargo-zigbuild (Linux glibc 2.17 + Windows);
#               needs tools/setup-toolchain.sh to have run once (CI runs it too)
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
version=""
out="$root/dist"
cross=0
while [ $# -gt 0 ]; do
    case $1 in
        -v) version=$2; shift 2 ;;
        -o) out=$2; shift 2 ;;
        --cross) cross=1; shift ;;
        *) sed -n '2,13p' "$0"; exit 2 ;;
    esac
done
version=${version:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/zktdai/Cargo.toml" | head -1)}
ai="$out/AI/Skirmish/ZKTDAI/$version"
mkdir -p "$ai"
cd "$root"
target="${CARGO_TARGET_DIR:-$root/target}"

if [ "$cross" = 1 ]; then
    export PATH="$root/.toolchains/venv/bin:$root/.toolchains/bin:$PATH"
    command -v cargo-zigbuild > /dev/null || { echo "run tools/setup-toolchain.sh first (or drop --cross)" >&2; exit 1; }
    cargo zigbuild --release -p zktdai --target x86_64-unknown-linux-gnu.2.17
    cargo zigbuild --release -p zktdai --target x86_64-pc-windows-gnu
    cp "$target/x86_64-unknown-linux-gnu/release/libzktdai.so" "$ai/libSkirmishAI.so"
    cp "$target/x86_64-pc-windows-gnu/release/zktdai.dll" "$ai/SkirmishAI.dll"
else
    cargo build --release -p zktdai
    case "$(uname -s)" in
        Linux)  cp "$target/release/libzktdai.so" "$ai/libSkirmishAI.so" ;;
        Darwin) cp "$target/release/libzktdai.dylib" "$ai/libSkirmishAI.dylib" ;;
        MINGW*|MSYS*|CYGWIN*) cp "$target/release/zktdai.dll" "$ai/SkirmishAI.dll" ;;
        *) echo "unsupported platform: $(uname -s)" >&2; exit 2 ;;
    esac
fi

sed "s/@VERSION@/$version/" crates/zktdai/data/AIInfo.lua > "$ai/AIInfo.lua"
cp crates/zktdai/data/AIOptions.lua "$ai/AIOptions.lua"
echo "$ai"
