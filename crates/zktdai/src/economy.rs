//! Economy: the scripted opening (1 energy constructor, 1 con + 5 raiders,
//! factory, 3 surrounding mexes, wind grid linking them, >=2 solars), the
//! energy policy (>=60% static, wind mainly at >=0.6 wind), mex expansion per
//! the territory policy, and the war_chest-gated strategic projects
//! (storages, geothermal, fusion 1/2, singularity).

use std::collections::{HashMap, HashSet};

use springai::{Callback, Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::planner::Planner;
use crate::territory::Territory;
use crate::war_chest::{ProjectKind, WarChest};

/// Order `unit` to build `def` as close to `want` as the map allows.
pub fn build_at(cb: Callback, unit: Unit, def: UnitDef, want: Vec3, search_radius: f32) -> bool {
    let site = cb.map().find_closest_build_site(def, want, search_radius, 0, 0);
    if site.x < 0.0 {
        return false;
    }
    let y = cb.map().elevation_at(site.x, site.z);
    cb.log(&format!(
        "[eco] build_at {} unit={} at ({:.0},{:.0}) from ({:.0},{:.0}) dist={:.0} want ({:.0},{:.0})",
        def.name(),
        unit.id(),
        site.x,
        site.z,
        unit.pos().x,
        unit.pos().z,
        unit.pos().distance_2d(site),
        want.x,
        want.z
    ));
    let ok = unit.build(def, Vec3::new(site.x, y, site.z), 0, opt::NONE);
    if !ok {
        cb.log(&format!(
            "[eco] build {} by unit {} REJECTED (site {:.0},{:.0})",
            def.name(),
            unit.id(),
            site.x,
            site.z
        ));
    }
    ok
}

/// Order a mex exactly on its spot position. ZK's mex-placement gadget only
/// allows mex builds whose (raw) position matches a published metal spot — or,
/// for AI teams, is within 50 elmos of an unclaimed one — so building at the
/// raw spot coords (which match the rules params we read in Init) is what gets
/// accepted; the engine snaps the footprint itself. Falls back to a nearby
/// snapped site if the direct order is refused.
pub fn build_mex(cb: Callback, unit: Unit, mex: UnitDef, spot: Vec3) -> bool {
    if unit.build(mex, spot, 0, opt::NONE) {
        cb.log(&format!(
            "[eco] build_mex {} unit={} at spot ({:.1},{:.1}) direct",
            mex.name(),
            unit.id(),
            spot.x,
            spot.z
        ));
        return true;
    }
    build_at(cb, unit, mex, spot, 64.0)
}

/// True if `site` would block a friendly factory's exit lane. Factories need a
/// clear corridor on their front side (toward the map centre / enemy) for
/// built units to path out — power or other structures placed in that sector
/// trap the factory's output. Factories/plates themselves are exempt.
pub fn blocks_factory_exit(ctx: &Ctx, roles: &HashMap<i32, Role>, site: Vec3) -> bool {
    let cb = ctx.cb;
    let centre = Vec3::xz(cb.map().width() as f32 * 4.0, cb.map().height() as f32 * 4.0);
    for u in cb.team_units() {
        if roles.get(&u.id()) != Some(&Role::Factory) || u.is_being_built() {
            continue;
        }
        let fp = u.pos();
        let s = Vec3::xz(site.x - fp.x, site.z - fp.z);
        let sl = (s.x * s.x + s.z * s.z).sqrt();
        if sl > 280.0 {
            continue;
        }
        if sl < 1.0 {
            return true; // on top of the factory
        }
        let f = Vec3::xz(centre.x - fp.x, centre.z - fp.z);
        let fl = (f.x * f.x + f.z * f.z).sqrt().max(1.0);
        let dot = (f.x * s.x + f.z * s.z) / (fl * sl);
        if dot > 0.5 {
            return true; // within ~60 deg of the front lane
        }
    }
    false
}

/// Like `build_at`, but refuses sites that would block a factory's exit lane
/// (used for power/lotus/pylon placement around the base).
pub fn build_at_clear(
    cb: Callback,
    ctx: &Ctx,
    roles: &HashMap<i32, Role>,
    unit: Unit,
    def: UnitDef,
    want: Vec3,
    search_radius: f32,
) -> bool {
    let site = cb.map().find_closest_build_site(def, want, search_radius, 0, 0);
    if site.x < 0.0 {
        return false;
    }
    if blocks_factory_exit(ctx, roles, site) {
        cb.log(&format!("[eco] {} at ({:.0},{:.0}) rejected: blocks factory exit", def.name(), site.x, site.z));
        return false;
    }
    let y = cb.map().elevation_at(site.x, site.z);
    unit.build(def, Vec3::new(site.x, y, site.z), 0, opt::NONE)
}

fn current_metal(ctx: &Ctx) -> f32 {
    ctx.metal.map(|m| ctx.cb.economy().current(m)).unwrap_or(0.0)
}

/// Debug cadence for one-shot logs: every 600 frames.
fn frame_dbg(ctx: &Ctx) -> bool {
    ctx.frame % 600 == 0
}

pub fn metal_cost(ctx: &Ctx, def: UnitDef) -> f32 {
    ctx.defs.metal().map(|m| def.cost(m)).unwrap_or(f32::MAX)
}

/// A static power plant to build when energy is short. Wind only on maps with
/// enough wind (>= 0.6 of the 2.5 cap), otherwise solar.
pub fn power_def(ctx: &Ctx) -> Option<UnitDef> {
    let fallback = || ctx.defs.get("energywind");
    if ctx.wind_max >= 0.6 * 2.5 {
        ctx.defs.wind.or(ctx.defs.solar).or_else(fallback)
    } else {
        ctx.defs.solar.or(ctx.defs.wind).or_else(fallback)
    }
}

pub fn wants_energy(ctx: &Ctx) -> bool {
    let Some(energy) = ctx.energy else { return false };
    let eco = ctx.cb.economy();
    eco.income(energy) < eco.usage(energy) + 2.0
}

/// The scripted opening: the free factory first (constructors can only be
/// built by factories/plates/athena/engineer-commander, gated behind placing a
/// factory — the first factory is free), then the factory builds the first
/// constructor while the commander assists it, then commander + constructor
/// build the 3 home mexes in tandem, then the wind grid linking them and 2
/// solars.
enum Opening {
    Factory,
    Con,
    Mexes,
    WindGrid,
    Solars,
    Done,
}

pub struct Economy {
    opening: Opening,
    /// The 3 mexes around the start, chosen when the factory goes down.
    opening_mexes: Vec<usize>,
    wind_built: usize,
    solars_built: usize,
    /// Pylon positions linking the opening mexes into the energy grid
    /// (base -> mex1 -> mex2 -> ...), computed when the mexes are chosen.
    grid_pylons: Vec<Vec3>,
    pylons_built: usize,
    /// The single home lotus (near factory/mexes) is built once.
    lotus_built: bool,
}

impl Economy {
    pub fn new() -> Self {
        Economy {
            opening: Opening::Factory,
            opening_mexes: Vec::new(),
            wind_built: 0,
            solars_built: 0,
            grid_pylons: Vec::new(),
            pylons_built: 0,
            lotus_built: false,
        }
    }

    /// Pylon positions spaced along the chain base -> each opening mex, so
    /// every mex ends up inside the energy grid (mex overdrive needs it).
    fn plan_grid_pylons(ctx: &Ctx, opening_mexes: &[usize]) -> Vec<Vec3> {
        let Some(pylon) = ctx.defs.pylon else { return Vec::new() };
        let range =
            pylon.custom_param("pylonrange").and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(450.0).max(64.0);
        let mut chain = vec![ctx.our_start];
        chain.extend(opening_mexes.iter().map(|i| ctx.spots[*i]));
        let mut out = Vec::new();
        for w in chain.windows(2) {
            let (a, b) = (w[0], w[1]);
            let dist = a.distance_2d(b);
            let steps = (dist / (range * 0.9)).ceil().max(1.0) as i32;
            for s in 1..steps {
                let t = s as f32 / steps as f32;
                out.push(Vec3::xz(a.x + (b.x - a.x) * t, a.z + (b.z - a.z) * t));
            }
        }
        out
    }

    pub fn commander_idle(
        &mut self,
        ctx: &Ctx,
        unit: Unit,
        roles: &HashMap<i32, Role>,
        ordered: &mut HashSet<usize>,
        terr: &mut Territory,
        war: &WarChest,
    ) {
        let cb = ctx.cb;
        if ctx.metal.is_none() {
            return;
        }
        let cur = current_metal(ctx);
        match self.opening {
            Opening::Factory => {
                if let Some(f) = ctx.defs.factory {
                    // The first factory is free (ZK gadget discount): don't gate
                    // it on `cur >= cost` — with the storage cap pinning metal
                    // below the nominal cost we would never build anything.
                    let first_free = !roles.values().any(|r| *r == Role::Factory);
                    let cost = if first_free { 0.0 } else { metal_cost(ctx, f) };
                    if frame_dbg(ctx) {
                        cb.log(&format!(
                            "[eco] com idle f={} opening=factory def={} cost={:.0} cur={:.0} first_free={first_free}",
                            ctx.frame,
                            f.name(),
                            metal_cost(ctx, f),
                            cur
                        ));
                    }
                    // Build next to the commander's real position: at f=0 the
                    // sent start pos may not match where the engine actually
                    // spawned us, and ctx.our_start can still be map centre.
                    let built = war.can_spend(ctx, ProjectKind::Expansion, cost)
                        && (first_free || cur >= cost)
                        && build_at(cb, unit, f, unit.pos(), 400.0);
                    if frame_dbg(ctx) || built {
                        cb.log(&format!("[eco] factory build attempt f={} built={built}", ctx.frame));
                    }
                    if built {
                        // capture the 3 surrounding mexes for the wind grid
                        let mut spots: Vec<usize> =
                            (0..ctx.spots.len()).filter(|i| !terr.owned[*i] && !terr.enemy[*i]).collect();
                        spots.sort_by(|a, b| {
                            ctx.spots[*a]
                                .distance_2d(ctx.our_start)
                                .partial_cmp(&ctx.spots[*b].distance_2d(ctx.our_start))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        self.opening_mexes = spots.into_iter().take(3).collect();
                        // don't wait for the factory's first con to finish:
                        // the factory builds cons itself (factory_idle), the
                        // commander goes capture the opening mexes right away
                        self.opening = Opening::Mexes;
                    }
                } else {
                    self.opening = Opening::Con;
                }
                return;
            }
            Opening::Con => {
                // the factory builds the first con itself; the commander only
                // assists while no factory exists yet, then moves to the mexes
                if roles.values().any(|r| *r == Role::Factory) || roles.values().any(|r| *r == Role::Worker) {
                    self.opening = Opening::Mexes;
                    return;
                }
                if let Some(con) = ctx.defs.con
                    && let Some(nano) = cb.team_units().into_iter().find(|u| u.def() == Some(con) && u.is_being_built())
                {
                    unit.guard(nano, opt::NONE);
                }
                return;
            }
            Opening::Mexes => {
                let Some(mex) = ctx.defs.mex else {
                    self.opening = Opening::WindGrid;
                    return;
                };
                if self.opening_mexes.iter().all(|i| ordered.contains(i) || terr.owned[*i]) {
                    self.grid_pylons = Self::plan_grid_pylons(ctx, &self.opening_mexes);
                    self.opening = Opening::WindGrid;
                    return;
                }
                // tandem: workers take the same spots via expansion_spot
                if let Some(i) = self.opening_mexes.iter().copied().find(|i| !ordered.contains(i) && !terr.owned[*i])
                    && cur >= metal_cost(ctx, mex)
                    && build_mex(cb, unit, mex, ctx.spots[i])
                {
                    ordered.insert(i);
                }
                return;
            }
            Opening::WindGrid => {
                // First link the mexes into the energy grid: one pylon per
                // planned position along base -> mex chain.
                if let Some(pylon) = ctx.defs.pylon
                    && let Some(p) = self.grid_pylons.get(self.pylons_built).copied()
                    && war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, pylon))
                    && cur >= metal_cost(ctx, pylon)
                    && build_at_clear(cb, ctx, roles, unit, pylon, p, 160.0)
                {
                    self.pylons_built += 1;
                    return;
                }
                // Then turbines between the surrounding mexes, only on windy maps
                if ctx.wind_max >= 0.6 * 2.5
                    && self.wind_built < 6
                    && let Some(wind) = ctx.defs.wind
                    && cur >= metal_cost(ctx, wind)
                    && self.opening_mexes.len() >= 2
                {
                    // midpoint between two consecutive opening mexes
                    let i = self.wind_built % (self.opening_mexes.len() - 1);
                    let a = ctx.spots[self.opening_mexes[i]];
                    let b = ctx.spots[self.opening_mexes[i + 1]];
                    let mid = Vec3::xz((a.x + b.x) / 2.0, (a.z + b.z) / 2.0);
                    if build_at_clear(cb, ctx, roles, unit, wind, mid, 200.0) {
                        self.wind_built += 1;
                    }
                    return;
                }
                self.opening = Opening::Solars;
            }
            Opening::Solars => {
                if self.solars_built < 2
                    && let Some(solar) = ctx.defs.solar
                {
                    if cur >= metal_cost(ctx, solar)
                        && build_at_clear(cb, ctx, roles, unit, solar, ctx.our_start, 300.0)
                    {
                        self.solars_built += 1;
                    }
                    return;
                }
                self.opening = Opening::Done;
            }
            Opening::Done => {}
        }
        // steady-state commander: one home lotus near the factory/mexes
        // (porc-gated, built once), then expansion, energy, factory rebuild
        if !self.lotus_built
            && let Some(lotus) = ctx.defs.turret_ladder.first().copied()
        {
            let already =
                cb.team_units().iter().any(|u| u.def() == Some(lotus) && u.pos().distance_2d(ctx.our_start) < 600.0);
            let built = !already
                && war.can_spend(ctx, ProjectKind::Porc, metal_cost(ctx, lotus))
                && cur >= metal_cost(ctx, lotus)
                && build_at_clear(cb, ctx, roles, unit, lotus, ctx.our_start, 400.0);
            if already || built {
                self.lotus_built = true;
                return;
            }
        }
        if let Some(mex) = ctx.defs.mex
            && let Some(i) = terr.expansion_spot(ctx, ordered)
            && war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, mex))
            && cur >= metal_cost(ctx, mex)
            && build_mex(cb, unit, mex, ctx.spots[i])
        {
            ordered.insert(i);
            return;
        }
        // solar backup when below 80% energy parity (income < 0.8 * usage)
        if let Some(energy) = ctx.energy
            && let Some(solar) = ctx.defs.solar
        {
            let eco = cb.economy();
            let usage = eco.usage(energy);
            if usage > 0.0
                && eco.income(energy) < 0.8 * usage
                && war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, solar))
                && cur >= metal_cost(ctx, solar)
                && build_at_clear(cb, ctx, roles, unit, solar, ctx.our_start, 300.0)
            {
                return;
            }
        }
        if wants_energy(ctx)
            && let Some(power) = power_def(ctx)
            && war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, power))
            && cur >= metal_cost(ctx, power)
            && build_at_clear(cb, ctx, roles, unit, power, ctx.our_start, 300.0)
        {
            return;
        }
        let has_factory = roles.values().any(|r| *r == Role::Factory);
        if !has_factory
            && let Some(f) = ctx.defs.factory
            && war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, f))
            && cur >= metal_cost(ctx, f)
        {
            build_at(cb, unit, f, ctx.our_start, 400.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn worker_idle(
        &mut self,
        ctx: &Ctx,
        unit: Unit,
        roles: &HashMap<i32, Role>,
        ordered: &mut HashSet<usize>,
        terr: &mut Territory,
        war: &WarChest,
        fob_mobile: &HashSet<i32>,
        planner: &Planner,
    ) {
        let cb = ctx.cb;
        if ctx.metal.is_none() {
            return;
        }
        // constructors assigned to the forward base hold it instead
        if fob_mobile.contains(&unit.id()) {
            return;
        }
        // constructors with a planner job (energy/base/expand/reclaim) are run
        // by the planner; this is the fallback for unassigned workers
        if planner.has_job(unit.id()) {
            return;
        }
        let Some(mex) = ctx.defs.mex else { return };
        let cur = current_metal(ctx);
        if let Some(i) = terr.expansion_spot(ctx, ordered)
            && war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, mex))
            && cur >= metal_cost(ctx, mex)
            && build_mex(cb, unit, mex, ctx.spots[i])
        {
            ordered.insert(i);
            return;
        }
        if wants_energy(ctx)
            && let Some(power) = power_def(ctx)
            && war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, power))
            && cur >= metal_cost(ctx, power)
            && build_at_clear(cb, ctx, roles, unit, power, ctx.our_start, 300.0)
        {
            return;
        }
        let home = Vec3::xz(ctx.our_start.x, ctx.our_start.z);
        if unit.pos().distance_2d(home) > 400.0 {
            unit.move_to(home, opt::NONE);
        } else {
            unit.patrol_to(home, opt::NONE);
        }
    }

    /// Strategic projects, gated by the ledger. Called every ~10 s.
    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let income = cb.economy().income(metal);
        let has_project = |war: &WarChest, kind: ProjectKind| {
            war.projects.iter().any(|p| {
                p.kind == kind
                    && (p.state == crate::war_chest::ProjectState::Queued
                        || p.state == crate::war_chest::ProjectState::Active)
            })
        };
        // storages are conditional: one per 35 m/s income milestone (cap 4),
        // plus one immediately if the commander is dead
        let commander_alive = roles.values().any(|r| *r == Role::Commander);
        let milestones = ((income / 35.0) as usize).min(4);
        let target = milestones.max(if commander_alive { 0 } else { 1 });
        let storages = cb.team_units().iter().filter(|u| u.def() == ctx.defs.storage).count()
            + war
                .projects
                .iter()
                .filter(|p| {
                    p.kind == ProjectKind::Storage
                        && matches!(
                            p.state,
                            crate::war_chest::ProjectState::Queued | crate::war_chest::ProjectState::Active
                        )
                })
                .count();
        if let Some(s) = ctx.defs.storage
            && storages < target
            && !has_project(war, ProjectKind::Storage)
        {
            let cost = metal_cost(ctx, s);
            if war.request(ctx, "storage", ProjectKind::Storage, Some(s), cost, 0.0)
                && let Some(w) = idle_worker(ctx, roles, ctx.our_start, 900.0)
            {
                build_at(cb, w, s, ctx.our_start, 300.0);
            }
        }
        // geothermal: priority once a geospot is on our half near the start
        if let Some(geo) = ctx.defs.geo {
            let near = ctx.geo_spots.iter().any(|g| g.distance_2d(ctx.our_start) < 1000.0);
            if near && income >= 25.0 && !has_project(war, ProjectKind::Energy) {
                let cost = metal_cost(ctx, geo);
                if war.request(ctx, "geo", ProjectKind::Energy, Some(geo), cost, 0.2)
                    && let Some(w) = idle_worker(ctx, roles, ctx.our_start, 1200.0)
                {
                    let spot = ctx
                        .geo_spots
                        .iter()
                        .min_by(|a, b| {
                            a.distance_2d(ctx.our_start)
                                .partial_cmp(&b.distance_2d(ctx.our_start))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .copied();
                    if let Some(g) = spot {
                        build_at(cb, w, geo, g, 64.0);
                    }
                }
            }
        }
        // first fusion at 40-50 m/s, pausing other production (army checks the ledger)
        if let Some(fusion) = ctx.defs.fusion {
            let fusions_alive =
                cb.team_units().iter().filter(|u| u.def().is_some_and(|d| d.name() == "energyfusion")).count();
            if income >= 40.0 && fusions_alive == 0 && !has_project(war, ProjectKind::Energy) {
                let cost = metal_cost(ctx, fusion);
                // high priority: the builder draws from storage first while others throttle
                if war.request_priority(ctx, "fusion1", ProjectKind::Energy, Some(fusion), cost, 0.5)
                    && let Some(w) = idle_worker(ctx, roles, ctx.our_start, 1600.0)
                {
                    build_at(cb, w, fusion, ctx.our_start, 400.0);
                }
            }
        }
        let _ = terr;
    }
}

fn idle_worker(ctx: &Ctx, roles: &HashMap<i32, Role>, pos: Vec3, max_dist: f32) -> Option<Unit> {
    ctx.cb
        .team_units()
        .into_iter()
        .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
        .filter(|u| u.pos().distance_2d(pos) <= max_dist)
        .min_by(|a, b| {
            a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
        })
}
