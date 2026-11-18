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

## Round 1 refinements (from review discussion — incorporated)

1. Porc budget: minimal at start — hold ~10% of the starting metal capacity (≈50 of
   500) in reserve; expand the reserve as the economy grows. Note this is two axes:
   a spend cap (porc ≤ X% of income) and a floor (never spend below the reserve).
2. Thresholds are flexible, not fixed: e.g. enemy heavy spotted → queue a Lucifer
   already at ~40 m/s; the porc budget rises while the job is in progress and drops
   back when it completes (or is cancelled — release the budget on
   `CommandFinished`/`UnitDestroyed` of the build).
3. Initial raiders double as scouts: locate the enemy, detect the opener, make quick
   counters (commander-level units, not a factory swap), adjust to early enemy comp.
   Role split needed so scouts don't consume the whole raider pool.
4. Chokepoint-dependent rules stay dormant until the terrain model lands.
5. Terraform is cheap for small pads (raising a Desolator to a 160-elmo spire ≈ 12%
   of its cost) — worth doing. Large jobs split into sub-jobs and workers divide
   between them; mitigate with frontline caretakers. A naked caretaker is only placed
   under local superiority (QRF close / army on the front / no enemy on radar+LOS);
   in contested areas, caretakers go into 40-elmo pits or behind walls. Experiment
   (randomised A/B via an AIOptions toggle): tall front wall vs moderate ~40-elmo
   surround wall.
6. The 1500+300 stockpile exists to quick-print counters and silos; treat the silo
   project as a smaller work item from ~50 m/s, full commitment at 60–80 m/s, gated on
   "current army power is sufficient" (define: army metal ≥ k × seen enemy army, or a
   skirm count floor).
7. Reclaim backline defenses that have not seen a breach in a while. Endgame: lotuses
   barely matter vs artillery, so priority shifts to an impenetrable heavy perimeter
   plus point defenses near critical structures (factories, singularity, adv geo,
   superweapons). Reclaim only when the metal or the build site is actually needed —
   kept turrets have no upkeep.
8. Morph edge case: if a unit is below ~35% HP when a morph would finish, cancel the
   morph unless the game refunds (TEST in-game — refund behaviour on morph cancel is
   unverified). Terraform: the gadget's restore command can be used around Aspis;
   buildings can't be raised once construction starts, but units can be moved, so
   terraforming a placed building's footprint is fine.
9. Endgame scouting lives in the sabotage subroutine: skittish raiders (avoid damage,
   hit unprotected eco, prefer deep structures — antinukes, heavies) + more factories
   for scouting options; plus a ~10-Swift sweep: line across the map, sweep toward the
   enemy base, boost on damage or when entering AA range (chainsaw/hacksaw-class).
   Nuke project at ~80 m/s: allocate 30–50 m/s to a nuclear silo; once built, scout
   for antinukes; if present, site the silo favorably and send tacnukes at them, or
   infiltrate with a Widow to disable them. Execute only when the nuke can be launched
   or ≥2 missiles are stocked. NOTE: nukes cost energy too — the reserve ledger must
   track energy, not only metal.
10. Benchmark methodology agreed (fixed pool, swapped sides, ≥2 games/map, v1 as control).

## Adversarial review — round 2

1. **Four metal sinks, no arbiter.** Reserve floor, porc spend cap, 1500+300 stockpile,
   and the 30–50 m/s nuke allocation are separate budgets that will overdraw the same
   metal. Fix: one `war_chest` ledger module owning current/income/reservations with a
   single priority order (survival porc > army floor > stockpile > counters/silo).
2. **"Enemy heavy spotted" needs a definition.** Encode as a def predicate
   (human_name in {Ogre, Juggernaut, …} or max_weapon_range > R and speed < S), with
   hysteresis and an army floor before the lucifer triggers — one scout shouldn't
   commit 40 m/s of porc.
3. **Scout-raiders vs harass-raiders are the same pool** (point 3 vs doctrine): split
   explicitly (first N raiders scout until the enemy base is found, then join raids),
   or scouting eats the box-in.
4. **"Local superiority" for caretakers depends on Phase 1's threatmap/QRF** — define
   it as army-within-R or no enemy on radar+LOS within R' (fog makes "no enemy in
   sight" weak). Sequencing: Phase 1 must land before caretaker placement logic.
5. **Terraform volume expectations differ by shape.** The 12% figure is a small pad;
   a surround wall is a ring — order(s) of magnitude more volume. Budget the wall
   experiments separately and A/B them; don't assume pit + surround wall is cheap.
6. **Swift sweep costs and dies**: 10 swifts is a real investment, boost costs energy,
   and the line can path into unscouted heavies. Mitigate: spacing = 2× max seen AA
   range, sweep only when radar shows no heavy AA cluster, treat losses as intel cost.
7. **Tacnuke vs antinuke is usually intercepted.** Saturation or Widow disable is the
   reliable counter; also nukes cost energy — energy reserve required (see point 9).
8. **Morph-cancel refund is unverified** — mark as an in-game test; also make the <35%
   HP retreat skip morphing units so retreat doesn't silently break the morph.
9. **Reclaim triggers need precision**: log breach events per defense zone (timestamp
   of last UnitDamaged near each turret), reclaim only > M elmos behind the front and
   never with an incursion inbound; reclaim for metal/space only (kept turrets are free).
10. **Make every experiment an AIOptions toggle** (`porc_budget_pct`, `reserve_metal`,
    `lucifer_on_heavy`, `caretaker_wall=off|front|surround`, `nuke_project`, …) so
    `match.sh`/`series.sh` can A/B with `?key=value` and the results stay comparable.
