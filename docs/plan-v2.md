# ZKTDAI v2 plan — offensive-defense doctrine, phase by phase

v1 (PR #1, branch `vibe/zktdai-seed-702e86`) stays as-is for the baseline benchmark
against a normal-level CircuitAI. Everything below lands on later branches, measured
against that baseline with `reference/zkgbai-rs/tools/arena/series.sh` (same maps,
sides swapped, `RESULT winner=… panics=… errors=…` + infolog).

Design inputs: hold ~50% of the map aggressively, then creep; box the enemy in early;
perimeter porc with escalating, income-keyed tiers; terraformed pits/spires/trenches;
sabotage strikes on undefended critical targets; offense = riot-skirm + assault +
raiders + artillery; a way to force the game to end.

## Phase 0 — baseline (no code)

- Run v1 vs CircuitAI (normal level) on the `tools/arena/maps.txt` pool, `-n 2` per map,
  sides swapped. Record win rate, avg game length, mex loss rate, panics/errors.
- This is the reference point every later phase must beat.

## Phase 1 — foundations (territory, factory, creep, endgame)

1. **Terrain-lite**: metal-spot graph (positions, adjacency by distance) in
   `territory.rs`; front line = midpoint of owned/enemy centroids, refined to the
   spot-graph frontier. Port ideas from `reference/zkgbai-rs/crates/zkgbai/src/graph/`.
   Full region/chokepoint model (`crates/terrain` + `zkta`, needs C++ toolchain in
   WSL) is optional here; keep a seam for it.
2. **Factory by terrain**: weigh the opening factory by what its units can reach from
   the chosen start box (movement tier of factory units vs start terrain; jump factory
   if penned in). Port from `economy/commander.rs` (`comreach`, `facpit`).
3. **Expansion policy**: below `territory_share` (50%) mexes are top priority
   (aggressive); above it, claim only spots that are cheap to hold (near existing
   porc, far from enemy centroid) at a trickle — the "slow, resolute creep".
   Port from `economy/conquest*.rs`.
4. **Endgame forcing**: if no enemy mex/factory has been seen on radar+LOS for T
   minutes (use shadow-spot accounting from `military/intel.rs` to avoid false
   triggers on hidden mexes), or game time exceeds a map-size-scaled limit, commit the
   whole army to the enemy base. Also a porc/escalation ramp so the game cannot stall.
5. **Threatmap v0**: radar/LOS enemy density per region of the spot graph + incursion
   timers (when a raid can reach each of our spots). Port from
   `crates/terrain/src/threat.rs`, `military/incursions.rs`.

## Phase 2 — porc v2 (income-keyed, conditional, self-cleaning)

Tier thresholds keyed to **metal income**, normalized per map (income per metal spot,
since bigger maps scale faster): opening ~6, mid ~40, endgame ~80 on small maps.

- **Lotus-first** (`turretlaser`, matched by `human_name()`) near mex clusters: one at
  the first cluster, more at forward/perimeter mexes as income grows.
- **Pickets** (`turretgauss`) as perimeter defense when enemy jumpbots/spiders are
  scouted; **Stardust** when the enemy is raider-heavy AND the map has chokepoints
  (needs the Phase-1 frontier/chokepoint read).
- **Stingers** by default once we hold the midline, sited in **40-elmo-deep pits**.
- **Raised Gauss on a ~140-elmo spire** when a fragile enemy factory (amph/jump/spider)
  is spotted. (Upstream pattern: `economy/porc_policy.rs` `heavyporc` — Lucifer/Desolator
  on spires/pillars.)
- **Reclaim obsolete turrets**: when the front has moved > N elmos toward the enemy,
  workers reclaim our turrets now behind it (with a safety check: not while enemies
  are near). Answers "reclaim turrets no longer needed".
- **Strategic ditches**: only across measured approaches; never block own vehicle
  routes or build sites (upstream: `economy/terraform.rs` route-preservation check,
  `crates/terrain/src/fortify.rs` chokepoint cost). Terraform mechanics: custom cmd
  **39801** to one builder (kind/team/centre/tag/area/height/points/builder-ids),
  **39737** to each builder with the same tag; the `unit_terraform.lua` gadget spawns
  `terraunit` helpers and the builders repair them; heightmap is written incrementally
  from spent metal (`DoTerraformUpdate` → `Spring.SetHeightMap`); track tags until done.
- **Storages** (`staticstorage`): stockpile 1500 metal + 300 headroom for stalls.
- **Engineer taskgroup**: constructors on high priority (ZK priority = custom cmd
  34220) that spend the stockpile to print defenses / repair forward units, with
  hysteresis so a raider poke doesn't trigger a turret that then gets cancelled.

## Phase 3 — heavy encampments (terraformed, shielded, morphing)

- At **40–50 income**: reserve ~500 metal; build a **Lucifer** (`turretantiheavy`) in a
  **60-elmo-deep trench**, connected to a **50e grid** (pylons — needs a small grid
  module), sited so it can range enemy territory.
- **Aspis** (`shieldshield`) area shields guard Lucifers against silo strikes (silo
  reach 3450 — needs enemy silo intel from `military/missiles/site.rs`).
- **Encampment pattern**: prebuild **Aegis** in a pit, or a wall in front; when the
  front moves past it, morph Aegis→Aspis (ZK morph = custom cmd 31210), reclaim the
  heavy turret, and restore the pit terrain after the morph (gadget restore/level).

## Phase 4 — offense and sabotage v2

- Army doctrines as squads: raider detachments (box-in/harass), riot-skirm armies and
  assault armies on the front, artillery groups staging behind it; push at
  `push_size` or when Phase-1 endgame triggers. Port from
  `military/handlers/{raider,squad,precision}.rs`, `military/taskforce/`.
- Sabotage v2: strike targets = spotted enemy critical statics **lacking defenses**
  (threatmap-aware, not a flat radius), delivered by raiders/cloaked units, air, or
  artillery by distance. Port from `military/strikes/`, `airstrikes/`.

## Verification per phase

- `cargo build --release -p zktdai && cargo test` + `tools/dll-smoke` (FFI contract).
- `series.sh` vs CircuitAI normal on the map pool; metrics: win rate, game length,
  mexes lost, army value at 10/15/20 min, panics/errors in the RESULT line; infolog
  tags `[territory] [porc] [sabotage] [army] [endgame]` for debugging.

## Adversarial review (of this plan)

1. **Porc vs expansion tension.** Lotus-first + storages + reserves all compete with
   mex/army metal in the opening. Mitigation: explicit per-phase budget split (e.g.
   porc may spend at most X% of income until `territory_share` is reached); otherwise
   the "aggressive first 50%" and "porc the perimeter" goals fight and both lose.
2. **Hardcoded income thresholds (6/40/80) break on map size.** Mitigation: normalize
   by metal spots (income/spot) or calibrate per map class in the Phase-0 baseline.
3. **Conditional tiers depend on intel we won't have early.** "Enemy uses jumpbots" or
   "fragile factory" requires scouting through fog; a rush could hit before we react.
   Mitigation: default to lotus+picket mix early; make conditions hysteretic (sticky
   for N minutes once triggered) so one scout doesn't flip the build order.
4. **Chokepoint detection for the Stardust rule needs real terrain analysis.** The
   spot-graph frontier is a poor chokepoint detector. Mitigation: Phase 1 ships
   without it (Stardust rule dormant until the terrain crate lands); don't fake it
   with a distance heuristic and call it chokepoints.
5. **Terraforming pits/spires/trenches is slow, metal-expensive, and interruptible**
   (gadget cancels digs when enemies approach; cost is paid over time via terraunit
   HP). A 140-elmo spire or 60-elmo trench in the open will be shelled mid-dig.
   Mitigation: only terraform behind the current front or under local superiority;
   track tag → builder → completion; re-issue on cancel (upstream `terraform.rs`
   does exactly this with `is_terraforming` + reorder after 30×30 frames).
6. **Stockpile 1500+300 is dead metal.** On small maps that's ~a third of a Lucifer
   doing nothing. Mitigation: reserve only after army composition is met, and spend
   it reactively (engineer taskgroup) rather than letting it sit.
7. **Reclaim-behind-the-front can feed the enemy.** Reclaiming takes time and exposes
   builders; if the front estimate is wrong (it will be, early), we reclaim into an
   enemy push. Mitigation: reclaim only turrets > M elmos behind the front, never
   while threatmap shows an incursion inbound, and reclaim nearest-first.
8. **Aegis→Aspis morph chain has failure modes**: morph can be interrupted, the
   reclaimed turret's wreck can block the site, and "restore the pit after morph"
   needs the original heightmap values kept with the tag. Mitigation: store
   pre-dig heights with the terraform tag; on morph, restore via gadget level cmd;
   if restore fails, leave it — a pit is usually fine.
9. **Endgame forcing can false-trigger.** "No enemy mex seen for T minutes" triggers
   vs cloaked/hidden mexes. Mitigation: use shadow-spot accounting (unaccounted
   enemy metal, `military/intel.rs`) plus a minimum game time, and never trigger on
   FFA-ish layouts without confirming the enemy actually has a base.
10. **Benchmark noise.** Small samples lie. Mitigation: fixed map pool, sides
    swapped, ≥2 games per map per phase, and compare medians not single runs; keep
    v1 branch as the control.
