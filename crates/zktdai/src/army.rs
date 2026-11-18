//! The offense: factory production mix and the combat roles — raider
//! detachments (box-in and harassment), riot-skirmishers and assault armies
//! (front line, then pushes), artillery groups (back line), air.

use std::collections::HashMap;

use springai::{Unit, UnitDef, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::territory::Territory;

pub struct Army {
    cycle: usize,
}

impl Army {
    pub fn new() -> Self {
        Army { cycle: 0 }
    }

    pub fn push_ready(&self, ctx: &Ctx, roles: &HashMap<i32, Role>) -> bool {
        roles.values().filter(|r| r.is_combat()).count() as i32 >= ctx.opts.push_size
    }

    /// A factory that just went idle builds the next unit of the mix.
    pub fn factory_idle(&mut self, ctx: &Ctx, factory: Unit) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let Some(def) = self.next_unit(ctx) else { return };
        if cb.economy().current(metal) < def.cost(metal) {
            return;
        }
        let pos = factory.pos();
        factory.build(def, pos, 0, opt::NONE);
    }

    /// Production weights per 100 builds: raiders dominate the opening
    /// (box-in), then the army shifts to skirm/assault/arty.
    fn next_unit(&mut self, ctx: &Ctx) -> Option<UnitDef> {
        let d = &ctx.defs;
        let (wr, ws, wa, wt): (i32, i32, i32, i32) =
            if ctx.frame < 6000 { (70, 25, 0, 5) } else { (25, 30, 25, 20) };
        let total = wr + ws + wa + wt;
        let pick = (self.cycle as i32) % total;
        self.cycle += 1;
        let def = if pick < wr {
            d.raider
        } else if pick < wr + ws {
            d.skirm
        } else if pick < wr + ws + wa {
            d.assault
        } else {
            d.arty
        };
        def.or(d.raider).or(d.skirm).or(d.assault).or(d.arty)
    }

    pub fn combat_idle(&mut self, ctx: &Ctx, unit: Unit, role: Role, terr: &Territory, push: bool) {
        let cb = ctx.cb;
        let me = unit.pos();
        let by_dist = |a: &Unit, b: &Unit| {
            a.pos().distance_2d(me).partial_cmp(&b.pos().distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
        };
        // engage anything close
        if let Some(t) = cb.enemy_units().into_iter().filter(|e| e.pos().distance_2d(me) < 800.0).min_by(by_dist) {
            unit.attack(t, opt::NONE);
            return;
        }
        match role {
            Role::Raider => {
                if ctx.opts.raiding {
                    let target = terr
                        .enemy_spots
                        .iter()
                        .min_by(|a, b| {
                            a.distance_2d(me).partial_cmp(&b.distance_2d(me)).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .copied()
                        .unwrap_or(ctx.enemy_target);
                    unit.fight(target, opt::NONE);
                } else {
                    unit.fight(terr.front, opt::NONE);
                }
            }
            Role::Arty => {
                let range = unit.max_range().max(1.0);
                match cb.enemy_units().into_iter().filter(|e| e.pos().distance_2d(me) < range * 0.9).min_by(by_dist) {
                    Some(t) => {
                        unit.attack(t, opt::NONE);
                    }
                    None => {
                        // hold a staging point behind the front
                        let dir = terr.enemy_centroid - terr.our_centroid;
                        let len = dir.length_2d().max(1.0);
                        let staging = terr.front + dir * (-300.0 / len);
                        unit.fight(staging, opt::NONE);
                    }
                }
            }
            _ => {
                if push {
                    unit.fight(ctx.enemy_target, opt::NONE);
                } else {
                    unit.fight(terr.front, opt::NONE);
                }
            }
        }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory) {
        let combat = roles.values().filter(|r| r.is_combat()).count();
        ctx.cb.log(&format!(
            "[army] frame={} combat={} territory={:.0}% push_ready={}",
            ctx.frame,
            combat,
            terr.share * 100.0,
            self.push_ready(ctx, roles)
        ));
    }
}
