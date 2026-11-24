# ecov2 — construction v2: dedicated roles + free-worker pool + urgency queue

Design doc for restructuring `planner.rs` from pure role assignment into a hybrid:
**dedicated roles** (fixed jobs) plus a **free-worker pool** that serves a **global job
queue** ordered by **urgency**, with idle free workers **pre-positioned at defended
territory anchors** so jobs start with less delay.

Status: proposed (discussed in session; counter-reviewed below). Implementation order
in §6.

## 1. Current state (v2 @ 806cbde)

`planner.rs` assigns every constructor exactly one stable `ConJob`
(Energy / Base / Expand / Reclaim), re-evaluated every 300 frames; `worker_idle` in
`economy.rs` is the fallback for unassigned workers. Production gating lives in
`war_chest.rs` (high-priority bucket = floor-only; normal bucket = army/porc window
caps; reserve floor; project reservations; cmd-34220 priority flags). Turrets are
placed by `porc.rs` (income-keyed ladder, threat notes), mexes by Expand jobs, big
projects (lucifer/desolator/cerberus/silo/bertha) by `big_projects`.

Known weaknesses this doc addresses:

- assignment latency: planner ticks every 300 f (10 s), so a job created right after a
  tick waits up to 10 s before any worker is assigned — larger than local travel time;
- no shared queue: porc / expand / base each manage their own workers, no mobbing of
  big jobs, no global view of what is most urgent;
- idle constructors clump at base; cross-map travel to a new job is 100+ s;
- no auto-repair, no blockage handling (a worker standing on a build site just makes
  `build_at` fail and retry next tick).

## 2. Proposed model

```
con corps
├── Role::Energy      1 dedicated   always cmd-34220 High; wind/solar grid; solar <80% parity
├── Role::Base        1 dedicated   base hold; midgame air factory
├── Role::Expand    ≤ 3 dedicated   mex clusters: beeline, escort, lotus (+picket late)
└── Free pool       rest (≥1)      serves the global job queue; pre-positioned at anchors
```

**Global job queue** (`jobs: HashMap<u64, Job>`):

```rust
struct Job {
    pos: Vec3, def: UnitDef, kind: JobKind,   // Turret | Mex | Lotus | Energy | Project | Reclaim | Repair
    urgency: f32,          // computed, see §4
    assigned: Vec<i32>,    // worker ids
    reserved: f32,         // metal locked while a worker travels (McCoy locking)
    since: i32,
}
```

Job sources: porc (turret sites), Expand (mex + cluster lotus/picket), energy builder
assist, big projects, reclaim wrecks (from UnitDestroyed/EnemyDestroyed + features),
auto-repair (UnitDamaged on our static armed buildings), terraform assist (repair the
`terraunit` when we issue a dig).

**Assignment**: event-driven — on `UnitIdle`/`UnitFinished` of a Free worker, assign
immediately (GBC `newBuilders` pattern); the 300 f tick only re-evaluates (stickiness,
reassign-one-per-cycle, pruning). Effective cost per (worker, job):

```
cost = distance(worker, job) / urgency(job) + class_modifiers(job, assigned_count)
```

i.e. GBC's distance cost model (`IntelliCost`: expensive job +400 start penalty that
vanishes once assigned; small defense −150 and 2-worker cap before +800; small energy
+100; reclaim/repair +400 start; resurrect top) divided by our urgency weight, so an
urgent job looks "closer". Stickiness: current job −30; never pull a worker that is
actively building (nanoframe exists).

**Pre-positioning**: idle Free workers patrol the nearest **anchor** — an owned mex
with a lotus (defended point), else our centroid. Never outside the porc perimeter
(see §5 risk 2). Anchors recomputed as territory grows.

## 3. Prior art / research

- **Central Build AI (2009, T. Cheek) → Central Build Group → ZK Global Build Command**
  (aeonios 2015, `LuaUI/Widgets/unit_global_build_command.lua`): the direct precedent —
  global persistent queue, hash dedupe, worker states (idle/queue/drec/mov/chicken),
  cost-model assignment with mobbing caps, stickiness, move-blockers-aside, area-job
  splitting, auto-repair/auto-reclaim, deferred UnitIdle. Reactive (captures player
  orders), no economy gating — we keep our ledger on top.
- **McCoy et al., AAAI-08** (integrated RTS agent on Spring): production manager with
  priority queues (unit/building/repeat) + worker-release manager; explicitly **locks
  resources while a worker travels** so construction doesn't fail mid-transit → our
  `Job.reserved`.
- **Weber et al., MTAS (IEEE TCIAIG 2013)**: utility-function multi-agent task
  allocation integrated into E323AI on Spring/BA, statistically significant gains → the
  "urgency = utility" framing used in §4.
- **Hagelbäck & Johansson (ORTS 2008)**: potential fields pulling workers to build sites.
- **Buro; Blackford & Lamont (AIIDE'14)**: build-order optimization (PDDL/EA) — fixes the
  order, not dynamic worker allocation; complementary, not a substitute.
- **zkgbai (practice)**: constructors mostly **dedicated to projects** (walkin heavies,
  project tracker); no free-worker spreading — evidence that pre-positioning is not
  needed everywhere, only anchored inside defended territory.

## 4. Urgency function

Total order required (no ties that oscillate). Proposed:

```
class_weight:  SurvivalPorc 100 > BigProject 80 > Mex 60 > Energy 50 > Turret 40 > Reclaim 10 > Repair 30 (porc repair 90)
threat_mult:   1 + porc recent combat/damage near job (porc.note_combat/note_damage), capped ×3
age:           +1 per 600 f unassigned (starvation breaker)
urgency = class_weight * threat_mult + age
```

Urgency affects **assignment order and the cmd-34220 High flag only** — never the
war_chest ledger: a job is only created if `can_spend(kind, cost)` passes, and while a
worker travels, `reserved` metal is locked. Normal-bucket caps (army/porc window) still
apply at creation time.

## 5. Counter-review (risks → mitigations)

1. **Delay is mostly tick latency, not travel** → event-driven assignment on
   UnitIdle/UnitFinished first (step 1); pre-positioning only helps jobs near anchors.
2. **Safety**: spread-out lone cons get raided → anchors only inside porc perimeter;
   retreat-on-low-HP for jobbed workers only when *not* mid-build (nanoframe persists).
3. **Thrash**: no stickiness → workers abandon jobs mid-travel → port GBC: −30 current
   job, reassign one worker per cycle, never pull an actively-building worker.
4. **Role starvation**: free pool fully conscripted → keep ≥1 free; never conscript
   Energy/Base; Expand workers loanable only to jobs near their cluster.
5. **Ledger bypass**: urgency must not override war_chest → jobs created only via
   `can_spend`; urgency only orders assignment + sets builder High priority.
6. **Bookkeeping**: role-jobbed workers excluded from queue (`has_job` gate, Free =
   unassigned/Reclaim); worker death → job back to queue unassigned (don't delete);
   blocked site → move-out-of-the-way nudge (use `Map_isPossibleToBuildAt`); invalid
   site (mex taken) → drop job (`ordered_spots` / `terr.owned`).
7. **Measurement**: A/B with `tools/headless` (`-TraceEvents`); metric = frames from
   job creation to first nanoframe (`UnitCreated` with builder), plus win rate vs
   CircuitAI:Normal on issue #2.

## 6. Implementation order

1. Event-driven assignment: on `UnitIdle`/`UnitFinished` of a Free worker, assign from
   the queue immediately; 300 f tick only re-evaluates.
2. Job queue in `planner.rs`: `Job` struct, hash dedupe, urgency fn (§4), cost-based
   assignment for the Free pool, mobbing caps per class, stickiness, reservations.
3. Pre-positioning: idle Free workers patrol nearest owned-mex/lotus anchor (inside
   porc perimeter only).
4. Extras from the GBC list: move-out-of-the-way nudge in `build_at`, auto-repair jobs
   from `UnitDamaged`, feature-based reclaim, split area reclaim, terraform-assist
   repair jobs, footprint-overlap dedupe for turret sites.

## 7. Energy gridding with mexes — concrete tasks

Current state (branch @ 666a0c6): the opening (`economy.rs` `Opening::WindGrid`)
builds a pylon chain base → home mexes (`plan_grid_pylons`, 0.9×pylonrange spacing)
then winds at midpoints between home mexes; steady state (`planner.rs` `energy_step`)
builds power at the frontier owned mex (`territory.rs` `frontier_spot`). Gaps, as
tasks:

1. **Per-mex energy**: Expand job, after securing a mex cluster (lotus/picket), also
   queues one small energy plant at the mex (Energy bucket) — players grid a
   solar/wind per mex, not one per frontier push.
2. **Pylon extension on expansion**: recompute/extend the pylon chain to newly
   owned clusters (`plan_grid_pylons` per cluster) so frontier mexes can overdrive.
3. **Elevation-weighted wind placement** (ZK wind output scales with elevation —
   see `docs/possible-helk-integration.md` §5): when placing wind, prefer high ground
   (`Map::elevation_at`); optionally RAISE a pad under wind farms via `terraform.rs`.
4. **Invalid build-site cache with TTL**: remember cells where
   `find_closest_build_site`/`is_possible_to_build_at` failed (Hel-K `TestBuild`
   memory) so `build_at_clear` stops re-proposing dead cells every 300 f tick.
5. **Lattice layout for mass energy** (Hel-K paint-farm `FARM_SCALE`): when gridding
   a base area, tile with cell = footprint + gap, neat-squares snapping, rows from
   base toward mexes — replaces the current single `build_at(our_start, 300)` blob.

## 8. Open questions

- Should Expand roles be loanable to the queue at all (risk: unsecured mex clusters), or
  strictly dedicated until territory target is hit?
- Reclaim urgency: frontline wrecks (threat area) vs backline — should frontline reclaim
  outrank turrets under active combat? (Currently class 10, threat_mult can raise it.)
- Do we want path caching (`RequestPath`) per worker-job pair, or is raw distance enough
  at our con counts (≤10)?
