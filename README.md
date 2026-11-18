# ZKTDAI

A Rust skirmish AI for [Zero-K](https://zero-k.info) (Recoil/Spring engine), derived from
[Anarchid's zkgbai-rs](https://github.com/Anarchid/zkgbai-rs) (GPL-2.0; this repo is GPL-2.0 too —
see `LICENSE`).

ZKTDAI plays **offensive-defense**: it tries to hold about half the map, boxes the enemy in
early with raids so its focus stays on defense, porcs its perimeter with ever-stronger turrets
and terraform ditches, and sends surgical sabotage strikes at undefended enemy critical targets.
Offense runs on riot-skirm armies, assault armies, raider detachments and artillery groups.
See [`docs/design.md`](docs/design.md) for the full doctrine and where each piece lives.

It is a native engine AI: a plain `cdylib` speaking the engine's C Skirmish AI interface
(`init` / `release` / `handleEvent`), shipped the same way CircuitAI is — a folder with
`libSkirmishAI.so` / `SkirmishAI.dll`, `AIInfo.lua` and `AIOptions.lua` under
`AI/Skirmish/ZKTDAI/<version>/`.

## Layout

| Path | What |
|------|------|
| `crates/springai-sys` | Raw FFI bindings to the engine's C AI interface (pre-generated, no libclang needed). Vendored from zkgbai-rs. |
| `crates/springai` | Safe Rust wrapper: `Copy` handles for units/defs, engine queries, orders, decoded events, and the `export_ai!` macro (panic containment at the FFI boundary, multiple AI instances per process, re-entrancy queue). Vendored from zkgbai-rs, with `Map::resource_map_spots` / `Map::extractor_radius` added. |
| `crates/checkpoint`, `crates/checkpoint-derive` | Save-state (de)serialization used by the wrapper's handles. Vendored from zkgbai-rs. |
| `crates/zktdai` | The AI itself: `ai.rs` (event dispatch), `defs.rs` (unit-def cache + roles), `territory.rs` (map control / front line), `economy.rs` (opening + mex expansion), `porc.rs` (perimeter turrets + terraform), `sabotage.rs` (surgical strikes), `army.rs` (production mix + combat roles), `data/AIInfo.lua`, `data/AIOptions.lua`. |
| `tools/package.sh` | Build + lay out `dist/AI/Skirmish/ZKTDAI/<version>/` for the current platform. |
| `tools/dll-smoke` | Loads a built library outside the engine and checks its FFI contract (exports, panic containment). Vendored from zkgbai-rs. |
| `reference/zkgbai-rs/` | **Git-ignored** full copy of upstream zkgbai-rs, for porting modules. Refresh with: `rm -rf reference/zkgbai-rs && git clone --depth 1 https://github.com/Anarchid/zkgbai-rs reference/zkgbai-rs` |
| `docs/design.md` | The doctrine this AI implements, mapped to modules and to the upstream files to port from. |

## Build

Requires Rust ≥ 1.88 (edition 2024). Pure Rust — no C++ toolchain needed for this workspace.

```sh
cargo build --release -p zktdai      # target/release/libzktdai.so (Linux)
cargo test                           # unit tests of the vendored engine layer
```

FFI smoke test outside the engine (loads the built library and drives its entry points with an
empty callback table; the library must contain the resulting panics and report error codes):

```sh
cargo build --release -p zktdai
cargo run --manifest-path tools/dll-smoke/Cargo.toml -- target/release/libzktdai.so
```

## Package (install into Zero-K)

```sh
tools/package.sh        # -> dist/AI/Skirmish/ZKTDAI/<version>/
```

Copy that folder into the game's `AI/Skirmish/ZKTDAI/<version>/` (next to `CircuitAI`), then
pick **ZKTDAI** in the skirmish setup. On Windows, build with MSVC (`cargo build --release -p
zktdai`) and `package.sh` copies `zktdai.dll` as `SkirmishAI.dll`.

## Headless matches (the arena harness)

`reference/zkgbai-rs/tools/arena/` has the match harness. It is bash and expects a Linux
layout, so on Windows run it under **WSL2** (Windows 10 Home is fine: version 1903+, build
18362+, VT-x/AMD-V enabled in BIOS, Virtual Machine Platform on; check with `wsl -l -v` and
`wsl --set-version Ubuntu 2` if your distro is still WSL1). You need:

- a Linux build of the engine at `~/.spring/engine/linux64/<version>/spring-headless`,
- the game archive `Zero-K <version>.sdz` in `~/.spring/games/`,
- maps in `~/.spring/maps/` (default map: `Comet Catcher Redux v3.1`),
- Rust in WSL: `cargo build --release -p zktdai`.

Then, from the repo root in WSL:

```sh
# one match, ZKTDAI vs an installed AI (e.g. CircuitAI):
reference/zkgbai-rs/tools/arena/match.sh \
    -m "Comet Catcher Redux v3.1" -g "Zero-K <version>" \
    "ZKTDAI@$PWD/target/release/libzktdai.so?porc=true&sabotage=true" CircuitAI
# prints: RESULT winner=... frame=... panics=... errors=... dir=...

# a series over the map pool, in parallel:
reference/zkgbai-rs/tools/arena/series.sh -n 2 -j 4 -P 40000 -o out \
    "ZKTDAI@$PWD/target/release/libzktdai.so" CircuitAI
```

AI options go in the spec after `?` (`territory_share=50&push_size=12&raiding=true&porc=true&sabotage=true`);
see `crates/zktdai/data/AIOptions.lua`.

## Attribution

The engine layer (`crates/springai-sys`, `crates/springai`, `crates/checkpoint*`) and
`tools/dll-smoke` are vendored from [zkgbai-rs](https://github.com/Anarchid/zkgbai-rs)
(GPL-2.0-only). The AI crate `crates/zktdai` is new work for this repo.
