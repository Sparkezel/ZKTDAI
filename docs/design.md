# ZKTDAI doctrine

Offensive-defense. The AI's goals, in priority order:

1. **Hold ~50% of the map** (`territory_share` option, default 50). Territory is measured as
   the share of the map's metal spots with a friendly mex on them (`territory.rs`). Below the
   target, the economy keeps expanding to the nearest free spot; at or above it, expansion
   stops and the army turns fully offensive (`push_size`).
2. **Box the enemy in early.** Raider detachments harass the enemy perimeter from the opening
   so its focus is spent fending off raids instead of attacking our economy (`raiding` option,
   `army.rs` `Role::Raider`).
3. **Secure our perimeter with ever-stronger turrets**, sited along the front line, with the
   tier escalating with metal income: `turretlaser` → `turretriot` → `turretgauss` →
   `turretheavylaser` (plus AA `turretaalaser` when the enemy shows air), and **terraform
   ditches** dug across vehicle approaches through Zero-K's `unit_terraform.lua` gadget
   (`porc` option, `porc.rs`).
4. **Sabotage.** When enemy critical targets are spotted (mexes, factories, fusion, nukes, …)
   and no armed enemy mobile is near them, send a surgical strike — raiders/cloaked units for
   close targets, artillery or larger groups for far ones (`sabotage` option, `sabotage.rs`).
5. **Offense** runs on four tools: riot-skirmishers (`Role::Skirm`), assault armies
   (`Role::Assault`), raider detachments (`Role::Raider`) and artillery groups (`Role::Arty`)
   (`army.rs`). Skirm/assault hold the front until `push_size` combat units are gathered, then
   push the enemy base together.

## Where each piece lives

| Doctrine piece | Module | Port from (in `reference/zkgbai-rs/`, git-ignored) |
|---|---|---|
| Map control, front line, 50% target | `crates/zktdai/src/territory.rs` | `crates/zkgbai/src/graph/` (metal-spot graph, Delaunay), `crates/terrain/` (region/chokepoint model — needs the `zkta` dep + C++ toolchain; not vendored) |
| Opening + mex expansion + energy | `crates/zktdai/src/economy.rs` | `crates/zkgbai/src/economy/` (worker tasks, grid, geo, colonize) |
| Perimeter porc + terraform | `crates/zktdai/src/porc.rs` | `crates/zkgbai/src/economy/porc_policy.rs`, `fortify.rs`, `terraform.rs`, `crates/terrain/src/fortify.rs` (site scoring, ditch value, chokepoints) |
| Sabotage / surgical strikes | `crates/zktdai/src/sabotage.rs` | `crates/zkgbai/src/military/strikes/` (opportunities, prediction, evasion), `airstrikes/`, `handlers/misc/sappers/` (cloaked strikes) |
| Raiders / skirm / assault / artillery | `crates/zktdai/src/army.rs` | `crates/zkgbai/src/military/handlers/{raider,squad,precision}.rs`, `military/taskforce/` (army composition doctrine), `military/fightsim/` (when to fight vs retreat) |
| Enemy tracking, threat, AA-from-bearings | (partly in `territory.rs` / `sabotage.rs`) | `crates/zkgbai/src/military/enemy.rs`, `intel.rs`, `radar.rs` |

## Development loop

1. `cargo build --release -p zktdai` + `cargo test` + `tools/dll-smoke` (see README).
2. Headless matches in WSL2 with `reference/zkgbai-rs/tools/arena/{match,series}.sh` against
   CircuitAI and self-play mirrors; read `infolog.txt` (`[zktdai]`, `[army]`, `[porc]`,
   `[sabotage]` lines) and the `RESULT` line.
3. Port a module at a time from `reference/zkgbai-rs/`; keep the module boundaries above so
   behavior stays readable.
