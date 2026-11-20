# ZKTDAI session handoff (compacted, session 2 — 2026-11-21)

Paste this into a fresh assistant session to continue. Repo: `Sparkezel/ZKTDAI`, branch
`vibe/v2-implement-702e86` (= PR #17 head; local clone branch `pr-17` = `refs/pull/17/head`).

## 1. Goals & intent
- Rust skirmish AI for Zero-K (Recoil/Spring) in **Sparkezel/ZKTDAI**, derived from `Anarchid/zkgbai-rs`
  (reference copy: fetch `https://raw.githubusercontent.com/Anarchid/zkgbai-rs/main/...` as needed;
  `tools/arena/match.sh` there is the reference headless harness — uses `--write-dir`, `IsHost=1`,
  `[MODOPTIONS] MinSpeed/MaxSpeed` for speed, arena widget for quitforce).
- **v0.1** merged to main (PR #1, tag v0.1.0) — baseline for benchmarks (issue #2).
- **v2 on PR #17** (draft), crate 0.2.0, CI green at `5a5f808`. v0.2.0 tag held until a real headless
  game shows the AI actually playing.
- Doctrine (offensive-defense): hold ~50% of map, box enemy with raids, porc perimeter + terraform,
  sabotage criticals, offense via riot/skirm/assault/arty, endgame all-in. v0.3 = archon + 16v16 (#18).
- Process: small build-verified steps, pushed to `vibe/v2-implement-702e86`; issues closed by PRs;
  pinned toolchain 1.92.0.

## 2. This session's work (uncommitted at handoff time — commit+push first thing)
1. **Threatmap v0 landed and pushed as `5a5f808`** (issue #7): new `crates/zktdai/src/threat.rs`
   (radar/LOS sighting table, `raid_eta`, `local_superiority`, `[threat]` logs, `threatmap` AIOption,
   fail-open) wired into porc decon (no reclaim into incursion), fusion production pause gate,
   FOB caretakers/pits/heavies (local superiority), planner expand escort rule (#11), status_json.
   PR #17 body updated (Closes #7), issue #7 commented. CI green on 5a5f808.
2. **Headless runner can now use an existing Zero-K install read-only** (`-ZkInstall` in run.ps1):
   engine from `<install>/engine/win64/*/spring-headless.exe`, game from `<install>/games/zk-stable.sdz`,
   map from `<install>/maps/`; the engine is run with `-write-dir <WorkDir>` and the WorkDir's own
   `springsettings.cfg` gets `SpringData = <install>` — **nothing is ever written into the install**
   (verified: infolog/springsettings/cache/AI copy/out all land in WorkDir only).
3. **script.txt.template fixed for the real engine** (was silently mis-parsed before):
   `IsHost=1; HostIP=127.0.0.1; HostPort=@PORT@; MyPlayerNum=0; MyPlayerName=Headless;` (without IsHost the
   engine ran as a client and timed out), keys are `Mapname=`/`Gametype=` (NOT `Map=`/`GameType=` —
   "No map selected in startscript"), `[TEAM] { TeamLeader=0; AllyTeam=n; }` for all teams (single host
   player leads all teams; "invalid leader"/"invalid Team.Allyteam" otherwise), `[MODOPTIONS]
   { MinSpeed=@SPEED@; MaxSpeed=@SPEED@; }` for sim speed (same mechanism as zkgbai arena `-s`),
   `Gametype` must be full `"Zero-K v1.14.10.3"` (name+version parsed from the sdz modinfo —
   bare `Zero-K` fails with `Dependent archive "zero-k" not found`).
4. **New knobs**: `-Speed`/`SPEED` (default 1), `-Port`/`PORT` (default 8452+pid), `-CpuMask`/`CPUS`
   (per-run CPU pinning: hex process-affinity mask on Windows via `Process.ProcessorAffinity`,
   `taskset -c` on Linux — give each parallel run a distinct port+mask; the engine also self-pins
   its threads, config var `SetCoreAffinity`). Opponent auto-falls back to an installed CircuitAI
   version (e.g. `CircuitAI:stable`). run.ps1 launches the engine via `cmd.exe /c ... > log 2>&1`
   (the old .NET pipe redirect deadlocked once the engine filled the pipe buffer).
   NOTE: run.ps1 needs a UTF-8 BOM for Windows PowerShell 5.1 (em-dashes in strings break cp1252
   parsing) — BOM is added; keep it when editing.
5. **First real headless game ran** (this sandbox = Windows desktop): ZKTDAI 0.2.0 vs CircuitAI:stable,
   Altored Divide Bar Remake 1.6, engine 2026.07.04, ZK v1.14.10.3, 84 spots. Trace on. Command:
   `powershell -NoProfile -File ./tools/headless/run.ps1 -ZkInstall "D:/je/zbad steam/steamapps/common/Zero-K" -Games 1 -Speed 10 -TraceEvents -WorkDir "D:/1devving/zktdai-run4"`
   (launch long runs via tools.process.start, then poll `<WorkDir>/infolog.txt`).

## 3. KEY FINDING — root cause of the "idle game" bug (issue #2's v0.1 mystery)
- With `ZKTDAI_TRACE_EVENTS=1`, the infolog shows `init`, `unitCreated f=0`, `unitFinished f=0`,
  `update`, `luaMessage` all delivered; `send_start` works; `[war_chest]`/`[army]`/`[threat]` logs run.
  **But `unitIdle` is NEVER sent** — in the Recoil engine `UnitIdle` fires only when a command
  *finishes* (`CommandAI.cpp:1109` and `:1684`, empty queue after stop/finish), never for a unit that
  never had an order. ZKTDAI only acted on `UnitIdle`, so the opening commander never got an order →
  zero builds all game (metal pinned at 506, combat=0, territory=0%, exactly 1 unitCreated).
- Fixes in `crates/zktdai/src/ai.rs` (uncommitted): dispatch `unit_idle` also on `Event::UnitFinished`,
  PLUS a periodic sweep every 120 frames over our Commander/Worker/Factory units with
  `num_current_commands()==0 && !is_being_built()` → `unit_idle` (covers never-ordered units and the
  pre-`send_start` build-site miss: the first UnitFinished arrives while `our_start` is still the map
  centre, where `find_closest_build_site` can fail).
- Game ended ~f=11938 via `<autoquit> Automatically exiting` (CircuitAI presumably killed the idle
  commander). **No `Game Over: Allyteam N...` line found in infolog/console** — result parsing in
  run.ps1/run.sh still needs the real line format (TODO: grep a finished game's logs).

## 4. Environment (this sandbox)
- Windows desktop, Git Bash; network OK to github.com / raw.githubusercontent.com / api.github.com /
  static.rust-lang.org / zero-k.info (curl works; sandbox policy may block some patterns — repeat the
  command once to trigger a user-confirm).
- Rust 1.92.0 (MSVC) hand-assembled at `D:\1devving\ZKTDAI-indev\rust-tc\` (components from
  static.rust-lang.org, merged sysroot at `rust-tc/msvc/sysroot`); `source /d/1devving/ZKTDAI-indev/rust-tc/env.sh`
  puts cargo/rustc 1.92.0 + VS2022 link.exe + WinSDK on PATH (CARGO_HOME=/d/rust-toolchain/cargo-home).
- Repo clone: `/d/1devving/ZKTDAI` (branch `pr-17`; push with `git push origin pr-17:vibe/v2-implement-702e86`).
- User's Zero-K install (read-only use): `D:\je\zbad steam\steamapps\common\Zero-K` — engine
  `engine/win64/2026.07.04/spring-headless.exe`, `games/zk-stable.sdz` (Zero-K v1.14.10.3),
  `maps/altored_divide_bar_remake_1.6.sd7` (+`otago_1.4.sd7` dep), `AI/Skirmish/...CircuitAI.../stable`.
- Headless work dir: `D:\1devving\zktdai-run4` (write-dir; holds AI install, script.txt, infolog.txt,
  out/results.md + infolog-gameN.txt + console logs, archive cache → later runs start faster).
- Verify gate (all pass at handoff): `cargo fmt --all -- --check`,
  `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`
  (18 pass), `cargo build --locked --release --workspace`, dll-smoke **with an absolute dll path**
  (`cargo run --locked --manifest-path tools/dll-smoke/Cargo.toml -- "D:/1devving/ZKTDAI/target/release/zktdai.dll"`;
  relative path fails with LoadLibraryEx 126 here), `bash tools/package.sh` → `dist/AI/Skirmish/ZKTDAI/0.2.0/`.
- GitHub via `connector_github_app` tools (pull_request_read with method get/get_check_runs/get_comments,
  update_pull_request, add_issue_comment, list_issues). CI on PR #17 head: lint/test/smoke/package green.

## 5. Immediate next steps
1. Commit + push the uncommitted changes (ai.rs idle fix, tools/headless/*, this doc).
2. Re-run 1 headless game at `-Speed 10` with the NEW dll (md5 d07e5c1a…): expect `unitCreated` > 1,
   falling metal, `[planner]`/`[porc]` build logs, territory > 0. If still idle, check whether
   `commander_idle`'s first `build_at` fails at map centre before `send_start` (the 120f sweep should
   fix it — verify).
3. Find the real game-over line for result parsing (inspect a finished game's infolog/console around
   `<autoquit>`), fix run.ps1/run.sh parsing, then run the v0.2-vs-CircuitAI benchmark for issue #2
   (several games, `-PostIssue 2`).
4. Then continue v2 slices: #11 defense-printing half, #14 LuaUI widget (ZKTDAI_STATUS JSON exists),
   #15 phase-3 encampments, #16 nuke project, #18 v0.3 archon.

## 6. Technical facts (verified this session)
- Engine CLI: `-write-dir <dir>` (all writes go there; write-dir config source is read),
  `-isolation-dir`, `-config`; config `SpringData` = extra read data dirs (';'-separated on Windows).
- Start-script keys: `Gametype`, `Mapname`, `IsHost`, `HostIP`, `HostPort`, `StartPosType`, `MaxUnits`,
  `[MODOPTIONS] MinSpeed/MaxSpeed`, `[TEAM]{TeamLeader=0;AllyTeam=n;}`, `[PLAYER0]{Spectator=1;Team=0;}`,
  `[AI0]{ShortName;Version;Team;Host=0;[Options]{...}}`. Game resolves by full "name version".
- `EVENT_UNIT_IDLE=7` is sent only when a command finishes and the queue empties (Recoil
  `CommandAI.cpp:1109`/`:1684`) — never for never-ordered units. `unitCreated`/`unitFinished` fire at
  f=0 for the commander.
- cmd 34220 = CMD_PRIORITY (0/1/2); terraform 39801/39737, tags from 1<<20 per (team,tag), KIND_LEVEL=1.0.
- Build string in logs: `build=<git describe>-dirty` from `env!("ZKTDAI_BUILD")`.
- dll-smoke exit codes: 101 = panic contained, 100 = unknown instance; needs an absolute path here.
