//! Economy: the scripted opening (1 energy constructor, 1 con + 5 raiders,
//! factory, 3 surrounding mexes, wind grid linking them, >=2 solars), the
//! energy policy (>=60% static, wind mainly at >=0.6 wind), mex expansion per
//! the territory policy, and the war_chest-gated strategic projects
//! (storages, geothermal, fusion 1/2, singularity).

use std::collections::{HashMap, HashSet};

use springai::{Callback, NO_TIMEOUT, Unit, UnitDef, Vec3, opt};

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
    // 60 s order timeout (CircuitAI pattern): a dropped build is retried by
    // timeout instead of sitting in the queue forever.
    let timeout = cb.game().current_frame() + 1800;
    let ok = unit.build_timed(def, Vec3::new(site.x, y, site.z), 0, opt::NONE, timeout);
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
pub fn build_mex(cb: Callback, unit: Unit, mex: UnitDef, spot: Vec3, opts: i16) -> bool {
    // A shift-queued build waits behind a possibly long walk, so its timeout
    // must not expire mid-walk — keep it open there. Direct (in-range) builds
    // get the 60 s CircuitAI-style timeout: if the engine drops them, the
    // order finishes by timeout and queue verification re-sends.
    let timeout = if opts & opt::SHIFT_KEY != 0 { NO_TIMEOUT } else { cb.game().current_frame() + 1800 };
    if unit.build_timed(mex, spot, 0, opts, timeout) {
        cb.log(&format!(
            "[eco] build_mex {} unit={} at spot ({:.1},{:.1}) direct{}",
            mex.name(),
            unit.id(),
            spot.x,
            spot.z,
            if opts & opt::SHIFT_KEY != 0 { " (shift-queued)" } else { "" }
        ));
        return true;
    }
    build_at(cb, unit, mex, spot, 64.0)
}

/// Repair an unfinished mex nanoframe of ours (e.g. the con that started it
/// died) — CircuitAI's MexTask does `Repair(target)` when a nanoframe exists.
/// Returns true if a repair order was issued.
pub fn repair_unfinished_mex(ctx: &Ctx, unit: &Unit) -> bool {
    let cb = ctx.cb;
    let Some(mex) = ctx.defs.mex else { return false };
    if unit.num_current_commands() > 0 {
        return false;
    }
    let me = unit.pos();
    let nano = cb.team_units().into_iter().filter(|u| u.def() == Some(mex) && u.is_being_built()).min_by(|a, b| {
        a.pos().distance_2d(me).partial_cmp(&b.pos().distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(n) = nano
        && me.distance_2d(n.pos()) <= 2500.0
    {
        return unit.repair(n, opt::NONE);
    }
    false
}

/// Build a factory at raw spiral site `t` around `base` — NOT via
/// find_closest_build_site: its buildability test disagrees with the engine's
/// TestUnitBuildSquare often enough that the build is silently dropped
/// (CBuilderCAI::IsBuildPosBlocked). Raw positions are what actually work.
/// Skips sites in a friendly factory's exit lane.
pub fn build_factory_spiral(
    ctx: &Ctx,
    roles: &HashMap<i32, Role>,
    unit: Unit,
    f: UnitDef,
    base: Vec3,
    t: usize,
) -> bool {
    let cb = ctx.cb;
    let angle = (t % 8) as f32 * std::f32::consts::FRAC_PI_4;
    let radius = 96.0 + 80.0 * (t / 8) as f32;
    let site = Vec3::xz(base.x + angle.cos() * radius, base.z + angle.sin() * radius);
    if blocks_factory_exit(ctx, roles, site) {
        return false;
    }
    let y = cb.map().elevation_at(site.x, site.z);
    let timeout = cb.game().current_frame() + 1800;
    cb.log(&format!("[eco] factory try {t} f={} def={} site=({:.0},{:.0})", ctx.frame, f.name(), site.x, site.z));
    unit.build_timed(f, Vec3::new(site.x, y, site.z), 0, opt::NONE, timeout)
}

/// True if the unit's command queue already contains a build order for `mex`
/// at (within 96 elmos of) `spot` — zkgbai's `queued_build_matches`: the engine
/// can silently drop a build (blocked footprint, AllowCommand denial), and we
/// must not stack duplicate orders nor assume the order is still alive.
pub fn has_queued_mex_build(unit: &Unit, mex: UnitDef, spot: Vec3) -> bool {
    let want = -mex.id();
    unit.current_commands().iter().any(|c| {
        c.id() == want
            && c.params().len() >= 3
            && (c.params()[0] - spot.x).abs() < 96.0
            && (c.params()[2] - spot.z).abs() < 96.0
    })
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

/// Nearest empty (not ours, not enemy's, not recently denied, not already
/// ordered) metal spot to `near` — the commander's "next closest empty mex".
fn mex_candidate(ctx: &Ctx, terr: &Territory, ordered: &HashSet<usize>, near: Vec3) -> Option<usize> {
    (0..ctx.spots.len())
        .filter(|i| !terr.owned[*i] && !terr.enemy[*i] && !ordered.contains(i) && terr.not_blocked(ctx, *i))
        .min_by(|a, b| {
            ctx.spots[*a]
                .distance_2d(near)
                .partial_cmp(&ctx.spots[*b].distance_2d(near))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
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
    /// Opening-factory placement retry: GiveOrder returning true only means
    /// the command was queued — CBuilderCAI silently drops builds whose
    /// footprint is blocked, and a later non-shift order replaces a pending
    /// one. So we retry on a spiral of sites until a factory nanoframe exists.
    factory_tries: usize,
    factory_last_try: i32,
    /// Commander's current opening-mex approach: (spot, sent frame, best
    /// distance) — for stall detection so we don't re-issue the move+build
    /// chain every tick.
    com_mex: Option<(usize, i32, f32)>,
}

impl Economy {
    /// Current opening phase name (operator view).
    pub fn opening_tag(&self) -> &'static str {
        match self.opening {
            Opening::Factory => "factory",
            Opening::Con => "con",
            Opening::Mexes => "mexes",
            Opening::WindGrid => "wind-grid",
            Opening::Solars => "solars",
            Opening::Done => "done",
        }
    }

    /// Spot index the commander is currently working on, if any.
    pub fn com_mex_spot(&self) -> Option<usize> {
        self.com_mex.map(|(s, _, _)| s)
    }

    pub fn new() -> Self {
        Economy {
            opening: Opening::Factory,
            opening_mexes: Vec::new(),
            wind_built: 0,
            solars_built: 0,
            grid_pylons: Vec::new(),
            pylons_built: 0,
            lotus_built: false,
            factory_tries: 0,
            factory_last_try: -1000,
            com_mex: None,
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
                let Some(f) = ctx.defs.factory else {
                    self.opening = Opening::Con;
                    return;
                };
                // Advance only once a factory nanoframe/unit actually exists:
                // GiveOrder's true only means "queued" — a blocked footprint
                // (IsBuildPosBlocked) or a later non-shift order silently
                // drops the build and no nanoframe ever appears.
                if roles.values().any(|r| *r == Role::Factory) || cb.team_units().iter().any(|u| u.def() == Some(f)) {
                    if self.opening_mexes.is_empty() {
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
                    }
                    // don't wait for the factory's first con to finish: the
                    // factory builds cons itself (factory_idle), the commander
                    // goes capture the opening mexes right away
                    self.opening = Opening::Mexes;
                    return;
                }
                // Retry only when the commander has no pending order (the
                // previous one was dropped or finished without a nanoframe),
                // or when it has been stuck queued for too long.
                let pending = unit.num_current_commands() > 0;
                if ctx.frame - self.factory_last_try < 30 || (pending && ctx.frame - self.factory_last_try < 300) {
                    return;
                }
                self.factory_last_try = ctx.frame;
                // The first factory is free (ZK gadget discount): don't gate
                // on `cur >= cost` — with the storage cap pinning metal below
                // the nominal cost we would never build anything.
                if !war.can_spend(ctx, ProjectKind::Expansion, 0.0) {
                    return;
                }
                // Spiral of candidate sites around our real spawn position: at
                // f=0 the sent start pos may not match where the engine placed
                // us, and the nearest site can be blocked (river/slope).
                let t = self.factory_tries;
                self.factory_tries += 1;
                build_factory_spiral(ctx, roles, unit, f, ctx.our_start, t);
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
                // advance only when every opening mex is actually finished
                // (owned) or its order was denied (blocked) — not merely
                // ordered, or the commander walks off leaving nanoframes
                if self.opening_mexes.iter().all(|i| terr.owned[*i] || !terr.not_blocked(ctx, *i)) {
                    self.grid_pylons = Self::plan_grid_pylons(ctx, &self.opening_mexes);
                    self.opening = Opening::WindGrid;
                    return;
                }
                // Standing job: the commander is ALWAYS working a mex — the
                // nearest empty spot to its current position. Send move+build
                // as one SHIFT chain (zkgbai pattern): the build stays queued
                // behind the walk and the commander keeps building until the
                // mex is finished — it must not walk off leaving a nanoframe.
                // Re-issue only when the queue lost the build (engine dropped
                // it) or the approach stalled.
                if let Some(i) = mex_candidate(ctx, terr, ordered, unit.pos()) {
                    let spot = ctx.spots[i];
                    let d = unit.pos().distance_2d(spot);
                    let busy = unit.num_current_commands() > 0;
                    let has_build = has_queued_mex_build(&unit, mex, spot);
                    let stalled =
                        self.com_mex.is_some_and(|(s, sent, best)| s == i && ctx.frame - sent > 300 && d > best - 32.0);
                    // Mexes are paid per frame out of build power, not up
                    // front — don't gate on `cur >= cost`, or the commander
                    // idles with metal sitting in storage.
                    if !has_build && (!busy || stalled) && war.can_spend(ctx, ProjectKind::Expansion, 0.0) {
                        if d > 200.0 {
                            unit.move_to(spot, opt::NONE);
                            build_mex(cb, unit, mex, spot, opt::SHIFT_KEY);
                        } else {
                            build_mex(cb, unit, mex, spot, opt::NONE);
                        }
                        ordered.insert(i);
                        self.com_mex = Some((i, ctx.frame, d));
                        return;
                    }
                    // Can't build right now (queue busy / budget window closed):
                    // at least keep walking toward the closest empty spot.
                    if !busy && !has_build {
                        unit.move_to(spot, opt::NONE);
                        self.com_mex = Some((i, ctx.frame, d));
                    }
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
        {
            // Same standing job as the opening: build the expansion spot, or
            // at least walk to it when no build can be issued. Mexes are paid
            // per frame from build power — no `cur >= cost` gate.
            let spot = ctx.spots[i];
            let d = unit.pos().distance_2d(spot);
            let busy = unit.num_current_commands() > 0;
            let has_build = has_queued_mex_build(&unit, mex, spot);
            let stalled =
                self.com_mex.is_some_and(|(s, sent, best)| s == i && ctx.frame - sent > 300 && d > best - 32.0);
            if !has_build && (!busy || stalled) && war.can_spend(ctx, ProjectKind::Expansion, 0.0) {
                // move + build as one SHIFT chain so the commander keeps
                // building until the mex is finished instead of walking off
                if d > 200.0 {
                    unit.move_to(spot, opt::NONE);
                    build_mex(cb, unit, mex, spot, opt::SHIFT_KEY);
                } else {
                    build_mex(cb, unit, mex, spot, opt::NONE);
                }
                ordered.insert(i);
                self.com_mex = Some((i, ctx.frame, d));
                return;
            }
            if !busy && !has_build {
                unit.move_to(spot, opt::NONE);
                self.com_mex = Some((i, ctx.frame, d));
            }
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
        // finish abandoned mex nanoframes first (CircuitAI repairs the target)
        if repair_unfinished_mex(ctx, &unit) {
            return;
        }
        if let Some(i) = terr.expansion_spot(ctx, ordered)
            && war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, mex))
            && cur >= metal_cost(ctx, mex)
        {
            // move + build as one SHIFT chain so the worker keeps building
            // until the mex is finished instead of walking off
            let spot = ctx.spots[i];
            if unit.pos().distance_2d(spot) > 200.0 {
                unit.move_to(spot, opt::NONE);
                build_mex(cb, unit, mex, spot, opt::SHIFT_KEY);
            } else {
                build_mex(cb, unit, mex, spot, opt::NONE);
            }
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
