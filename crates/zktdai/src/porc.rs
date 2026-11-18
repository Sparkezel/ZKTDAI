//! Perimeter porc: escalating turret tiers just behind the front line, AA
//! when the enemy shows air, and terraform ditches across vehicle approaches
//! (Zero-K's `unit_terraform.lua` gadget, via its internal command).

use std::collections::HashMap;

use springai::{Unit, UnitDef, Vec3, NO_TIMEOUT, opt};

use crate::ai::Ctx;
use crate::defs::{Role, cmd};
use crate::economy::build_at;
use crate::territory::Territory;

pub struct Porc {
    next_ditch: i32,
    next_aa: i32,
}

impl Porc {
    pub fn new() -> Self {
        Porc { next_ditch: 1800, next_aa: 0 }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory) {
        if !ctx.opts.porc {
            return;
        }
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let ladder = &ctx.defs.turret_ladder;
        if !ladder.is_empty() {
            let income = cb.economy().income(metal);
            let tier = if income < 3.0 {
                0
            } else if income < 8.0 {
                1
            } else if income < 16.0 {
                2
            } else {
                3
            };
            let def = ladder[tier.min(ladder.len() - 1)];
            if cb.economy().current(metal) >= def.cost(metal) * 1.5 {
                if let Some(site) = self.turret_site(ctx, terr, def) {
                    if let Some(builder) = idle_worker_near(ctx, roles, site, 1600.0) {
                        let y = cb.map().elevation_at(site.x, site.z);
                        if builder.build(def, Vec3::new(site.x, y, site.z), 0, opt::NONE) {
                            cb.log(&format!("[porc] frame={} {} at ({:.0},{:.0})", ctx.frame, def.name(), site.x, site.z));
                        }
                    }
                }
            }
        }
        if ctx.frame >= self.next_aa && ctx.defs.aa_ladder.first().is_some() {
            let enemy_air = cb
                .enemy_units_in_radar_and_los()
                .iter()
                .any(|e| e.def().is_some_and(|d| d.is_able_to_fly()));
            if enemy_air {
                let aa = ctx.defs.aa_ladder[0];
                if cb.economy().current(metal) >= aa.cost(metal) * 1.5 {
                    if let Some(builder) = idle_worker_near(ctx, roles, ctx.our_start, 800.0) {
                        build_at(cb, builder, aa, ctx.our_start, 300.0);
                    }
                }
                self.next_aa = ctx.frame + 900;
            }
        }
        // one radar near the base once we can afford it
        if let Some(radar) = ctx.defs.get("staticradar") {
            let have = cb
                .team_units()
                .iter()
                .any(|u| u.def().is_some_and(|d| d.name() == "staticradar"));
            if !have && cb.economy().current(metal) >= radar.cost(metal) * 1.5 {
                if let Some(builder) = idle_worker_near(ctx, roles, ctx.our_start, 800.0) {
                    build_at(cb, builder, radar, ctx.our_start, 300.0);
                }
            }
        }
        if ctx.frame >= self.next_ditch {
            self.ditch(ctx, roles, terr);
            self.next_ditch = ctx.frame + 1500;
        }
    }

    /// A build site for `def` just behind the front line, not crowded by
    /// existing turrets.
    fn turret_site(&self, ctx: &Ctx, terr: &Territory, def: UnitDef) -> Option<Vec3> {
        let cb = ctx.cb;
        let dir = terr.enemy_centroid - terr.our_centroid;
        let len = dir.length_2d().max(1.0);
        let want = terr.front + dir * (-250.0 / len);
        let site = cb.map().find_closest_build_site(def, want, 350.0, 0, 0);
        if site.x < 0.0 {
            return None;
        }
        let crowded = cb
            .friendly_units_in(site, 250.0)
            .iter()
            .any(|u| u.def().is_some_and(|d| !d.is_able_to_move() && d.is_able_to_attack()));
        if crowded { None } else { Some(site) }
    }

    /// Dig a ditch line across the front with up to two idle workers.
    fn ditch(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory) {
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        if cb.economy().income(metal) < 2.0 {
            return;
        }
        let builders: Vec<Unit> = cb
            .team_units()
            .into_iter()
            .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
            .take(2)
            .collect();
        if builders.is_empty() {
            return;
        }
        let dir = terr.enemy_centroid - terr.our_centroid;
        let len = dir.length_2d().max(1.0);
        let perp = Vec3::xz(-dir.z / len, dir.x / len);
        let mid = terr.front;
        let points: Vec<Vec3> =
            (-3..=3).map(|k| Vec3::xz(mid.x + perp.x * 100.0 * k as f32, mid.z + perp.z * 100.0 * k as f32)).collect();
        let n = points.len() as f32;
        let (cx, cz) = points.iter().fold((0.0, 0.0), |(x, z), p| (x + p.x / n, z + p.z / n));
        let tag = (ctx.frame + 1) as f32;
        let mut params = vec![
            cmd::KIND_RAISE,
            cb.game().my_team() as f32,
            cx,
            cz,
            tag,
            0.0,   // a line, not a closed area
            -12.0, // ditch depth: raise with negative height
            n,
            builders.len() as f32,
            0.0, // any volume change
        ];
        params.extend(points.iter().flat_map(|p| [p.x, p.z]));
        params.extend(builders.iter().map(|b| b.id() as f32));
        builders[0].execute_custom_command(cmd::TERRAFORM_INTERNAL, &params, opt::NONE, NO_TIMEOUT);
        let y = cb.map().elevation_at(cx, cz);
        for b in &builders {
            b.execute_custom_command(cmd::RAISE, &[cx, y, cz, tag], opt::NONE, NO_TIMEOUT);
        }
        cb.log(&format!("[porc] frame={} ditch at ({cx:.0},{cz:.0}) by {} workers", ctx.frame, builders.len()));
    }
}

fn idle_worker_near(ctx: &Ctx, roles: &HashMap<i32, Role>, pos: Vec3, max_dist: f32) -> Option<Unit> {
    ctx.cb
        .team_units()
        .into_iter()
        .filter(|u| roles.get(&u.id()) == Some(&Role::Worker) && u.num_current_commands() == 0)
        .filter(|u| u.pos().distance_2d(pos) <= max_dist)
        .min_by(|a, b| {
            a.pos().distance_2d(pos).partial_cmp(&b.pos().distance_2d(pos)).unwrap_or(std::cmp::Ordering::Equal)
        })
}
