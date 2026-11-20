# Headless game runner (`tools/headless/`)

Runs real Zero-K games headless on your machine — no WSL required on Windows
(`spring-headless.exe` ships in the win64 engine zip) and no display needed
either way. Used for the v1-vs-CircuitAI benchmark (issue #2) and for
reproducing in-game behavior with full infologs.

## Quick start

Windows (PowerShell, from the repo root):

```powershell
# local build (build the dll first:  tools\package.sh --cross)
powershell -File tools\headless\run.ps1 -Games 3 -TraceEvents -PostIssue 2

# or install a CI artifact instead:
#   gh run download <run-id> -n ZKTDAI-0.2.0 -D C:\tmp\zktdai-artifact
#   powershell -File tools\headless\run.ps1 -AiZip C:\tmp\zktdai-artifact\ZKTDAI-0.2.0.zip -Games 3
```

To use an existing Zero-K install (engine + game + maps) **read-only** — nothing is
written into the install; the engine runs with `-write-dir <WorkDir>` so the AI copy,
`script.txt`, `infolog.txt` and `out/` all live in `<WorkDir>` only:

```powershell
powershell -File tools\headless\run.ps1 -ZkInstall "D:\...\steamapps\common\Zero-K" -Games 1 -TraceEvents
```
(`-Opp` auto-falls back to an installed CircuitAI version, e.g. `stable`.)

WSL2 / Linux:

```bash
cargo build --release -p zktdai          # produces libzktdai.so
tools/headless/run.sh GAMES=3 TRACE=1 POST_ISSUE=2
```

Everything is cached under the work dir (`~/.zktdai-headless` or
`%USERPROFILE%\.zktdai-headless`), so repeat runs only re-download nothing.
Results (per-game result, frames, exit code, console log, full infolog) land
in `<work>/out/`; `-PostIssue N` / `POST_ISSUE=N` comments a summary onto the
issue via `gh`.

## What it sets up

```
<work>/
  engine.../spring-headless[.exe]   from https://zero-k.info/engine/{win64|linux64}/<ver>.zip
  engine.../springsettings.cfg      SpringData = <work>
  games/zk.sdp                      latest zk:stable package from repos.springrts.com
  maps/<map>.sd7                    from https://zero-k.info/content/maps/<map>.sd7
  AI/Skirmish/ZKTDAI/<version>/     AIInfo.lua, AIOptions.lua, libSkirmishAI.so / SkirmishAI.dll
  script.txt                        generated from script.txt.template
  infolog.txt                       per-game infolog (copied to out/infolog-gameN.txt)
```

## Knobs (env vars for run.sh, parameters for run.ps1)

| knob | default | meaning |
|---|---|---|
| `ENGINE_VER` / `-EngineVer` | `2026.07.04` | engine version (zero-k.info/engine/{win64,linux64}/<ver>.zip) |
| `MAP_NAME` / `-MapName` | `Altored Divide Bar Remake 1.6` | map name as written in script.txt |
| `MAP_FILE` / `-MapFile` | derived (lowercase, spaces→`_`, dots kept, `.sd7`) | map archive file on zero-k.info |
| `GAME_TYPE` / `-GameType` | auto from the sdp's modinfo.lua | `GameType` in script.txt |
| `GAME_TAG_URL` / `-GameTagUrl` | `https://repos.springrts.com/zk/tags/zk:stable` | rapid tag listing the game package |
| `OPP` / `-Opp` | `CircuitAI:Normal` | opponent AI as `short:version` (e.g. `ZKTDAI:0.2.0` for self-play) |
| `GAMES` / `-Games` | 1 | number of games |
| `AI_ZIP` / `-AiZip` | local build | CI artifact zip/URL with `AI/Skirmish/ZKTDAI/...` layout |
| `AI_DIR` / `-AiDir` | — | loose dir with AIInfo.lua, AIOptions.lua, libSkirmishAI.so/SkirmishAI.dll |
| `VERSION` / `-Version` | from crates/zktdai/Cargo.toml | AI version directory |
| `TRACE` / `-TraceEvents` | off | run with `ZKTDAI_TRACE_EVENTS=1`: the AI logs every non-update event (and every 300th update) for the first minute — this tells us whether `unitCreated`/`unitIdle` reach the AI, the key diagnostic for the v0.1 idle game |
| `STARTPOS` / `-StartPos` | 2 (chosen) | start position mode; AIs send their start via `send_start_position` |
| `SPEED` / `-Speed` | 1 | sim speed multiplier, written to `[MODOPTIONS] MinSpeed/MaxSpeed` in script.txt (e.g. `10` for 10x; same mechanism as zkgbai's arena `-s`) |
| `PORT` / `-Port` | 8452 + pid-derived | host UDP port (`IsHost=1; HostIP=127.0.0.1` in script.txt makes the engine self-host; set distinct ports for parallel runs) |
| `CPUS` / `-CpuMask` | — (engine default) | pin the engine process to its own core(s): `taskset -c` list on Linux (`CPUS=4,5`), hex affinity mask on Windows (`-CpuMask 30`); give each parallel run a distinct port and mask |
| `MAXUNITS` / `-MaxUnits` | 2000 | per-team unit cap |
| `TIMEOUT_SEC` / `-TimeoutSec` | 1800 | per-game timeout |
| `POST_ISSUE` / `-PostIssue` | 0 | gh issue number to comment results on |
| `EXTRA_ARGS` / `-ExtraArgs` | — | extra args for spring-headless |
| `ZkInstall` / `-ZkInstall` | — | existing Zero-K install dir; engine/game/map come from it and all writes go to `WorkDir` via `-write-dir` (the install is never modified) |

## Interpreting results

- Result is parsed from the infolog's `Game Over: Allyteam N has met the game
  over conditions.` line — ZKTDAI is allyteam 0, so "Allyteam 1" in that line
  means ZKTDAI **won**.
- With `TRACE=1`, grep the infolog for `\[zktdai\] event` to see exactly which
  engine events arrived (e.g. `event unitCreated f=0`, `event unitIdle f=0`).
- Our AI's own decisions are logged as `[zktdai]`, `[army]`, `[war_chest]`,
  `[porc]`, `[planner]`, `[fob]`, `[sabotage]` lines.

## Troubleshooting

- `spring-headless` missing shared libraries on WSL2: `sudo apt install
  libsdl2-2.0-0 libopenal1 libgl1 libxcursor1 libxinerama1` (as needed).
- Game not found (`GameType` mismatch): pass `-GameType "Zero-K v1.14.10.3"`
  explicitly — the auto-detect reads `modinfo.lua` from the sdp.
- Map 404: pass `-MapFile` with the exact archive name, e.g.
  `altored_divide_bar_remake_1.6.sd7`.
- The AI library must be named exactly `libSkirmishAI.so` (Linux) /
  `SkirmishAI.dll` (Windows) inside `AI/Skirmish/ZKTDAI/<version>/`; the
  engine ignores other names.
