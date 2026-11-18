//! Economy: the commander's opening (mex, energy, constructors, factory) and
//! worker mex expansion up to the territory target.

use std::collections::{HashMap, HashSet};

use springai::{Callback, Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;

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

fn wants_energy(ctx: &Ctx) -> bool {
    let Some(energy) = ctx.energy else { return false };
    let eco = ctx.cb.economy();
    eco.income(energy) < eco.usage(energy) + 2.0
}

/// A static power plant to build when energy is short.
fn power_def(ctx: &Ctx) -> Option<UnitDef> {
    ctx.defs.solar.or_else(|| ctx.defs.get("energywind"))
}

pub fn commander_idle(ctx: &Ctx, unit: Unit, roles: &HashMap<i32, Role>, ordered: &mut HashSet<usize>, terr: &Territory) {
    let cb = ctx.cb;
    let Some(metal) = ctx.metal else { return };
    let Some(mex) = ctx.defs.mex else { return };
    let cur = current_metal(ctx);
    let mex_cost = mex.cost(metal);

    if let Some(i) = terr.expansion_spot(ctx, ordered) {
        if cur >= mex_cost && build_at(cb, unit, mex, ctx.spots[i], 128.0) {
            ordered.insert(i);
            return;
        }
    }
    if wants_energy(ctx) {
        if let Some(power) = power_def(ctx) {
            if cur >= power.cost(metal) && build_at(cb, unit, power, ctx.our_start, 300.0) {
                return;
            }
        }
    }
    let workers = roles.values().filter(|r| **r == Role::Worker).count();
    if workers < 3 {
        if let Some(con) = ctx.defs.con {
            if cur >= con.cost(metal) && build_at(cb, unit, con, ctx.our_start, 300.0) {
                return;
            }
        }
    }
    let has_factory = roles.values().any(|r| *r == Role::Factory);
    if !has_factory {
        if let Some(f) = ctx.defs.factory {
            if cur >= f.cost(metal) && build_at(cb, unit, f, ctx.our_start, 400.0) {
                return;
            }
        }
    }
    if let Some(power) = power_def(ctx) {
        if cur >= power.cost(metal) {
            build_at(cb, unit, power, ctx.our_start, 300.0);
        }
    }
}

pub fn worker_idle(ctx: &Ctx, unit: Unit, ordered: &mut HashSet<usize>, terr: &Territory) {
    let cb = ctx.cb;
    let Some(metal) = ctx.metal else { return };
    let Some(mex) = ctx.defs.mex else { return };
    let cur = current_metal(ctx);

    if let Some(i) = terr.expansion_spot(ctx, ordered) {
        if cur >= mex.cost(metal) && build_at(cb, unit, mex, ctx.spots[i], 128.0) {
            ordered.insert(i);
            return;
        }
    }
    if wants_energy(ctx) {
        if let Some(power) = power_def(ctx) {
            if cur >= power.cost(metal) && build_at(cb, unit, power, ctx.our_start, 300.0) {
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
