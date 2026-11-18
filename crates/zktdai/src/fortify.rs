//! Fortify: walled mexes (enemy-facing 60-elmo arc walls terraformed around
//! forward/hot mexes) and caretaker placement (naked only under local
//! superiority; contested sites get pits/walls — phase 3).

use std::collections::{HashMap, HashSet};

use springai::Unit;

use crate::ai::Ctx;
use crate::defs::Role;
use crate::economy::build_at;
use crate::territory::Territory;
use crate::terraform;
use crate::war_chest::{ProjectKind, WarChest};

pub struct Fortify {
    /// Metal spots that already have a wall.
    walled: HashSet<usize>,
    caretakers_built: usize,
    next: i32,
}

impl Fortify {
    pub fn new() -> Self {
        Fortify { walled: HashSet::new(), caretakers_built: 0, next: 2400 }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        if ctx.frame < self.next {
            return;
        }
        self.next = ctx.frame + 900;
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let income = cb.economy().income(metal);

        // walled mexes near the front / in hot areas
        if ctx.opts.walled_mexes {
            for (i, spot) in ctx.spots.iter().enumerate() {
                if !terr.owned[i] || self.walled.contains(&i) {
                    continue;
                }
                if spot.distance_2d(terr.front) > 1500.0 {
                    continue;
                }
                let toward = terr.enemy_centroid - *spot;
                let pts = terraform::arc_points(*spot, toward, 40.0, 180.0, 30.0);
                let builders: Vec<Unit> = idle_workers(ctx, roles).into_iter().take(2).collect();
                if builders.len() >= 2 && terraform::terraform(cb, &builders, &pts, 60.0, ctx.frame + 1) {
                    self.walled.insert(i);
                    cb.log(&format!("[fortify] frame={} wall around mex {} at ({:.0},{:.0})", ctx.frame, i, spot.x, spot.z));
                }
            }
        }

        // caretakers: naked only under local superiority
        if ctx.opts.caretakers && income > 10.0 && self.caretakers_built < 2 {
            if let Some(ca) = ctx.defs.caretaker {
                let cost = ca.cost(metal);
                if !war.can_spend(ctx, ProjectKind::Porc, cost) {
                    return;
                }
                let dir = terr.enemy_centroid - terr.our_centroid;
                let len = dir.length_2d().max(1.0);
                let site = terr.front + dir * (-400.0 / len);
                let enemy_near = cb.enemy_units_in_radar_and_los().iter().any(|e| e.pos().distance_2d(site) < 1200.0);
                let army_near = cb
                    .friendly_units_in(site, 400.0)
                    .iter()
                    .any(|u| roles.get(&u.id()).is_some_and(|r| r.is_combat()));
                if !enemy_near || army_near {
                    if let Some(w) = idle_workers(ctx, roles).into_iter().find(|w| w.pos().distance_2d(site) < 1200.0) {
                        if build_at(cb, w, ca, site, 200.0) {
                            self.caretakers_built += 1;
                            war.note_porc_spend(cost);
                            cb.log(&format!("[fortify] frame={} caretaker at ({:.0},{:.0})", ctx.frame, site.x, site.z));
                        }
                    }
                }
            }
        }
    }
}

fn idle_workers(ctx: &Ctx, roles: &HashMap<i32, Role>) -> Vec<Unit> {
    ctx.cb
        .team_units()
        .into_iter()
        .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
        .collect()
}
