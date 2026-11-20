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
   **Landed (issue #7)** as `crates/zktdai/src/threat.rs`: per-enemy sighting table
   (radar+LOS, 60 s staleness), 1024-elmo density cells, `raid_eta` per position,
   `local_superiority` (army near OR no enemy on radar+LOS). Consumers: porc decon
   never reclaims into an inbound incursion; the fusion production pause is gated on
   no incursion near the base; naked FOB caretakers / pit digs / heavy escalation
   require local superiority; expansion constructors skip hot spots (#11 escort rule).
   `threatmap` AIOption (fail-open when off).

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

## Round 2 refinements (incorporated)

1. A strategy profile is chosen at game start (random among a list, or pinned via a
   `strategy` option + logged seed so benchmarks stay repeatable); when a project
   completes, the next one starts. The nuke project is prioritised after the first
   2–3 heavy turrets are on the frontline.
2. "Heavy" = non-flying unit with metal cost > 1600; morphed commanders are an edge
   case if under 4000 metal (commanders upgrade repeatedly). Counter plans split:
   defensive (pit Lucifers, spired Desolators — terrain that wrecks single heavies)
   vs offensive (Widow shock to paralyze, then hunt by local army/artillery). See the
   detailed discussion below.
3. Scout raiders are a split pool: initially ~3 raiders scout (unless they can
   opportunistically win the fight), then all raiders join raider parties; the scout
   share grows to 20% of the raider force until special scouts exist (Sparrow, Gremlin,
   Scythe, Swift, Widow — match by `human_name()`).
4. Constructor taskgroups get an escort: a small raider force ~100 elmos in front of
   the constructors facing enemy territory; otherwise the main battlegroup within
   ~300 elmos; otherwise require no enemy within ~400 elmos. Tuned by A/B tests.
5. Small terraform jobs have per-job overhead, so prioritise high-yield mexes: read
   per-spot values (`mex_metal{i}` rules params / `Map_getResourceMapSpotsAverageIncome`),
   dig ring pits around the best spots (never under the mex itself), give them
   overdrive + defense priority. Midline-adjacent high-yield spots are prime candidates.
6. Corrections accepted: unit abilities (e.g. Swift boost) do not drain energy; radar
   dots are opaque until direct LOS, so intel must come from scouts/saboteurs.
   10 Swifts + air fac ≈ 2200 metal ≈ one Cyclops — cheap at 80 m/s, and some air
   presence is needed anyway.
7. Interception model: Eos (tacnuke) is stopped only by hostile terraform and shields;
   the strategic nuke (Trinity) only by Funnelweb (soft) and antinukes (hard) — so
   dismantling antinukes is the priority target for the nuke project.
8. Morph metal is refunded if the unit dies while morphing (verify in-game) — so no
   cancel-on-low-HP needed for morphs.
9. Turret decon: each turret stamps `last_combat` (our `WeaponFired` / nearby
   `EnemyDamaged`); turrets with no combat for T frames and > M elmos behind the front
   go on a decon list that idle engineers reclaim.
10. Operator UI: start-time strategy toggles via AIOptions (free, plus `?opts` in
    match.sh); live dashboard via a LuaUI widget polling the AI through a LuaMessage
    round-trip (`ZKTDAI_STATUS` → JSON reply) showing projects, taskgroups, budget
    shares, enemy evaluation; websocket remote operator later (pattern:
    `reference/zkgbai-rs/tools/operator/`).

## war_chest ledger (detailed design)

One module, `crates/zktdai/src/war_chest.rs`, owns all spending decisions for metal and
energy. No other module reads `economy.current` to decide spending; they ask the ledger.

State: `reserve_metal` (floor — never spend below `current − reserve`; starts at the
`reserve_metal` option, default 0, and grows as `clamp(0.1 × (storage − starting storage),
reserve_metal, stockpile_target)`), `reserve_energy` (for nuke/overdrive bursts),
`porc_cap_pct` + `army_share` (normal bucket: porc and army production are each capped
per 300-frame window at their share × metal income; the army share is greedy = 100%
below 10 m/s), `stockpile_target` (1500 + 300 headroom), and
`projects: Vec<Project>` where `Project { name, kind, state: Queued|Active|Done|Failed,
budget_bump, reserved, target_unit, started_frame }`. Kinds split into two buckets:
high-priority (mexes/radar/efficient energy/special projects — only floor-constrained,
no window cap) vs normal (army, porc — window-capped, remainder floats as reserve).

API: `update(ctx)` every ~30 frames (reads current/income/usage/storage for both
resources, ages projects, releases reservations on completion); `can_spend(kind, amount)`
= `current − reserve − Σ reserved ≥ amount` plus the per-window cap for normal-bucket
kinds; `note_spend(kind, amount)` records normal-bucket spending; `request_project(…)`
reserves the cost and activates when a builder starts (`UnitCreated` with builder);
priority when metal is scarce: survival porc > army floor > stockpile top-up > active
project (with its `budget_bump`) > queued projects in profile order (nuke after 2–3
frontline heavies). Events: `UnitFinished` of the target → Done, `UnitDestroyed` →
Failed (release reservation), `CommandFinished` → Done. Every 30 s log
`[war_chest] current=… income=… reserve=… projects=[…]` and expose `status_json()` for
the operator widget. Strategy profile picked at Init from a seeded RNG, logged.

## Construction planner (`planner.rs`)

Grows the constructor corps from the factories (`wanted_cons` = 1 energy + 1 base +
up to 3 expansion + reclaim pool growing with income, cap 10) and assigns every
constructor a `ConJob`:

- **Energy** — always High construction priority (cmd 34220 state 2), grids
  wind/solar (whichever is more efficient) near the base, solar backup below 80%
  energy parity.
- **Base** — holds the base; from mid game (income ≥ 30 or 15k frames) builds the
  air factory (build-option expansion is v0.3, #18).
- **Expand** — up to 3 cons beeline to untaken mexes, escorted by up to 3 idle
  raiders (or 1 skirm/riot), secure each cluster with a lotus (+ picket past the
  3rd cluster or 14 total mexes). Turret placement itself stays with porc.
- **Reclaim** — solo wreck-clearing early; from mid game pooled into taskgroups of
  3 on the front line (defense-build / turret-reclaim / maintenance objectives are
  phase 3, #15).

Big plans: checked once a minute once the first fusion / advanced geo is online —
rotating Lucifer → Desolator → Cerberus → missile silo → Bertha at the front line as
High-priority projects. `worker_idle` is the fallback for unassigned workers.

## Adversarial review — round 3

- Random strategies hurt benchmark comparability → pin via option/logged seed; only
  randomise in non-benchmark games.
- The heavy predicate (cost > 1600, non-flying) also catches artillery and some
  supports — fine, but Widow-shock offense needs a spider factory commitment; make it
  a profile choice, not always-on.
- Ring pits around mexes: never dig under the extractor itself; overdrive needs the
  grid module first.
- Decon list needs breach stamps per turret (WeaponFired/EnemyDamaged proximity);
  reclaim only for metal/space.
- Widget: LuaMessage round-trip is enough for v1; websocket operator later.

## Round 3 refinements (incorporated)

1. Heavy predicate corrected: non-flying && metal cost > **1000** (not 1600); Ogre
   (riot tank, ~500m) is *not* heavy; the 1000–2999m band is what the user calls
   "demistriders"; exceptions: Krow (gunship) and Ultimatum (strider). Striders come
   from the strider hub, demistriders from coupon (free-placement) factories.
   Classify by runtime `def.cost(metal)` + `human_name()` — never hardcoded costs
   (they drift between patches; a spot check showed even guessed unitdef costs are
   unreliable). Pit-Lucifer lure: hold fire (`set_fire_state(0)`) until a Widow-stunned
   heavy is in range, then release; triggers rarely, fallback = fire on any heavy in
   range.
2. Escort rule simplified: nearby army OR no enemy within 400 elmos of the group centre.
3. Walled mexes: terraform first — either a 60-elmo-deep footprint pit under the mex,
   or a 60-elmo wall around it. Full ring is expensive → prefer an enemy-facing arc
   with a gap for constructor access. Encourage near the enemy frontline / hot areas
   (players and current AIs struggle to crack walled mexes).
4. Endgame sacrificial swifts are OK; a critically injured swift with dash available
   should consider a suicide scout run into dense, not-yet-identified areas.
5–10. Accepted as stated (interception model Eos/Trinity, morph refund on death,
    turret decon via last-combat stamps, operator widget plan).

## Economy model (new)

- **Power grid**: cylindrical around generators; excess feeds overdrive with multiplier
  `1 + 0.25·sqrt(excess · Σb²)` (confirmed in `reference/.../economy/grid.rs`; ≈ the
  `1+sqrt(excess)/4` per-mex form). Mex extraction scales with supplied power → grid
  mexes early: build wind/solar in the shortest lines between mexes.
- **Wind**: flat 0–1e scaled by map wind/elevation; build when wind > 60% or base wind
  near constructors > 0.4; maps with 1.0-wind areas → wind instead of fusion. Keep
  turbines spaced (rogue raider / Phoenix napalm kills). [verify the elevation rule in
  LuaRules before encoding thresholds]
- **Geothermal**: if a geospot is within ~1k elmos of the start or on our half near the
  border → priority geo project at ~25 m/s with 5–12 buildpower. Keep factories and any
  >2k building ≥700 elmos from geospots (morph explosion); endgame may relax this but
  never for superweapons. Morph to adv geo when energy income ≈ 50e without it (90 s
  job, high priority).
- **Opening (balanced)**: 1 constructor dedicated to energy; initial ~400–500 metal →
  1 constructor + 5 raiders; after the factory, capture the 3 surrounding mexes, then 6
  wind turbines linking them (wind-dependent), then ≥2 solars (2e each); ≥60% of energy
  demand from static sources unless ≥0.6-wind areas exist.
- **Fusion timeline**: 1st fusion at 40–50 m/s, stopping other production for it; 2nd
  fusion after that at lower priority; after the 2nd, only singularity plants.
  [the "15–25 m/s" phrasing for the 2nd fusion is ambiguous — treat as "after the 1st,
  lower priority", tune in tests]
- **Placement**: fusion on a 100-elmo spire, singularity on a 144-elmo spire —
  terraform the spire BEFORE placing (buildings can't be raised once construction
  starts). Singularity defense budget: 2 Faradays, or Newton turrets ringing the
  perimeter, or air supremacy (Swifts, with some requisitioned for lategame scouting,
  plus Raptors/Magpies vs Odin raids), or ground AA (a jumpbot Moderator on Odin
  interception near high-risk targets, a Gremlin garrison, or 4 Hacksaws near the
  target set to ignore targets <300m). Endgame: if the enemy fields 20+ planes, consider
  an Artemis sited to push its range slightly into enemy territory or ~300 elmos from
  forward priority targets (e.g. a Cerberus siege camp).

## Adversarial review — round 4

1. Full-ring 60-elmo walls are volume-expensive (a ring, not a pad — see round-2 #5):
   ship enemy-facing arcs first, A/B ring vs arc vs pit, and keep a constructor gap.
2. Spire-then-build is load-bearing sequencing: terraform first, then place — if the
   dig is interrupted the project must re-queue (tag tracking), else the fusion sits
   on flat ground.
3. The 700-elmo geospot rule must apply to ALL >2k buildings, not just factories, and
   the endgame exception must never cover superweapons.
4. "Stop all production at 40–50 m/s for fusion" opens an army gap — gate on the
   threatmap (no incursion inbound) or make it an econ-greedy profile choice.
5. Wind/fusion branching needs map wind data at Init; verify the elevation rule before
   hardcoding 0.4/0.6 thresholds.
6. Never hardcode unit costs — classify via runtime `def.cost()`/`human_name()`.
7. Hacksaw "ignore <300m", Odin/Raptor/Magpie names → resolve to cmd ids/unitdefs at
   runtime (reference `defs.rs` cmd list has the target-selection commands).
8. The balanced opening (1 con + 5 raiders) depends on the scout-pool split so raiders
   don't all die scouting; make the opening a scripted sequence with early-exit
   conditions.

## Round 4 highlights (incorporated) + verified unit definitions

1. First fusion (35e) is a big overdrive boost: at 40–50 m/s, pause all other production
   until it completes, to field it fast and reap overdrive sooner. Second fusion builds
   slower — army power / extra defense (e.g. 3 Lotuses near the fusion) may come first.
2. Opening scouts: first 2 raiders scout, the rest raid; even if scouts die, first contact
   reveals the enemy factory → adjust from it.
3. Wind decision rule (analysis): expected turbine output = `myMin + w·(windMax−myMin)`;
   windMax = 2.5·energyMult (unit_windmill_control.lua:243), myMin rises with altitude
   (0.4·windMax per 400 elmo), strength is re-randomised each period in [windMin, windMax].
   Energy-per-metal: turbine(0–2.5) = 2.5w/35; turbine(1–2.5) = (1+1.5w)/35;
   fusion 35/1000 = 0.035; geo 25/500 = 0.05; singu 225/4000 = 0.05625; adv geo 100/1500 = 0.0667.
   Breakpoints — min-0 turbine beats fusion at w>0.49, geo at w>0.70, singu at w>0.79,
   adv geo at w>0.93; min-1 (high-ground) turbine beats fusion at w>0.15, geo at w>0.50,
   singu at w>0.65, adv geo at w>0.89. Rule of thumb "wind mainly at ≥0.6" ≈ beats geo,
   not yet singu; with high-ground turbines wind wins much earlier. Wind also doubles as
   mex grid links (overdrive), which e/m math misses. Sub-20% wind dips need static backup
   (solars). AI reads map wind via `Map_getMinWind/MaxWind/CurWind` (in the FFI bindings;
   add wrappers in Phase 1).
4. Overdrive confirmed: multiplier `1 + 0.25·sqrt(excess)` (unit_mex_overdrive.lua:266).

### Verified unitdefs (Zero-K repo, units/*.lua — `name` = human name, key = internal)

| Human | Internal | metal | | Human | Internal | metal |
|---|---|---|---|---|---|---|
| Ogre | tankriot | 500 | | Lotus | turretlaser | 90 |
| Krow | gunshipkrow | 4200 (flies → not heavy) | | Picket | turretmissile | 95 |
| Ultimatum | striderantiheavy | 2400 | | Stardust | turretriot | 220 |
| Widow | spiderantiheavy | 280 | | Gauss | turretgauss | 400 |
| Sparrow | planelightscout | 230 | | Stinger | turretheavylaser | 450 |
| Gremlin | cloakaa | 120 | | Razor (AA laser) | turretaalaser | 280 |
| Scythe | cloakheavyraid | 250 | | Chainsaw (AA) | turretaafar | 900 |
| Swift | planefighter | 150 | | Hacksaw (AA) | turretaaclose | 210 |
| Glaive (raider) | cloakraid | 65 | | Faraday | turretemp | 260 |
| Ronin (skirm) | cloakskirm | 90 | | Newton | turretimpulse | 200 |
| Minotaur (assault) | tankassault | 850 | | Lucifer | turretantiheavy | 2200 |
| Scorcher (veh raid) | vehraid | 130 | | Desolator | turretheavy | 1500 |
| Caretaker | staticcon | 180 | | Aegis | staticshield | 525 |
| Strider Hub | striderhub | — | | Aspis | shieldshield | 700 |
| Radar Tower | staticradar | 55 | | Artemis (AA) | turretaaheavy | 2400 |
| Wind turbine | energywind | 35 | | Cerberus (arty) | staticarty | 2500 |
| Solar | energysolar | 70 | | Eos missile | tacnuke | 600 |
| Mex | staticmex | 85 | | Missile silo | staticmissilesilo | 1200 |
| Fusion | energyfusion | 1000 | | Nuke silo | staticnuke | 8000 |
| Singularity | energysingu | 4000 | | Anti-nuke | staticantinuke | 3000 |
| Geothermal | energygeo | 500 | | Storage | staticstorage | 100 |
| Adv geothermal | energyheavygeo | 1500 | | Pylon | energypylon | 200 |

Note: v1's turret ladder internal names were right but labels off (turretgauss is Gauss,
not Picket); v2 classifies by `human_name()` with internal-name fallback. Heavy
predicate: `!is_able_to_fly && cost > 1000`; Krow auto-excluded (flies); commanders
(`dyn*`/`cremcom*`) edge case <4000.

## v0.3 goal — archon mode + ally coordination for up to 16v16 (#18)

Tracked in https://github.com/Sparkezel/ZKTDAI/issues/18.

### Engine side (verified in RecoilEngine + Chobby sources)

- `/aicontrol <teamToControl> <aiShortName> [aiVersion] [name] [options...]` — engine
  command; blocked by default since 2026-06 (`aiControlFlags.fill(true)`,
  `GameServer.cpp:215`). Host enables per player with **`/aiCtrl <playerName>`**
  (case-insensitive prefix match) or **`/aiCtrlByNum <num>`** (host-only server
  commands, `GameServer.cpp:118`, local check at `:1804`). Issuer must be team
  leader / singleplayer / allied+cheats / no-leader. One AI per team.
- Engine allows a human player and an AI on the same team (leader check passes),
  so archon mode = `/aiCtrl <myName>` then `/aicontrol 0 ZKTDAI 0.3.0?archon_mode=true`.

### ZKTDAI side — archon gating (`archon_mode` AIOption)

- Skip `send_start` (the human picks the start position).
- Only Commander / Worker / Factory units receive orders (economy, mexes, energy,
  storages, porc under a reduced budget); no combat or sabotage orders are issued.
- Human keeps army micro; AI feeds the eco. Status JSON (`ZKTDAI_STATUS`) reports
  what the AI is doing so the player can see projects/budget.

### Ally coordination (16v16, up to 16 AI instances in one process)

- Shared vision is on in team games, so each instance already sees the whole team's
  sight; no extra work needed for intel.
- Intent broadcast: instances send `LuaMessage` ("ZKTDAI_INTENT" / status) so ally
  ZKTDAI instances can read teammates' current projects.
- `war_chest` project dedup by team: energy/silo/nuke/storage projects are claimed
  team-wide via the intent channel so two instances don't both build a fusion.
- Per-instance CPU budget: stagger update cadence by slot index so 16 instances in
  one library don't spike one frame.

### Acceptance

- Headless `match.sh -L 2v2` with two ZKTDAI instances on one team (no crash,
  projects deduped).
- Local lobby: `/aiCtrl <player>` + `/aicontrol 0 ZKTDAI 0.3.0?archon_mode=true` —
  AI runs eco while player micros army.
