//! Fortify: walled mexes (enemy-facing 60-elmo arc walls terraformed around
//! forward/hot mexes). Caretaker placement lives in the FOB module (`fob.rs`).

use std::collections::{HashMap, HashSet};

use springai::Unit;

use crate::ai::Ctx;
use crate::defs::Role;
use crate::terraform;
use crate::territory::Territory;
use crate::war_chest::WarChest;

pub struct Fortify {
    /// Metal spots that already have a wall.
    walled: HashSet<usize>,
    next: i32,
}

impl Fortify {
    pub fn new() -> Self {
        Fortify { walled: HashSet::new(), next: 2400 }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, _war: &mut WarChest) {
        if ctx.frame < self.next {
            return;
        }
        self.next = ctx.frame + 900;
        let cb = ctx.cb;

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
                // flat-top wall: level to ground + 60 (zkgbai geo-box style)
                let wall_top = cb.map().elevation_at(spot.x, spot.z) + 60.0;
                let builders: Vec<Unit> = idle_workers(ctx, roles).into_iter().take(2).collect();
                if builders.len() >= 2
                    && terraform::terraform_level(cb, &builders, &pts, wall_top, terraform::next_tag())
                {
                    self.walled.insert(i);
                    cb.log(&format!(
                        "[fortify] frame={} wall around mex {} at ({:.0},{:.0})",
                        ctx.frame, i, spot.x, spot.z
                    ));
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
