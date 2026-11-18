//! Economy: the scripted opening (1 energy constructor, 1 con + 5 raiders,
//! factory, 3 surrounding mexes, wind grid linking them, >=2 solars), the
//! energy policy (>=60% static, wind mainly at >=0.6 wind), mex expansion per
//! the territory policy, and the war_chest-gated strategic projects
//! (storages, geothermal, fusion 1/2, singularity).

use std::collections::{HashMap, HashSet};

use springai::{Callback, Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;
use crate::war_chest::{ProjectKind, WarChest};

/// Order `unit` to build `def` as close to `want` as the map allows.
pub fn build_at(cb: Callback, unit: Unit, def: UnitDef, want: Vec3, search_radius: f32) -> bool {
    let site = cb.map().find_closest_build_site(def, want, search_radius, 0, 0);
    if site.x < 0.0 {
        return false;
    }
    let y = cb.map().elevation_at(site.x, site.z);
    unit.build(def, Vec3::new(site.x, y, site.z), 0, opt::NONE)
}

fn current_metal(ctx: &Ctx) -> f32 {
    ctx.metal.map(|m| ctx.cb.economy().current(m)).unwrap_or(0.0)
}

fn metal_cost(ctx: &Ctx, def: UnitDef) -> f32 {
    ctx.metal.map(|m| def.cost(m)).unwrap_or(f32::MAX)
}

/// A static power plant to build when energy is short. Wind only on maps with
/// enough wind (>= 0.6 of the 2.5 cap), otherwise solar.
fn power_def(ctx: &Ctx) -> Option<UnitDef> {
    if ctx.wind_max >= 0.6 * 2.5 {
        ctx.defs.wind.or(ctx.defs.solar)
    } else {
        ctx.defs.solar.or(ctx.defs.wind)
    }
}

fn wants_energy(ctx: &Ctx) -> bool {
    let Some(energy) = ctx.energy else { return false };
    let eco = ctx.cb.economy();
    eco.income(energy) < eco.usage(energy) + 2.0
}

/// The scripted opening.
enum Opening {
    Con,
    Factory,
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
}

impl Economy {
    pub fn new() -> Self {
        Economy { opening: Opening::Con, opening_mexes: Vec::new(), wind_built: 0, solars_built: 0 }
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
        let Some(metal) = ctx.metal else { return };
        let cur = current_metal(ctx);
        match self.opening {
            Opening::Con => {
                if let Some(con) = ctx.defs.con {
                    if cur >= metal_cost(ctx, con) && build_at(cb, unit, con, ctx.our_start, 300.0) {
                        self.opening = Opening::Factory;
                    }
                } else {
                    self.opening = Opening::Factory;
                }
            }
            Opening::Factory => {
                if let Some(f) = ctx.defs.factory {
                    if war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, f))
                        && build_at(cb, unit, f, ctx.our_start, 400.0)
                    {
                        // capture the 3 surrounding mexes next
                        let mut spots: Vec<usize> = (0..ctx.spots.len())
                            .filter(|i| !terr.owned[*i] && !terr.enemy[*i])
                            .collect();
                        spots.sort_by(|a, b| {
                            ctx.spots[*a]
                                .distance_2d(ctx.our_start)
                                .partial_cmp(&ctx.spots[*b].distance_2d(ctx.our_start))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        self.opening_mexes = spots.into_iter().take(3).collect();
                        self.opening = Opening::Mexes;
                    }
                } else {
                    self.opening = Opening::Mexes;
                }
            }
            Opening::Mexes => {
                let Some(mex) = ctx.defs.mex else { self.opening = Opening::WindGrid; return };
                let next = self.opening_mexes.iter().copied().find(|i| !ordered.contains(i) && !terr.owned[*i]);
                match next {
                    Some(i) if cur >= metal_cost(ctx, mex) => {
                        if build_at(cb, unit, mex, ctx.spots[i], 128.0) {
                            ordered.insert(i);
                        }
                    }
                    _ => self.opening = Opening::WindGrid,
                }
            }
            Opening::WindGrid => {
                // 6 turbines linking the surrounding mexes, only on windy maps
                if ctx.wind_max >= 0.6 * 2.5 && self.wind_built < 6 {
                    if let Some(wind) = ctx.defs.wind {
                        if cur >= metal_cost(ctx, wind) && self.opening_mexes.len() >= 2 {
                            // midpoint between two consecutive opening mexes
                            let i = self.wind_built % (self.opening_mexes.len() - 1);
                            let a = ctx.spots[self.opening_mexes[i]];
                            let b = ctx.spots[self.opening_mexes[i + 1]];
                            let mid = Vec3::xz((a.x + b.x) / 2.0, (a.z + b.z) / 2.0);
                            if build_at(cb, unit, wind, mid, 200.0) {
                                self.wind_built += 1;
                            }
                            return;
                        }
                    }
                }
                self.opening = Opening::Solars;
            }
            Opening::Solars => {
                if self.solars_built < 2 {
                    if let Some(solar) = ctx.defs.solar {
                        if cur >= metal_cost(ctx, solar) && build_at(cb, unit, solar, ctx.our_start, 300.0) {
                            self.solars_built += 1;
                        }
                        return;
                    }
                }
                self.opening = Opening::Done;
            }
            Opening::Done => {}
        }
        // steady-state commander: expansion, energy, factory rebuild
        if let Some(mex) = ctx.defs.mex {
            if let Some(i) = terr.expansion_spot(ctx, ordered) {
                if war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, mex))
                    && cur >= metal_cost(ctx, mex)
                    && build_at(cb, unit, mex, ctx.spots[i], 128.0)
                {
                    ordered.insert(i);
                    return;
                }
            }
        }
        if wants_energy(ctx) {
            if let Some(power) = power_def(ctx) {
                if war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, power))
                    && cur >= metal_cost(ctx, power)
                    && build_at(cb, unit, power, ctx.our_start, 300.0)
                {
                    return;
                }
            }
        }
        let has_factory = roles.values().any(|r| *r == Role::Factory);
        if !has_factory {
            if let Some(f) = ctx.defs.factory {
                if war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, f))
                    && cur >= metal_cost(ctx, f)
                {
                    build_at(cb, unit, f, ctx.our_start, 400.0);
                }
            }
        }
    }

    pub fn worker_idle(
        &mut self,
        ctx: &Ctx,
        unit: Unit,
        ordered: &mut HashSet<usize>,
        terr: &mut Territory,
        war: &WarChest,
    ) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let Some(mex) = ctx.defs.mex else { return };
        let cur = current_metal(ctx);
        if let Some(i) = terr.expansion_spot(ctx, ordered) {
            if war.can_spend(ctx, ProjectKind::Expansion, metal_cost(ctx, mex))
                && cur >= metal_cost(ctx, mex)
                && build_at(cb, unit, mex, ctx.spots[i], 128.0)
            {
                ordered.insert(i);
                return;
            }
        }
        if wants_energy(ctx) {
            if let Some(power) = power_def(ctx) {
                if war.can_spend(ctx, ProjectKind::Energy, metal_cost(ctx, power))
                    && cur >= metal_cost(ctx, power)
                    && build_at(cb, unit, power, ctx.our_start, 300.0)
                {
                    return;
                }
            }
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
            war.projects.iter().any(|p| p.kind == kind && (p.state == crate::war_chest::ProjectState::Queued || p.state == crate::war_chest::ProjectState::Active))
        };
        let count_alive = |roles: &HashMap<i32, Role>, pred: &dyn Fn(&Role) -> bool| roles.values().filter(|r| pred(r)).count();
        let _ = count_alive;

        // storages to reach the stockpile target
        let storage = cb.economy().storage(metal);
        if let Some(s) = ctx.defs.storage {
            if storage < war.stockpile_target && !has_project(war, ProjectKind::Storage) {
                let cost = metal_cost(ctx, s);
                if war.request(ctx, "storage", ProjectKind::Storage, Some(s), cost, 0.0) {
                    if let Some(w) = idle_worker(ctx, roles, ctx.our_start, 900.0) {
                        build_at(cb, w, s, ctx.our_start, 300.0);
                    }
                }
            }
        }
        // geothermal: priority once a geospot is on our half near the start
        if let Some(geo) = ctx.defs.geo {
            let near = ctx.geo_spots.iter().any(|g| g.distance_2d(ctx.our_start) < 1000.0);
            if near && income >= 25.0 && !has_project(war, ProjectKind::Energy) {
                let cost = metal_cost(ctx, geo);
                if war.request(ctx, "geo", ProjectKind::Energy, Some(geo), cost, 0.2) {
                    if let Some(w) = idle_worker(ctx, roles, ctx.our_start, 1200.0) {
                        let spot = ctx.geo_spots.iter().min_by(|a, b| {
                            a.distance_2d(ctx.our_start).partial_cmp(&b.distance_2d(ctx.our_start)).unwrap_or(std::cmp::Ordering::Equal)
                        }).copied();
                        if let Some(g) = spot {
                            build_at(cb, w, geo, g, 64.0);
                        }
                    }
                }
            }
        }
        // first fusion at 40-50 m/s, pausing other production (army checks the ledger)
        if let Some(fusion) = ctx.defs.fusion {
            let fusions_alive = cb.team_units().iter().filter(|u| u.def().is_some_and(|d| d.name() == "energyfusion")).count();
            if income >= 40.0 && fusions_alive == 0 && !has_project(war, ProjectKind::Energy) {
                let cost = metal_cost(ctx, fusion);
                if war.request(ctx, "fusion1", ProjectKind::Energy, Some(fusion), cost, 0.5) {
                    if let Some(w) = idle_worker(ctx, roles, ctx.our_start, 1600.0) {
                        build_at(cb, w, fusion, ctx.our_start, 400.0);
                    }
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
