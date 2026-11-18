//! Perimeter porc v2: income-keyed turret tiers, lotus-first at mex clusters,
//! conditional picks (Picket vs enemy jumpbots/spiders, Stardust vs raider-heavy,
//! Stinger once we hold the midline, heavies at high income), the AA ladder, and
//! decon of turrets left behind a moved front. All spending goes through the
//! war_chest ledger (porc cap per income window).

use std::collections::{HashMap, HashSet};

use springai::{Unit, UnitDef, Vec3, opt};

use crate::ai::Ctx;
use crate::defs::Role;
use crate::economy::build_at;
use crate::territory::Territory;
use crate::terraform;
use crate::war_chest::{ProjectKind, WarChest};

/// Frames without combat after which a backline turret may be deconned (10 min).
const DECON_QUIET_FRAMES: i32 = 18000;
/// How far behind the front (toward our side) a turret must be to decon.
const DECON_BEHIND_FRONT: f32 = 300.0;

pub struct Porc {
    next_turret: i32,
    next_aa: i32,
    next_ditch: i32,
    next_decon: i32,
    /// Turret unit id -> last frame it saw combat.
    pub last_combat: HashMap<i32, i32>,
    /// Metal spots that already have a turret (lotus-first at mex clusters).
    turreted_spots: HashSet<usize>,
}

impl Porc {
    pub fn new() -> Self {
        Porc {
            next_turret: 300,
            next_aa: 0,
            next_ditch: 1800,
            next_decon: 3000,
            last_combat: HashMap::new(),
            turreted_spots: HashSet::new(),
        }
    }

    /// Stamp a turret's last combat frame (from our WeaponFired).
    pub fn note_combat(&mut self, ctx: &Ctx, unit: Unit, frame: i32) {
        if unit.def().is_some_and(|d| !d.is_able_to_move() && d.is_able_to_attack()) {
            self.last_combat.insert(unit.id(), frame);
        }
    }

    /// Stamp turrets near a damage event.
    pub fn note_damage(&mut self, ctx: &Ctx, pos: Vec3, frame: i32) {
        for u in ctx.cb.friendly_units_in(pos, 300.0) {
            if u.def().is_some_and(|d| !d.is_able_to_move() && d.is_able_to_attack()) {
                self.last_combat.insert(u.id(), frame);
            }
        }
    }

    pub fn update(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory, war: &mut WarChest) {
        if !ctx.opts.porc {
            return;
        }
        let cb = ctx.cb;
        let Some(metal) = ctx.metal else { return };
        let income = cb.economy().income(metal);
        let enemies = cb.enemy_units_in_radar_and_los();
        let enemy_jump_spider = enemies.iter().any(|e| {
            e.def().is_some_and(|d| {
                let n = d.name();
                d.is_able_to_move() && (n.contains("jump") || n.contains("spider"))
            })
        });
        let enemy_raider_heavy = enemies
            .iter()
            .filter(|e| e.def().is_some_and(|d| ctx.defs.role(d) == Role::Raider))
            .count()
            >= 5;
        let enemy_air = enemies.iter().any(|e| e.def().is_some_and(|d| d.is_able_to_fly()));

        if ctx.frame >= self.next_turret {
            self.next_turret = ctx.frame + 150;
            let ladder = &ctx.defs.turret_ladder; // Lotus, Stardust, Gauss, Stinger, Desolator, Lucifer
            if !ladder.is_empty() {
                let base = if income < 8.0 {
                    0
                } else if income < 20.0 || enemy_raider_heavy {
                    1
                } else if income < 40.0 {
                    2
                } else if terr.share >= 0.5 {
                    3 // midline held: Stingers by default
                } else {
                    2
                };
                let tier = if income >= 60.0 { 5.min(ladder.len() - 1) } else { base.min(ladder.len() - 1) };
                // Picket as perimeter defense vs jumpbots/spiders.
                let def = if enemy_jump_spider && ctx.defs.picket.is_some() {
                    ctx.defs.picket.unwrap()
                } else {
                    ladder[tier]
                };
                let cost = def.cost(metal);
                if war.can_spend(ctx, ProjectKind::Porc, cost) {
                    if let Some(site) = self.turret_site(ctx, terr, def, tier) {
                        if let Some(builder) = idle_worker_near(ctx, roles, site, 1600.0) {
                            let y = cb.map().elevation_at(site.x, site.z);
                            if builder.build(def, Vec3::new(site.x, y, site.z), 0, opt::NONE) {
                                war.note_porc_spend(cost);
                                cb.log(&format!(
                                    "[porc] frame={} {} at ({:.0},{:.0}) tier={tier}",
                                    ctx.frame,
                                    def.name(),
                                    site.x,
                                    site.z
                                ));
                            }
                        }
                    }
                }
            }
        }

        // AA ladder when the enemy shows air
        if ctx.frame >= self.next_aa && !ctx.defs.aa_ladder.is_empty() {
            if enemy_air {
                let aa = if income < 15.0 {
                    ctx.defs.aa_ladder[0]
                } else if income < 40.0 && ctx.defs.aa_ladder.len() > 1 {
                    ctx.defs.aa_ladder[1]
                } else if ctx.defs.aa_ladder.len() > 2 {
                    ctx.defs.aa_ladder[2]
                } else {
                    ctx.defs.aa_ladder[0]
                };
                let cost = aa.cost(metal);
                if war.can_spend(ctx, ProjectKind::Porc, cost) {
                    if let Some(builder) = idle_worker_near(ctx, roles, ctx.our_start, 800.0) {
                        if build_at(cb, builder, aa, ctx.our_start, 300.0) {
                            war.note_porc_spend(cost);
                        }
                    }
                }
                self.next_aa = ctx.frame + 900;
            }
        }

        // one radar near the base
        if let Some(radar) = ctx.defs.radar {
            let have = cb.team_units().iter().any(|u| u.def().is_some_and(|d| d.name() == "staticradar"));
            if !have && war.can_spend(ctx, ProjectKind::Porc, radar.cost(metal)) {
                if let Some(builder) = idle_worker_near(ctx, roles, ctx.our_start, 800.0) {
                    build_at(cb, builder, radar, ctx.our_start, 300.0);
                }
            }
        }

        // basic front ditch
        if ctx.frame >= self.next_ditch {
            self.ditch(ctx, roles, terr);
            self.next_ditch = ctx.frame + 1500;
        }

        // decon turrets left behind the front
        if ctx.frame >= self.next_decon {
            self.next_decon = ctx.frame + 600;
            self.decon(ctx, roles, terr);
        }
    }

    /// A build site for `def`: lotus-first at untouched owned mex clusters near
    /// the front (low tiers), otherwise just behind the front line.
    fn turret_site(&mut self, ctx: &Ctx, terr: &Territory, def: UnitDef, tier: usize) -> Option<Vec3> {
        let cb = ctx.cb;
        if tier <= 1 {
            // nearest owned, not-yet-turreted mex spot to the front
            if let Some((i, spot)) = ctx
                .spots
                .iter()
                .enumerate()
                .filter(|(i, _)| terr.owned[*i] && !self.turreted_spots.contains(i))
                .min_by(|(_, a), (_, b)| {
                    a.distance_2d(terr.front).partial_cmp(&b.distance_2d(terr.front)).unwrap_or(std::cmp::Ordering::Equal)
                })
            {
                let dir = terr.our_centroid - *spot;
                let len = dir.length_2d().max(1.0);
                let want = *spot + dir * (120.0 / len);
                let site = cb.map().find_closest_build_site(def, want, 200.0, 0, 0);
                if site.x >= 0.0 {
                    self.turreted_spots.insert(i);
                    return Some(site);
                }
            }
        }
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
        let tag = ctx.frame + 1;
        if terraform::terraform(cb, &builders, &points, -12.0, tag) {
            cb.log(&format!("[porc] frame={} ditch at ({:.0},{:.0}) by {} workers", ctx.frame, mid.x, mid.z, builders.len()));
        }
    }

    /// Reclaim our turrets that have seen no combat for a long time and sit
    /// behind the current front.
    fn decon(&mut self, ctx: &Ctx, roles: &HashMap<i32, Role>, terr: &Territory) {
        let cb = ctx.cb;
        let dir = terr.enemy_centroid - terr.our_centroid;
        let len = dir.length_2d().max(1.0);
        let toward_us = dir * (-1.0 / len);
        let mut doomed = Vec::new();
        for u in cb.team_units() {
            let Some(d) = u.def() else { continue };
            if !(!d.is_able_to_move() && d.is_able_to_attack()) {
                continue;
            }
            let quiet = ctx.frame - self.last_combat.get(&u.id()).copied().unwrap_or(0) > DECON_QUIET_FRAMES;
            let behind = (u.pos() - terr.front).length_2d() > DECON_BEHIND_FRONT
                && (u.pos() - terr.front).x * toward_us.x + (u.pos() - terr.front).z * toward_us.z > 0.0;
            if quiet && behind {
                doomed.push(u);
            }
        }
        for t in doomed {
            if let Some(w) = idle_worker_near(ctx, roles, t.pos(), 900.0) {
                if w.reclaim_unit(t, opt::NONE) {
                    cb.log(&format!("[porc] frame={} decon {} ({})", ctx.frame, t.def().map(|d| d.name()).unwrap_or_default(), t.id()));
                }
            }
        }
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
