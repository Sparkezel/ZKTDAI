//! Construction planner: grows the constructor corps from the factories and
//! assigns every constructor a job — energy (always High construction priority),
//! base (caretakers, express defense, midgame air factory), expansion (beeline
//! to untaken mex clusters with a small escort, secure with lotus + picket) —
//! plus reclaim (solo early, pooled into taskgroups in the mid game) and the big
//! late-game projects (Lucifer/Desolator/Cerberus, missile silo, Bertha) once
//! the first fusion / advanced geothermal is online.

use std::collections::{HashMap, HashSet};

use springai::{NO_TIMEOUT, Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::{Role, cmd};
use crate::economy::{
    build_at, build_at_clear, build_mex, has_queued_mex_build, metal_cost, power_def, repair_unfinished_mex,
    wants_energy,
};
use crate::territory::Territory;
use crate::threat::Threat;
use crate::war_chest::{ProjectKind, ProjectState, WarChest};

/// What a constructor is currently assigned to do.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ConJob {
    /// Grids energy near the base; always on High construction priority (cmd 34220).
    Energy,
    /// Stays at base: caretakers, express defense, midgame air factory.
    Base,
    /// Beelines to an untaken mex cluster with an escort; secures it with a
    /// lotus (plus a picket past the 3rd cluster or 14 total mexes).
    Expand,
    /// Reclaims wrecks; pooled into taskgroups from the mid game on.
    Reclaim,
}

/// A constructor's in-flight approach to a mex spot (zkgbai's `Approach`):
/// re-send the move+build chain only when the queue lost the build order or
/// no progress (32 elmos closer) was made within 300 frames.
#[derive(Clone, Copy)]
struct Approach {
    spot: usize,
    sent: i32,
    best: f32,
}

/// A pooled group of constructors with a shared objective and position.
pub struct TaskGroup {
    pub members: Vec<i32>,
    /// Read by the phase-3 objective dispatch (issue #15).
    #[allow(dead_code)]
    pub objective: &'static str,
    pub pos: Vec3,
}

/// Big late-game projects, tried in rotation once fusion/advanced geo is up.
/// (project name, def key — human names resolve via `Defs::by_human`).
const BIG_PROJECTS: &[(&str, &str)] = &[
    ("lucifer", "turretantiheavy"),
    ("desolator", "turretheavy"),
    ("cerberus", "Cerberus"),
    ("silo", "staticmissilesilo"),
    ("bertha", "staticheavyarty"),
];
/// Check big projects this often (60 s at 30 fps).
const BIG_CHECK_FRAMES: i32 = 1800;
/// Mid-game threshold: pool reclaimers into taskgroups, build the air factory.
const MIDGAME_INCOME: f32 = 30.0;
/// Re-issue expansion escorts this often.
const ESCORT_FRAMES: i32 = 600;
/// Max tracked wreck positions.
const MAX_WRECKS: usize = 64;
/// Escort rule (plan round 2 #4, issue #11): a constructor may work a spot
/// when our army is within `ESCORT_ARMY_RADIUS` of it OR no enemy is on
/// radar+LOS within `ESCORT_ENEMY_RADIUS` of it.
const ESCORT_ARMY_RADIUS: f32 = 300.0;
const ESCORT_ENEMY_RADIUS: f32 = 400.0;

pub struct Planner {
    jobs: HashMap<i32, ConJob>,
    /// Units already set to High construction priority (energy builders).
    high_prio: HashSet<i32>,
    pub groups: Vec<TaskGroup>,
    wrecks: Vec<Vec3>,
    last_big_check: i32,
    big_idx: usize,
    clusters_secured: usize,
    escort_cd: i32,
    air_fac_queued: bool,
    air_fac_done: bool,
    second_fac_queued: bool,
    second_fac_done: bool,
    /// Per-constructor in-flight mex approach (spot, sent frame, best dist).
    approach: HashMap<i32, Approach>,
}

impl Planner {
    pub fn new() -> Self {
        Planner {
            jobs: HashMap::new(),
            high_prio: HashSet::new(),
            groups: Vec::new(),
            wrecks: Vec::new(),
            last_big_check: 0,
            big_idx: 0,
            clusters_secured: 0,
            escort_cd: 0,
            air_fac_queued: false,
            air_fac_done: false,
            second_fac_queued: false,
            second_fac_done: false,
            approach: HashMap::new(),
        }
    }

    /// Whether this constructor is run by the planner (`worker_idle` skips it).
    pub fn has_job(&self, id: i32) -> bool {
        self.jobs.contains_key(&id)
    }

    /// How many constructors we want in total right now: 1 energy + 1 base +
    /// up to 3 expansion + a reclaim pool that grows with income.
    pub fn wanted_cons(&self, ctx: &Ctx, terr: &Territory) -> usize {
        let Some(metal) = ctx.metal else { return 1 };
        let income = ctx.cb.economy().income(metal);
        let free = ctx.spots.iter().zip(&terr.owned).zip(&terr.enemy).filter(|((_, o), e)| !**o && !**e).count();
        let expand = free.min(5);
        let reclaim = if income < 20.0 { 1 } else { (1 + (income / 25.0) as usize).min(4) };
        (2 + expand + reclaim).min(12)
    }

    /// Track a wreck (from UnitDestroyed/EnemyDestroyed) for reclaim jobs.
    pub fn note_wreck(&mut self, pos: Vec3) {
        if self.wrecks.iter().any(|w| w.distance_2d(pos) < 64.0) {
            return;
        }
        if self.wrecks.len() >= MAX_WRECKS {
            self.wrecks.remove(0);
        }
        self.wrecks.push(pos);
    }

    #[allow(clippy::too_many_arguments)] // ctx+roles+terr+threat are the shared read view
    pub fn update(
        &mut self,
        ctx: &Ctx,
        roles: &HashMap<i32, Role>,
        terr: &Territory,
        ordered: &mut HashSet<usize>,
        threat: &Threat,
        war: &mut WarChest,
        fob_mobile: &HashSet<i32>,
    ) {
        let cb = ctx.cb;
        // prune dead units
        let alive: HashSet<i32> = cb.team_units().iter().map(|u| u.id()).collect();
        self.jobs.retain(|id, _| alive.contains(id));
        self.high_prio.retain(|id| alive.contains(id));
        self.approach.retain(|id, _| alive.contains(id));
        for g in &mut self.groups {
            g.members.retain(|id| alive.contains(id));
        }
        self.groups.retain(|g| !g.members.is_empty());
        // workers available for jobs (not mid-build, not held by the FOB)
        let mut workers: Vec<Unit> = cb
            .team_units()
            .into_iter()
            .filter(|u| {
                roles.get(&u.id()) == Some(&Role::Worker) && !u.is_being_built() && !fob_mobile.contains(&u.id())
            })
            .collect();
        workers.sort_by(|a, b| {
            a.pos()
                .distance_2d(ctx.our_start)
                .partial_cmp(&b.pos().distance_2d(ctx.our_start))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.assign(ctx, &workers, terr);
        let pooled = ctx.metal.is_some_and(|m| cb.economy().income(m) >= MIDGAME_INCOME);
        // taskgroups: from the mid game, pool reclaimers into groups of 3 on the
        // front line (defense-build / turret-reclaim / maintenance objectives
        // are phase-3 work, issue #15)
        if pooled {
            let reclaimers: Vec<i32> =
                self.jobs.iter().filter(|(_, j)| **j == ConJob::Reclaim).map(|(id, _)| *id).collect();
            self.groups = reclaimers
                .chunks(3)
                .map(|chunk| TaskGroup { members: chunk.to_vec(), objective: "frontline-reclaim", pos: terr.front })
                .collect();
        } else {
            self.groups.clear();
        }
        let group_pos: HashMap<i32, Vec3> =
            self.groups.iter().flat_map(|g| g.members.iter().map(|id| (*id, g.pos))).collect();
        // run the jobs
        let jobs: Vec<(i32, ConJob)> = self.jobs.iter().map(|(id, j)| (*id, *j)).collect();
        for (id, job) in jobs {
            let Some(unit) = Unit::from_id(cb, id) else { continue };
            match job {
                ConJob::Energy => self.energy_step(ctx, unit, roles, terr, war),
                ConJob::Base => self.base_step(ctx, unit, roles, terr, war),
                ConJob::Expand => self.expand_step(ctx, unit, roles, terr, ordered, threat, war),
                ConJob::Reclaim => self.reclaim_step(unit, terr, pooled, group_pos.get(&id).copied()),
            }
        }
        self.big_projects(ctx, roles, terr, war);
    }

    /// Keep existing assignments while the job is still wanted, then fill
    /// unfilled jobs with unassigned workers in priority order.
    fn assign(&mut self, ctx: &Ctx, workers: &[Unit], terr: &Territory) {
        let Some(metal) = ctx.metal else { return };
        let income = ctx.cb.economy().income(metal);
        let free = ctx.spots.iter().zip(&terr.owned).zip(&terr.enemy).filter(|((_, o), e)| !**o && !**e).count();
        let _ = income;
        let want: [(ConJob, usize); 4] =
            [(ConJob::Energy, 1), (ConJob::Base, 1), (ConJob::Expand, free.min(5)), (ConJob::Reclaim, usize::MAX)];
        let mut counts: HashMap<ConJob, usize> = HashMap::new();
        let mut new_jobs: HashMap<i32, ConJob> = HashMap::new();
        let mut old: Vec<(i32, ConJob)> = self.jobs.iter().map(|(id, j)| (*id, *j)).collect();
        old.sort_by_key(|(_, j)| match j {
            ConJob::Energy => 0,
            ConJob::Base => 1,
            ConJob::Expand => 2,
            ConJob::Reclaim => 3,
        });
        for (id, job) in old {
            let cap = want.iter().find(|(j, _)| *j == job).map(|(_, c)| *c).unwrap_or(0);
            let c = counts.entry(job).or_insert(0);
            if *c < cap {
                new_jobs.insert(id, job);
                *c += 1;
            }
        }
        for u in workers {
            if new_jobs.contains_key(&u.id()) {
                continue;
            }
            for (job, cap) in want {
                let c = counts.entry(job).or_insert(0);
                if *c < cap {
                    new_jobs.insert(u.id(), job);
                    *c += 1;
                    break;
                }
            }
        }
        self.jobs = new_jobs;
    }

    /// The energy builder: always High priority, grids solar/wind (whichever is
    /// more efficient; solar backup below 80% energy parity) near the base.
    fn energy_step(&mut self, ctx: &Ctx, unit: Unit, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        let cb = ctx.cb;
        if self.high_prio.insert(unit.id()) {
            unit.execute_custom_command(cmd::PRIORITY, &[2.0], opt::NONE, NO_TIMEOUT);
            cb.log(&format!("[planner] con {} is the energy builder (High priority)", unit.id()));
        }
        let cur = ctx.metal.map(|m| cb.economy().current(m)).unwrap_or(0.0);
        let def = if let Some(e) = ctx.energy {
            let eco = cb.economy();
            let usage = eco.usage(e);
            if usage > 0.0 && eco.income(e) < 0.8 * usage {
                ctx.defs.solar // below 80% energy parity: solar backup
            } else if wants_energy(ctx) {
                power_def(ctx)
            } else {
                None
            }
        } else if wants_energy(ctx) {
            power_def(ctx)
        } else {
            None
        };
        // CircuitAI builds energy at the frontier (mex spots outside the ally
        // zone), not only at the base: anchor to the owned spot nearest the
        // front line, falling back to our start.
        let want = terr.frontier_spot(ctx).map(|i| ctx.spots[i]).unwrap_or(ctx.our_start);
        if let Some(def) = def {
            let cost = metal_cost(ctx, def);
            if war.can_spend(ctx, ProjectKind::Energy, cost) && cur >= cost {
                build_at_clear(cb, ctx, roles, unit, def, want, 300.0);
            }
        }
        if unit.pos().distance_2d(want) > 400.0 {
            unit.move_to(want, opt::NONE);
        }
    }

    /// The base constructor: holds the base and, from the mid game on, opens the
    /// air build options (air factory) and a second factory. Caretakers/express
    /// defense on demand are porc's job; unit build-option expansion is v0.3.
    fn base_step(&mut self, ctx: &Ctx, unit: Unit, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        let cb = ctx.cb;
        // Second factory once the economy can afford it: one factory cannot
        // replace a raider army that is constantly fighting.
        if !self.second_fac_done {
            let income = ctx.metal.map(|m| cb.economy().income(m)).unwrap_or(0.0);
            let factories = cb
                .team_units()
                .iter()
                .filter(|u| roles.get(&u.id()) == Some(&Role::Factory) && !u.is_being_built())
                .count();
            let want_second = factories < 2 && (income >= 25.0 || ctx.frame >= 12000);
            if want_second && let Some(f) = ctx.defs.factory {
                let cost = metal_cost(ctx, f);
                if !self.second_fac_queued {
                    // Reserve the cost (stops army/porc draining it) but start
                    // the build immediately: buildings don't need the full metal
                    // up front, construction is paid per frame from build power.
                    self.second_fac_queued = true;
                    war.request_save(ctx, "second-factory", ProjectKind::Expansion, Some(f), cost);
                }
                // CircuitAI places follow-up factories at finished clusters
                // near the midfield (forward base), offset from the enemy:
                // anchor to the owned spot nearest the front line.
                let want = terr.frontier_spot(ctx).map(|i| ctx.spots[i]).unwrap_or(ctx.our_start);
                if build_at_clear(cb, ctx, roles, unit, f, want, 500.0) {
                    self.second_fac_done = true;
                    cb.log("[planner] second factory started");
                }
            }
        }
        if !self.air_fac_done {
            let income = ctx.metal.map(|m| cb.economy().income(m)).unwrap_or(0.0);
            let air_exists = cb
                .team_units()
                .iter()
                .any(|u| u.def().is_some_and(|d| d.name() == "factoryplane" || d.name() == "factorygunship"));
            let midgame = income >= MIDGAME_INCOME || ctx.frame >= 15000;
            if !air_exists
                && midgame
                && let Some(f) = ctx.defs.get("factoryplane").or_else(|| ctx.defs.get("factorygunship"))
            {
                let cost = metal_cost(ctx, f);
                if !self.air_fac_queued {
                    // Buildings don't need the full metal up front: the engine
                    // starts the nanoframe and construction is paid per frame
                    // from build power. So reserve the cost (stops army/porc
                    // draining it, gives the builder High priority) and start
                    // the build immediately — supply it with build power.
                    self.air_fac_queued = true;
                    war.request_save(ctx, "air-factory", ProjectKind::Expansion, Some(f), cost);
                    if build_at(cb, unit, f, ctx.our_start, 500.0) {
                        self.air_fac_done = true;
                        cb.log("[planner] air factory started (paid over time from build power)");
                    }
                }
            }
        }
        if unit.pos().distance_2d(ctx.our_start) > 400.0 {
            unit.move_to(ctx.our_start, opt::NONE);
        } else {
            unit.patrol_to(ctx.our_start, opt::NONE);
        }
    }

    /// An expansion constructor: beeline to the nearest untaken mex, build it,
    /// then secure the cluster (lotus, + picket late); escorted by up to 3 idle
    /// raiders (or 1 skirm/riot).
    /// Send the approach + mex build as one SHIFT chain: the build stays
    /// queued behind the move and the constructor keeps building until the
    /// mex is finished (it must not walk off leaving an unfinished frame).
    fn send_mex_chain(
        &mut self,
        ctx: &Ctx,
        unit: &Unit,
        mex: UnitDef,
        i: usize,
        war: &WarChest,
        ordered: &mut HashSet<usize>,
    ) {
        let cb = ctx.cb;
        let spot = ctx.spots[i];
        let cur = ctx.metal.map(|m| cb.economy().current(m)).unwrap_or(0.0);
        let cost = metal_cost(ctx, mex);
        if !(war.can_spend(ctx, ProjectKind::Expansion, cost) && cur >= cost) {
            return;
        }
        if unit.pos().distance_2d(spot) > 200.0 {
            unit.move_to(spot, opt::NONE);
            build_mex(cb, *unit, mex, spot, opt::SHIFT_KEY);
        } else {
            build_mex(cb, *unit, mex, spot, opt::NONE);
        }
        ordered.insert(i);
        self.approach.insert(unit.id(), Approach { spot: i, sent: ctx.frame, best: unit.pos().distance_2d(spot) });
    }

    /// Escort: up to 3 idle raiders, else 1 skirm/riot, guard this constructor.
    fn escort(&mut self, ctx: &Ctx, unit: &Unit, roles: &HashMap<i32, Role>) {
        let cb = ctx.cb;
        let me = unit.pos();
        if ctx.frame < self.escort_cd {
            return;
        }
        self.escort_cd = ctx.frame + ESCORT_FRAMES;
        let mut escorts: Vec<Unit> = cb
            .team_units()
            .into_iter()
            .filter(|u| roles.get(&u.id()) == Some(&Role::Raider) && u.num_current_commands() == 0)
            .collect();
        escorts.sort_by(|a, b| {
            a.pos().distance_2d(me).partial_cmp(&b.pos().distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
        });
        if escorts.is_empty() {
            escorts = cb
                .team_units()
                .into_iter()
                .filter(|u| roles.get(&u.id()) == Some(&Role::Skirm) && u.num_current_commands() == 0)
                .take(1)
                .collect();
        }
        for e in escorts.iter().take(3) {
            e.guard(*unit, opt::NONE);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_step(
        &mut self,
        ctx: &Ctx,
        unit: Unit,
        roles: &HashMap<i32, Role>,
        terr: &Territory,
        ordered: &mut HashSet<usize>,
        threat: &Threat,
        war: &mut WarChest,
    ) {
        let cb = ctx.cb;
        let cur = ctx.metal.map(|m| cb.economy().current(m)).unwrap_or(0.0);
        let me = unit.pos();
        let Some(mex) = ctx.defs.mex else { return };
        // 1) Continue the current approach if the target is still valid and the
        // order chain is intact (zkgbai's approach/queued_build_matches). The
        // engine can silently drop a build; detect that via the queue and
        // re-send. Re-send a stalled approach at most every 300f with no
        // 32-elmo progress.
        if let Some(a) = self.approach.get(&unit.id()).copied() {
            if a.spot < ctx.spots.len() && !terr.owned[a.spot] && !terr.enemy[a.spot] {
                let spot = ctx.spots[a.spot];
                let d = me.distance_2d(spot);
                let walking = d > 200.0 && unit.num_current_commands() > 0;
                let chain_ok = has_queued_mex_build(&unit, mex, spot) || walking;
                let stalled = ctx.frame - a.sent > 300 && d > a.best - 32.0;
                if chain_ok && !stalled {
                    if d < a.best {
                        self.approach.insert(unit.id(), Approach { best: d, ..a });
                    }
                    // in range but the build order vanished from the queue: re-send it
                    if d <= 200.0 && !has_queued_mex_build(&unit, mex, spot) {
                        let cost = metal_cost(ctx, mex);
                        if war.can_spend(ctx, ProjectKind::Expansion, cost) && cur >= cost {
                            build_mex(cb, unit, mex, spot, opt::SHIFT_KEY);
                            ordered.insert(a.spot);
                        }
                    }
                    self.escort(ctx, &unit, roles);
                    return;
                }
                // broken or stalled: drop and re-send the chain below
                self.approach.remove(&unit.id());
                self.send_mex_chain(ctx, &unit, mex, a.spot, war, ordered);
                return;
            }
            self.approach.remove(&unit.id());
        }
        // 2) Pick a new target. CircuitAI expands at the frontier (clusters
        // toward midfield), not only the home half, and never through a hot
        // spot: enemy near the spot is only OK with local superiority.
        let target = (0..ctx.spots.len())
            .filter(|i| !terr.owned[*i] && !terr.enemy[*i] && !ordered.contains(i) && terr.not_blocked(ctx, *i))
            .filter(|i| {
                let spot = ctx.spots[*i];
                let superior = threat.local_superiority(ctx, roles, spot, 400.0, 700.0);
                let safe = superior || !threat.enemy_near(spot, 700.0);
                safe && (me.distance_2d(spot) <= 200.0
                    || ctx.our_start.distance_2d(spot) <= 1600.0
                    || terr.front.distance_2d(spot) <= 1400.0
                    || threat.local_superiority(ctx, roles, spot, ESCORT_ARMY_RADIUS, ESCORT_ENEMY_RADIUS))
            })
            .min_by(|a, b| {
                // prefer frontier spots (near the front), then nearest to us
                let key = |i: &usize| {
                    let s = ctx.spots[*i];
                    (terr.front.distance_2d(s) * 2.0 + me.distance_2d(s)) as i64
                };
                key(a).cmp(&key(b))
            });
        let Some(i) = target else {
            // nothing left to expand to: finish an abandoned mex nanoframe
            // if there is one, else hold the front line
            if repair_unfinished_mex(ctx, &unit) {
                return;
            }
            if me.distance_2d(terr.front) > 300.0 {
                unit.move_to(terr.front, opt::NONE);
            }
            return;
        };
        self.send_mex_chain(ctx, &unit, mex, i, war, ordered);
        // 3) secure the cluster once the mex is up: lotus, plus a picket past
        // the 3rd cluster or 14 total mexes
        if terr.owned[i] {
            self.approach.remove(&unit.id());
            let owned_count = terr.owned.iter().filter(|o| **o).count();
            let picket_needed = self.clusters_secured >= 3 || owned_count > 14;
            let lotus = ctx.defs.turret_ladder.first().copied();
            let def = if picket_needed { ctx.defs.picket.or(lotus) } else { lotus };
            if let Some(d) = def {
                let cost = metal_cost(ctx, d);
                if war.can_spend(ctx, ProjectKind::Porc, cost)
                    && cur >= cost
                    && build_at(cb, unit, d, ctx.spots[i], 200.0)
                {
                    if !picket_needed {
                        self.clusters_secured += 1;
                    }
                    cb.log(&format!(
                        "[planner] con {} secured mex {} with {}{}",
                        unit.id(),
                        i,
                        d.name(),
                        if picket_needed { " (picket)" } else { "" }
                    ));
                }
            }
        }
        self.escort(ctx, &unit, roles);
    }

    /// A reclaimer: nearest wreck, else its taskgroup's frontline position
    /// (pooled, mid game), else patrol the home centroid.
    fn reclaim_step(&mut self, unit: Unit, terr: &Territory, pooled: bool, group_pos: Option<Vec3>) {
        let me = unit.pos();
        let wreck = self
            .wrecks
            .iter()
            .min_by(|a, b| me.distance_2d(**a).partial_cmp(&me.distance_2d(**b)).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(w) = wreck
            && me.distance_2d(*w) <= 1600.0
        {
            if me.distance_2d(*w) > 96.0 {
                unit.move_to(*w, opt::NONE);
            } else {
                unit.reclaim_area(*w, 64.0, opt::NONE);
            }
            return;
        }
        let home = if pooled { group_pos.unwrap_or(terr.front) } else { terr.our_centroid };
        if me.distance_2d(home) > 400.0 {
            unit.move_to(home, opt::NONE);
        } else {
            unit.patrol_to(home, opt::NONE);
        }
    }

    /// Big plans, checked once a minute once the first fusion / advanced geo is
    /// online: Lucifer, Desolator, Cerberus, missile silo, Bertha (late game),
    /// built at the front line on High priority.
    fn big_projects(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        if ctx.frame - self.last_big_check < BIG_CHECK_FRAMES {
            return;
        }
        self.last_big_check = ctx.frame;
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let fusions = cb
            .team_units()
            .iter()
            .filter(|u| u.def().is_some_and(|d| d.name() == "energyfusion") && !u.is_being_built())
            .count();
        let adv_geo = cb.team_units().iter().any(|u| u.def() == ctx.defs.adv_geo && !u.is_being_built());
        if fusions == 0 && !adv_geo {
            return;
        }
        let busy = war.projects.iter().any(|p| {
            matches!(p.state, ProjectState::Queued | ProjectState::Active)
                && matches!(p.kind, ProjectKind::HeavyPorc | ProjectKind::Silo)
        });
        if busy {
            return;
        }
        let (name, key) = BIG_PROJECTS[self.big_idx % BIG_PROJECTS.len()];
        self.big_idx += 1;
        let Some(def) = ctx.defs.get(key).or_else(|| ctx.defs.by_human(key)) else {
            cb.log(&format!("[planner] big project {name}: def '{key}' not found, skipping"));
            return;
        };
        let cost = def.cost(metal);
        let kind = if name == "silo" { ProjectKind::Silo } else { ProjectKind::HeavyPorc };
        if !war.request_priority(ctx, name, kind, Some(def), cost, 0.5) {
            return;
        }
        let pos = Vec3::xz((terr.front.x + terr.enemy_centroid.x) / 2.0, (terr.front.z + terr.enemy_centroid.z) / 2.0);
        let builder = cb
            .team_units()
            .into_iter()
            .filter(|u| {
                roles.get(&u.id()) == Some(&Role::Worker) && !u.is_being_built() && u.num_current_commands() == 0
            })
            .min_by(|a, b| {
                a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(w) = builder
            && build_at(cb, w, def, pos, 500.0)
        {
            cb.log(&format!("[planner] big project {name} ({}) at ({:.0},{:.0})", def.name(), pos.x, pos.z));
        }
    }
}
