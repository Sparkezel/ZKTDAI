# Possible integration from Helwor's New-Hel-K

Source repo: **https://github.com/Helwor/New-Hel-K** — author/credit: **Helwor**
(widget collection for Zero-K players, 212 Lua files; each widget header carries
`author = "Helwor"` plus original authors where forked, e.g. Google Frog,
very_bad_soldier, versus666, wolas, xponen).

License note: New-Hel-K widgets are **GPL v2+**, and this repo is **GPL v2** — so even
verbatim porting is license-compatible; still, we port *algorithms* reimplemented in
Rust against our `springai` bindings (which already expose `WeaponDef::range()`,
`WeaponDef::shield_radius()`, `custom_params`, `Unit::max_range()`, etc.).

Status: **research only — nothing integrated into the AI yet** (to be prototyped
locally first). New-Hel-K is a LuaUI *widget* collection, not a skirmish AI, so the
value is in the algorithms, not the UI.

## What we found, by area

### 1. Builder idle tasks (richest area)
- `Widgets/SmartBuilders.lua` (3480 lines, "Smart Builders", mid 2021): idle builders
  get auto-assigned work — **reclaim weighted by real resource need**
  (`UpdateResources` computes needM/needE from team income/storage; reclaims metal
  only when metal-starved), **auto-assist nearby unfinished builds** (`UpdateOrder`,
  action states manual/reclaiming/repairing/listening/building), **fighting retreat**
  using enemy weapon ranges (`GetFightingOrder` + `Include/weap_ranges.lua`),
  cloaked-conjurer safety (`CheckSmartConjurer`: cloaked cons stay near base
  clusters), own-order protection via command tags (`IsOwnOrder`, `SB_INTERNAL`).
- `unit_tree_reclaim.lua`, `unit_unit_reclaimer.lua`, `cmd_fix_area_metal_reclaim.lua`
  (449 lines): auto-reclaim trees / units / metal fields around base.
- `unit_building_starter.lua` (Google Frog, 2008): nanoframe-only queueing
  (`buildProgress > 0.01` guard against double-starting in one gameframe).
- `cmd_fix_unfinished_build.lua` ("FinishIt", dec 2022): force-complete stalled
  nanoframes. `unit_ghostSite3.lua` (1485 lines): tracks placed-but-unbuilt ghosts.

### 2. Unit automatisation (combat micro)
`AutoManeuverOnPatrol.lua`, `unit_pursue_target.lua`, `unit_close_enemy_range.lua`,
`cmd_keep_target.lua`, `cmd_split_attack.lua`, `unit_bomber_fog_chase2.lua`,
`unit_transport_ai.lua`, `cmd_autoretreat_type.lua`, `cmd_bury_me.lua` (auto-bury
cloaked units), `unit_cloakfirestate2.lua`, `unit_decloak_range.lua`,
`cmd_fix_unreachable.lua`, `cmd_no_duplicate_orders.lua`, `cmd_fix_wait.lua`,
`FixSquadWait.lua`, `cmd_tweaks.lua`.

### 3. Terraforming
- `gui_lasso_terraform.lua` (2511 lines): the CMD_TERRAFORM sender. Param layout
  (cross-validates our `terraform.rs`): `[1]=type (1 level, 2 raise, 3 smooth, 4 ramp,
  5 restore), [2]=team, [3,4]=center, [5]=tag, [6]=loop, [7]=height, [8]=pointCount,
  [9]=constructorCount, [10]=volumeSelection`, points from `[11]`, then builderIds;
  sent as `CMD_TERRAFORM_INTERNAL` (39801) with SHIFT, plus per-builder 39737.
- `MoveAwayFromLevelBuild.lua` (243): on a LEVEL order, **moves builders off the dig
  site first** (the "move out of the way" pattern, here for terraform).
- `persistent_build_height2.lua` (4433) + `draw_placements.lua` (6598): persistent
  build heights — auto-**LEVEL the footprint** of placed structures.
- `terra_unit_handler2.lua` (327): terraunit lifecycle — auto-repair/assist the dig
  (same pattern as GBC's `CaptureTF`).

### 4. Enemy weapon range viewing (danger estimation)
- `Widgets/Include/weap_ranges.lua`: per-unitdef weapon range with ZK quirks —
  `customParams.truerange` > `nuke_coverage` > `wDef.range`; AoE override via
  `damageAreaOfEffect` for bombs / cornea / teleport (`_death$`, `_emp$`, `bogus`);
  shield radius when range ≤ 32; skip "fake" weapons; water-weapon split; per-weapon
  entries for ballistic drawing.
- `Utilities/engine_range_circles.lua` (192): **ballistic 2D range correction** for
  cannon weapons (`GetRange2DCannon(yDiff)`, gravity + heightBoostFactor) — arty/nuke
  range on hills.
- `gui_sensor_ranges_radar_preview.lua` (274): radar/jammer/sight preview from
  `ud.radarDistance` etc.
- `gui_attack_aoe.lua` (961), `cmd_newton_firezone.lua` (1043),
  `gui_outscreen_danger.lua`, `unit_close_enemy_range.lua` (671),
  `unit_decloak_range.lua`: danger-zone / close-enemy-range display when enemies are
  in LOS.

## What we can implement from it (ranked, mapped to our modules)

1. **Enemy range/threat model → new `crates/zktdai/src/ranges.rs`**: port
   `weap_ranges.lua` (truerange/nuke_coverage/AoE/shield/fake rules) + ballistic
   correction from `engine_range_circles.lua`; precompute max enemy range per UnitDef
   at Init. Feeds porc placement (don't build where enemy arty outranges), sabotage
   approach safety, threatmap (#7), radar/jam/sight ranges from
   `gui_sensor_ranges_radar_preview.lua` params.
2. **SmartBuilders idle logic → `planner.rs` Free pool / ecov2 job queue
   (`docs/ecov2.md`)**: needM/needE-weighted reclaim, auto-assist of nearby
   nanoframes, fighting retreat by enemy range, own-order protection, cloaked-con
   safety.
3. **Terraform site hygiene → `terraform.rs` / `fob.rs`**: move builders off a dig
   site before LEVEL (`MoveAwayFromLevelBuild.lua`), auto-repair the terraunit
   (`terra_unit_handler2.lua`, GBC `CaptureTF`).
4. **Auto-level pads for big structures → `planner.rs` big projects**: LEVEL the
   footprint of fusion/heavies/silo before building (simplified
   `persistent_build_height2` idea).
5. **Stalled-build recovery → planner job queue**: re-assign nanoframes with no
   builder and low progress (`cmd_fix_unfinished_build.lua`, `unit_ghostSite3.lua`).
6. **Combat micro → `army.rs` / `sabotage.rs`**: auto-bury (`cmd_bury_me.lua`),
   pursue/keep-target, decloak-range awareness (`unit_decloak_range.lua`).

## Gaps in our bindings to close for #1
Per-def weapon list iteration (UnitDef → weaponDef ids) and ballistic params
(projectileSpeed, gravity, heightBoost) — check `UnitDef_getWeapons` /
`WeaponDef_getProjectileSpeed` in `crates/springai-sys` and extend `weapondef.rs`
if missing.
