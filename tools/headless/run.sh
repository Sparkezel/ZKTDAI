#!/usr/bin/env bash
# ZKTDAI headless game runner (Linux / WSL2). Windows: use run.ps1.
#
# Downloads the Spring engine + Zero-K game + a map into a work dir, installs
# the ZKTDAI build, writes script.txt, and runs N AI-vs-AI games headless.
# Results land in <work>/out/results.md and can be posted to a GitHub issue.
# See README.md for details and all overridable knobs (env vars below).
#
# Example (v0.2 build from this repo vs CircuitAI Normal on Altored Divide):
#   tools/headless/run.sh GAMES=3 TRACE=1 POST_ISSUE=2
#
# Use a CI artifact instead of a local build:
#   AI_ZIP="https://github.com/Sparkezel/ZKTDAI/actions/artifacts/<id>/zip" tools/headless/run.sh
set -euo pipefail

ENGINE_VER="${ENGINE_VER:-2026.07.04}"
MAP_NAME="${MAP_NAME:-Altored Divide Bar Remake 1.6}"
MAP_FILE="${MAP_FILE:-}"                 # e.g. altored_divide_bar_remake_1.6.sd7 (auto-derived if empty)
GAME_TYPE="${GAME_TYPE:-}"               # auto-detected from the game's modinfo.lua if empty
GAME_TAG_URL="${GAME_TAG_URL:-https://repos.springrts.com/zk/tags/zk:stable}"
OPP="${OPP:-CircuitAI:Normal}"           # opponent AI as short:version
GAMES="${GAMES:-1}"
WORKDIR="${WORKDIR:-$HOME/.zktdai-headless}"
AI_ZIP="${AI_ZIP:-}"                     # CI artifact zip (AI/Skirmish/ZKTDAI/<ver>/... layout) or URL
AI_DIR="${AI_DIR:-}"                     # or a loose dir with AIInfo.lua, AIOptions.lua, libSkirmishAI.so
VERSION="${VERSION:-}"                   # AI version; default read from crates/zktdai/Cargo.toml
POST_ISSUE="${POST_ISSUE:-0}"            # gh issue number to comment the results on (0 = no)
TRACE="${TRACE:-0}"                      # 1 = run games with ZKTDAI_TRACE_EVENTS=1
STARTPOS="${STARTPOS:-2}"                # 0 random, 1 fixed, 2 chosen (AIs send start pos)
MAXUNITS="${MAXUNITS:-2000}"
TIMEOUT_SEC="${TIMEOUT_SEC:-1800}"
EXTRA_ARGS="${EXTRA_ARGS:-}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$WORKDIR/out"

for tool in curl unzip; do
    command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

if [[ -z "$VERSION" ]]; then
    VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO_ROOT/crates/zktdai/Cargo.toml" | head -n1)"
fi
if [[ -z "$MAP_FILE" ]]; then
    MAP_FILE="$(echo "$MAP_NAME" | tr '[:upper:]' '[:lower:]' | tr ' ' '_').sd7"
fi
OPP_SHORT="${OPP%%:*}"
OPP_VER="${OPP#*:}"

mkdir -p "$WORKDIR/downloads" "$WORKDIR/games" "$WORKDIR/maps" "$OUT"

echo "== engine $ENGINE_VER (linux64)"
ENGINE_ZIP="$WORKDIR/downloads/engine-$ENGINE_VER-linux64.zip"
if [[ ! -f "$ENGINE_ZIP" ]]; then
    curl -fsSL -o "$ENGINE_ZIP" "https://zero-k.info/engine/linux64/$ENGINE_VER.zip"
fi
if [[ -z "$(find "$WORKDIR" -name spring-headless -type f -print -quit 2>/dev/null)" ]]; then
    unzip -q -o "$ENGINE_ZIP" -d "$WORKDIR"
fi
ENGINE_BIN="$(find "$WORKDIR" -name spring-headless -type f -print -quit)"
ENGINE_DIR="$(dirname "$ENGINE_BIN")"
echo "   engine binary: $ENGINE_BIN"

echo "== game (rapid zk:stable)"
TAG="$(curl -fsSL "$GAME_TAG_URL")"
PKG="$(echo "$TAG" | grep -oE 'packages/[A-Za-z0-9._-]+\.sdp' | tail -n1)"
if [[ -z "$PKG" ]]; then echo "no .sdp found in $GAME_TAG_URL" >&2; exit 1; fi
GAME_SDP="$WORKDIR/games/zk.sdp"
if [[ ! -f "$GAME_SDP" ]]; then
    for base in "https://repos.springrts.com/zk/stable" "https://repos.springrts.com/zk"; do
        if curl -fsSL -o "$GAME_SDP" "$base/$PKG"; then break; fi
    done
    [[ -f "$GAME_SDP" ]] || { echo "failed to download $PKG" >&2; exit 1; }
fi
if [[ -z "$GAME_TYPE" ]]; then
    GAME_TYPE="$(unzip -p "$GAME_SDP" modinfo.lua 2>/dev/null | sed -n 's/^[[:space:]]*name[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' | head -n1)"
    [[ -n "$GAME_TYPE" ]] || GAME_TYPE="Zero-K"
fi
echo "   game: $GAME_TYPE ($PKG)"

echo "== map: $MAP_NAME ($MAP_FILE)"
MAP_SD7="$WORKDIR/maps/$MAP_FILE"
if [[ ! -f "$MAP_SD7" ]]; then
    curl -fsSL -o "$MAP_SD7" "https://zero-k.info/content/maps/$MAP_FILE"
fi

echo "== AI: ZKTDAI $VERSION"
AI_DEST="$WORKDIR/AI/Skirmish/ZKTDAI/$VERSION"
mkdir -p "$AI_DEST"
if [[ -n "$AI_ZIP" ]]; then
    case "$AI_ZIP" in
        http*) ZIP_LOCAL="$WORKDIR/downloads/ai.zip"; [[ -f "$ZIP_LOCAL" ]] || curl -fsSL -o "$ZIP_LOCAL" "$AI_ZIP"; AI_ZIP="$ZIP_LOCAL" ;;
    esac
    unzip -q -o "$AI_ZIP" -d "$WORKDIR"
    [[ -d "$AI_DEST" ]] || { echo "AI zip has no AI/Skirmish/ZKTDAI/$VERSION/" >&2; exit 1; }
elif [[ -n "$AI_DIR" ]]; then
    sed "s/@VERSION@/$VERSION/g" "$AI_DIR/AIInfo.lua" > "$AI_DEST/AIInfo.lua"
    cp "$AI_DIR/AIOptions.lua" "$AI_DEST/AIOptions.lua"
    cp "$AI_DIR/libSkirmishAI.so" "$AI_DEST/libSkirmishAI.so"
else
    LIB="$REPO_ROOT/target/release/libzktdai.so"
    [[ -f "$LIB" ]] || { echo "no local build at $LIB — build it or pass AI_ZIP=/AI_DIR=" >&2; exit 1; }
    sed "s/@VERSION@/$VERSION/g" "$REPO_ROOT/crates/zktdai/data/AIInfo.lua" > "$AI_DEST/AIInfo.lua"
    cp "$REPO_ROOT/crates/zktdai/data/AIOptions.lua" "$AI_DEST/AIOptions.lua"
    cp "$LIB" "$AI_DEST/libSkirmishAI.so"
fi
echo "   installed at $AI_DEST"

echo "== spring settings + script.txt"
printf 'SpringData = %s\n' "$(cd "$WORKDIR" && pwd -P | sed 's|\\|/|g')" > "$ENGINE_DIR/springsettings.cfg"
sed -e "s/@GAMETYPE@/$GAME_TYPE/g" -e "s/@MAP@/$MAP_NAME/g" -e "s/@STARTPOS@/$STARTPOS/g" \
    -e "s/@MAXUNITS@/$MAXUNITS/g" -e "s/@AI_NAME@/ZKTDAI $VERSION (1)/g" -e "s/@AI_SHORT@/ZKTDAI/g" \
    -e "s/@AI_VERSION@/$VERSION/g" -e "s/@OPP_NAME@/$OPP_SHORT $OPP_VER (1)/g" -e "s/@OPP_SHORT@/$OPP_SHORT/g" \
    -e "s/@OPP_VERSION@/$OPP_VER/g" \
    "$REPO_ROOT/tools/headless/script.txt.template" > "$WORKDIR/script.txt"

echo "== running $GAMES game(s)"
RESULTS="$OUT/results.md"
echo "| game | map | opponent | result | frames | exit |" > "$RESULTS"
echo "|---|---|---|---|---|---|" >> "$RESULTS"
ENV_PREFIX=()
[[ "$TRACE" == "1" ]] && ENV_PREFIX+=(ZKTDAI_TRACE_EVENTS=1)
for i in $(seq 1 "$GAMES"); do
    rm -f "$WORKDIR/infolog.txt"
    set +e
    (cd "$WORKDIR" && timeout "$TIMEOUT_SEC" env "${ENV_PREFIX[@]}" "$ENGINE_BIN" $EXTRA_ARGS > "$OUT/console-$i.log" 2>&1)
    CODE=$?
    set -e
    cp "$WORKDIR/infolog.txt" "$OUT/infolog-game$i.txt" 2>/dev/null || true
    LOG="$OUT/infolog-game$i.txt"
    OVER="$(grep -m1 -i 'game over' "$LOG" 2>/dev/null || echo 'no game-over line')"
    # ZKTDAI is allyteam 0: if allyteam 1 met the game-over conditions, we won
    if echo "$OVER" | grep -qi 'Allyteam 1'; then RES="WIN"; elif echo "$OVER" | grep -qi 'Allyteam 0'; then RES="LOSS"; else RES="?"; fi
    FRAMES="$(grep -oE 'f=[-0-9]+' "$LOG" 2>/dev/null | tail -n1 || echo '?')"
    echo "game $i: result=$RES ($OVER) $FRAMES exit=$CODE"
    echo "| $i | $MAP_NAME | $OPP | $RES | $FRAMES | $CODE |" >> "$RESULTS"
done

echo "== results"
cat "$RESULTS"
echo "infologs: $OUT/infolog-game*.txt"

if [[ "$POST_ISSUE" != "0" ]] && command -v gh >/dev/null; then
    {
        echo "## Headless run — ZKTDAI $VERSION vs $OPP on $MAP_NAME (engine $ENGINE_VER)"
        echo
        cat "$RESULTS"
        echo
        echo "infologs: see run output artifacts (out/infolog-game*.txt)"
    } > "$OUT/issue-comment.md"
    gh issue comment "$POST_ISSUE" --repo Sparkezel/ZKTDAI --body-file "$OUT/issue-comment.md" \
        && echo "posted results to issue #$POST_ISSUE" || echo "gh issue comment failed (results still in $RESULTS)"
fi
